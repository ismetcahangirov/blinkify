//! Decoding a source's audio for the preview: an FFmpeg process per stream,
//! producing interleaved stereo `f32` at the output rate into a bounded ring.
//!
//! The start is sample-exact. The demuxer seeks a little early and `atrim`
//! cuts at the requested time — with `-copyts`, in the stream's own
//! timestamps — so a clip starts on the sample it names. Preview speed is a
//! time-stretch (`atempo`) that keeps pitch, so speech stays intelligible at
//! half and double speed; it exists only on this path and has nothing to do
//! with the lossless speed change in Epic #6.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::CHANNELS;
use crate::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand,
};

/// How far before the start the demuxer is asked to seek. Audio packets are
/// all independently decodable, but a container seeks by its index, which may
/// land just after the time asked for; `atrim` removes the excess exactly.
const SEEK_MARGIN_SECONDS: f64 = 0.5;

/// How long a blocked producer sleeps before re-checking for a cancel.
const PRODUCER_RECHECK: Duration = Duration::from_millis(50);

#[derive(Debug, Default)]
struct State {
    samples: VecDeque<f32>,
    finished: bool,
    closed: bool,
}

/// A bounded buffer of interleaved stereo samples from one decoder.
#[derive(Debug)]
pub struct SampleRing {
    capacity: usize,
    state: Mutex<State>,
    changed: Condvar,
}

impl SampleRing {
    /// A ring holding at most `frames` stereo frames.
    #[must_use]
    pub fn new(frames: usize) -> Self {
        Self {
            capacity: frames.max(1).saturating_mul(CHANNELS),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Append samples, blocking while the ring is full. `false` if the ring
    /// was closed or `cancel` fired.
    pub fn push(&self, mut samples: &[f32], cancel: &CancelToken) -> bool {
        while !samples.is_empty() {
            let mut state = self.lock();
            while !state.closed && !cancel.is_cancelled() && state.samples.len() >= self.capacity {
                state = self
                    .changed
                    .wait_timeout(state, PRODUCER_RECHECK)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            if state.closed || cancel.is_cancelled() {
                return false;
            }
            let room = self.capacity - state.samples.len();
            let (now, later) = samples.split_at(room.min(samples.len()));
            state.samples.extend(now.iter().copied());
            drop(state);
            self.changed.notify_all();
            samples = later;
        }
        true
    }

    /// Move up to `out.len()` samples into `out` without waiting. Returns how
    /// many were moved.
    pub fn take(&self, out: &mut [f32]) -> usize {
        let mut state = self.lock();
        let count = out.len().min(state.samples.len());
        for (slot, sample) in out.iter_mut().zip(state.samples.drain(..count)) {
            *slot = sample;
        }
        drop(state);
        self.changed.notify_all();
        count
    }

    /// Wait until at least `samples` are buffered or the producer has
    /// finished, for at most `timeout`. Returns the number buffered.
    pub fn wait_for(&self, samples: usize, timeout: Duration) -> usize {
        let state = self.lock();
        if state.samples.len() >= samples || state.finished || state.closed {
            return state.samples.len();
        }
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.samples.len() < samples && !state.finished && !state.closed
            })
            .unwrap_or_else(PoisonError::into_inner);
        state.samples.len()
    }

    /// The producer delivered its last sample.
    pub fn finish(&self) {
        self.lock().finished = true;
        self.changed.notify_all();
    }

    /// Whether the producer finished and every sample was taken.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        let state = self.lock();
        state.finished && state.samples.is_empty()
    }

    /// The consumer is gone.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.samples.clear();
        drop(state);
        self.changed.notify_all();
    }
}

/// What to decode.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRequest {
    pub source: PathBuf,
    /// The stream's absolute index in the file.
    pub stream: u32,
    /// Where to start, in the stream's own time, in seconds.
    pub start_seconds: f64,
    /// Where the stream's first sample is, in the same time. A start before
    /// it is padded with silence, so the output lines up with the timeline.
    pub stream_start_seconds: f64,
    pub sample_rate: u32,
    /// Preview speed: 1.0 is normal, 0.5 half, 2.0 double.
    pub tempo: f64,
}

impl AudioRequest {
    #[must_use]
    pub fn command(&self) -> SidecarCommand {
        let mut command = SidecarCommand::ffmpeg()
            .option("-v", "error")
            .flag("-copyts")
            .option("-seek_timestamp", "1")
            .flag("-noaccurate_seek");
        let seek = self.start_seconds - SEEK_MARGIN_SECONDS;
        if seek > 0.0 {
            command = command.option("-ss", format!("{seek:.6}"));
        }
        let mut graph = format!(
            "atrim=start={start:.6},aresample={rate},aformat=sample_fmts=flt:channel_layouts=stereo",
            start = self.start_seconds.max(0.0),
            rate = self.sample_rate,
        );
        for factor in tempo_stages(self.tempo) {
            let _ = write!(graph, ",atempo={factor}");
        }
        command
            .input(&self.source)
            .option("-map", format!("0:{}", self.stream))
            .flags(&["-vn", "-sn", "-dn"])
            .option("-af", graph)
            .option("-ar", self.sample_rate.to_string())
            .option("-ac", "2")
            .option("-f", "f32le")
            .output_stdout()
    }

    /// Frames of silence to put in front of the decoded audio: the distance
    /// from the requested start to the stream's first sample, at the output
    /// rate and the preview tempo.
    #[must_use]
    pub fn lead_in_frames(&self) -> usize {
        let gap = (self.stream_start_seconds - self.start_seconds).max(0.0);
        let frames = (gap * f64::from(self.sample_rate) / self.tempo.max(0.01)).round();
        // Bounded by any real stream's start offset.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = frames as usize;
        frames
    }
}

/// `atempo` takes 0.5 to 100 in one stage; slower speeds chain stages.
pub(crate) fn tempo_stages(tempo: f64) -> Vec<f64> {
    let mut stages = Vec::new();
    let mut left = tempo;
    if (left - 1.0).abs() < 1e-9 {
        return stages;
    }
    while left < 0.5 {
        stages.push(0.5);
        left /= 0.5;
    }
    stages.push(left);
    stages
}

/// How an audio decode ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEnd {
    Finished,
    Stopped,
    Failed(String),
}

/// One running audio decode filling a [`SampleRing`].
#[derive(Debug)]
pub struct AudioDecoder {
    cancel: CancelToken,
    ring: Arc<SampleRing>,
    end: Arc<Mutex<Option<AudioEnd>>>,
    watcher: Option<JoinHandle<()>>,
}

impl AudioDecoder {
    /// Start decoding `request` into `ring`. Returns at once.
    #[must_use]
    pub fn start(
        orchestrator: &Orchestrator,
        request: &AudioRequest,
        ring: Arc<SampleRing>,
    ) -> Self {
        let cancel = CancelToken::default();
        let lead_in = vec![0.0_f32; request.lead_in_frames() * CHANNELS];
        let mut pending: Vec<u8> = Vec::new();
        let mut lead_in_sent = lead_in.is_empty();
        let producer = Arc::clone(&ring);
        let producer_cancel = cancel.clone();
        let options = JobOptions::default()
            .cancel_token(cancel.clone())
            .on_chunk(move |chunk| {
                if !lead_in_sent {
                    lead_in_sent = true;
                    if !producer.push(&lead_in, &producer_cancel) {
                        return Flow::Stop;
                    }
                }
                pending.extend_from_slice(chunk);
                let whole = pending.len() - pending.len() % 4;
                let samples: Vec<f32> = pending
                    .get(..whole)
                    .unwrap_or_default()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| f32::from_le_bytes(*bytes))
                    .collect();
                pending.drain(..whole);
                if producer.push(&samples, &producer_cancel) {
                    Flow::Continue
                } else {
                    Flow::Stop
                }
            });
        let job = orchestrator.run(request.command(), Priority::Playback, options);
        let end = Arc::new(Mutex::new(None));
        let watcher = {
            let end = Arc::clone(&end);
            let ring = Arc::clone(&ring);
            thread::Builder::new()
                .name("preview-audio".to_owned())
                .spawn(move || {
                    let outcome = match job.wait() {
                        Ok(_) => AudioEnd::Finished,
                        Err(JobError::Cancelled | JobError::ShuttingDown) => AudioEnd::Stopped,
                        Err(error) => AudioEnd::Failed(error.to_string()),
                    };
                    ring.finish();
                    *end.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
                })
                .ok()
        };
        Self {
            cancel,
            ring,
            end,
            watcher,
        }
    }

    #[must_use]
    pub fn end(&self) -> Option<AudioEnd> {
        self.end
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stop the decode and wait until its process has exited.
    pub fn stop(mut self) {
        self.stop_in_place();
    }

    fn stop_in_place(&mut self) {
        self.cancel.cancel();
        self.ring.close();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        self.stop_in_place();
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn slow_speeds_chain_tempo_stages_and_normal_speed_has_none() {
        assert!(tempo_stages(1.0).is_empty());
        assert_eq!(tempo_stages(2.0), vec![2.0]);
        assert_eq!(tempo_stages(0.5), vec![0.5]);
        assert_eq!(tempo_stages(0.25), vec![0.5, 0.5]);
    }

    #[test]
    fn a_start_before_the_stream_is_padded_with_silence() {
        let request = AudioRequest {
            source: PathBuf::from("a.mp4"),
            stream: 1,
            start_seconds: 0.0,
            stream_start_seconds: 0.021,
            sample_rate: 48_000,
            tempo: 1.0,
        };
        assert_eq!(request.lead_in_frames(), 1008);
        assert_eq!(
            AudioRequest {
                start_seconds: 1.0,
                ..request.clone()
            }
            .lead_in_frames(),
            0
        );
        let command = request.command().to_string();
        assert!(!command.contains("-ss"), "{command}");
        assert!(command.contains("atrim=start=0.000000"), "{command}");
    }

    #[test]
    fn the_command_trims_exactly_where_it_seeks_early() {
        let request = AudioRequest {
            source: PathBuf::from("a.mp4"),
            stream: 1,
            start_seconds: 10.0,
            stream_start_seconds: 0.0,
            sample_rate: 44_100,
            tempo: 0.25,
        };
        let command = request.command().to_string();
        assert!(command.contains("-ss 9.500000"), "{command}");
        assert!(
            command.contains("atrim=start=10.000000,aresample=44100"),
            "{command}"
        );
        assert!(command.contains("atempo=0.5,atempo=0.5"), "{command}");
        assert!(command.contains("-f f32le"), "{command}");
    }

    #[test]
    fn a_ring_moves_what_it_holds_and_releases_a_blocked_producer_on_close() {
        let ring = Arc::new(SampleRing::new(2));
        assert!(ring.push(&[1.0, 2.0, 3.0, 4.0], &CancelToken::default()));
        let producer = {
            let ring = Arc::clone(&ring);
            thread::spawn(move || ring.push(&[5.0, 6.0], &CancelToken::default()))
        };
        thread::sleep(Duration::from_millis(50));
        let mut out = [0.0; 3];
        assert_eq!(ring.take(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert!(producer.join().unwrap_or(false));
        ring.close();
        assert!(!ring.push(&[7.0], &CancelToken::default()));
    }
}
