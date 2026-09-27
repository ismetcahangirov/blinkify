//! A reversed clip's sound for the preview, backwards, in bounded memory
//! (#113, ADR-0017).
//!
//! The export reverses a clip's sound with one `areverse` over the whole
//! stretch (#55), which holds all of it. The preview cannot wait for that
//! before its first sample, and must not hold a long clip. So the sound is
//! decoded [`CHUNK_SECONDS`] of source at a time, from the playback position
//! down to the clip's in point, each chunk by one process that trims,
//! resamples, runs the clip's audio chain, reverses the chunk and applies the
//! tempo — the export's order, chain before reverse, tempo after — and the
//! chunks are handed to the ring one after another. The next chunk's process
//! runs while this one's samples are played.
//!
//! - **The chain hears the sound forwards**, as in the export, with
//!   [`PRIMING_SECONDS`] of the sound before each chunk run through it first
//!   and cut off after, so a limiter or denoiser is settled at the chunk's
//!   edge rather than starting cold.
//! - **Exact length.** Each chunk is cut or padded to the samples its source
//!   span makes at the output rate and tempo, counted from the start, so
//!   rounding in `atempo` never accumulates into drift against the pictures.
//!   Padding is silence where the stream has no sound — before its first
//!   sample, which reversed is the chunk's end.
//! - **The bound** is two chunks and the ring, whatever the clip's length.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use super::decoder::tempo_stages;
use super::{AudioEnd, CHANNELS, SampleRing};
use crate::orchestrator::{
    CancelToken, Flow, Job, JobError, JobOptions, Orchestrator, Priority, SidecarCommand,
};

/// Seconds of source in one chunk. A process start per chunk costs tens of
/// milliseconds, so a chunk is long next to that; two of them in stereo
/// float at 48 kHz are well under a megabyte.
pub const CHUNK_SECONDS: f64 = 2.0;

/// Seconds of the sound before a chunk that its chain hears first.
pub const PRIMING_SECONDS: f64 = 0.5;

/// How far before a stretch the demuxer is asked to seek, as forwards.
const SEEK_MARGIN_SECONDS: f64 = 0.5;

/// What to decode backwards.
#[derive(Debug, Clone, PartialEq)]
pub struct ReverseRequest {
    pub source: PathBuf,
    /// The stream's absolute index in the file.
    pub stream: u32,
    /// Where the sound starts playing — the newest moment — in the stream's
    /// own time, in seconds.
    pub from_seconds: f64,
    /// The clip's in point, where it ends, in the same time.
    pub to_seconds: f64,
    pub sample_rate: u32,
    /// The clip's speed under the transport's.
    pub tempo: f64,
    /// The clip's audio chain, as [`chain::playable`](super::chain::playable)
    /// built it for `sample_rate`.
    pub filters: String,
}

impl ReverseRequest {
    /// The chunks, newest first, as source spans `[from, to)` in seconds.
    #[must_use]
    pub fn chunks(&self) -> Vec<(f64, f64)> {
        let mut chunks = Vec::new();
        let mut to = self.from_seconds;
        while to > self.to_seconds + 1e-9 {
            let from = (to - CHUNK_SECONDS).max(self.to_seconds);
            chunks.push((from, to));
            to = from;
        }
        chunks
    }

    /// Output frames from the start of playback down to source second `at`,
    /// at the output rate and tempo.
    fn frames_down_to(&self, at: f64) -> usize {
        let frames = ((self.from_seconds - at).max(0.0) * f64::from(self.sample_rate)
            / self.tempo.max(0.01))
        .round();
        // Bounded by the clip's length at the output rate.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = frames as usize;
        frames
    }

    /// The process that decodes the chunk `[from, to)`, reversed.
    #[must_use]
    pub fn command(&self, from: f64, to: f64) -> SidecarCommand {
        let primed = (from - PRIMING_SECONDS).max(0.0);
        let mut command = SidecarCommand::ffmpeg()
            .option("-v", "error")
            .flag("-copyts")
            .option("-seek_timestamp", "1")
            .flag("-noaccurate_seek");
        let seek = primed - SEEK_MARGIN_SECONDS;
        if seek > 0.0 {
            command = command.option("-ss", format!("{seek:.6}"));
        }
        let mut graph = format!(
            "atrim=start={primed:.6}:end={to:.6},aresample={rate},",
            rate = self.sample_rate,
        );
        if !self.filters.is_empty() {
            graph.push_str(&self.filters);
            graph.push(',');
        }
        let _ = write!(
            graph,
            "atrim=start={from:.6},aformat=sample_fmts=flt:channel_layouts=stereo,areverse"
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
}

/// One chunk's process, collecting its samples.
struct Chunk {
    from: f64,
    job: Job,
    bytes: Arc<Mutex<Vec<u8>>>,
}

/// A reversed clip's sound being decoded into a [`SampleRing`].
#[derive(Debug)]
pub struct ReverseDecoder {
    cancel: CancelToken,
    ring: Arc<SampleRing>,
    end: Arc<Mutex<Option<AudioEnd>>>,
    thread: Option<JoinHandle<()>>,
}

impl ReverseDecoder {
    /// Start decoding `request` into `ring`. Returns at once.
    #[must_use]
    pub fn start(
        orchestrator: &Orchestrator,
        request: &ReverseRequest,
        ring: Arc<SampleRing>,
    ) -> Self {
        let cancel = CancelToken::default();
        let end = Arc::new(Mutex::new(None));
        let thread = {
            let orchestrator = orchestrator.clone();
            let request = request.clone();
            let ring = Arc::clone(&ring);
            let cancel = cancel.clone();
            let end = Arc::clone(&end);
            thread::Builder::new()
                .name("preview-audio-reverse".to_owned())
                .spawn(move || {
                    let outcome = run(&orchestrator, &request, &ring, &cancel);
                    ring.finish();
                    *end.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
                })
                .ok()
        };
        Self {
            cancel,
            ring,
            end,
            thread,
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
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ReverseDecoder {
    fn drop(&mut self) {
        self.stop_in_place();
    }
}

fn start_chunk(
    orchestrator: &Orchestrator,
    request: &ReverseRequest,
    (from, to): (f64, f64),
    cancel: &CancelToken,
) -> Chunk {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    let job = orchestrator.run(
        request.command(from, to),
        Priority::Playback,
        JobOptions::default()
            .cancel_token(cancel.clone())
            .on_chunk(move |chunk| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(chunk);
                Flow::Continue
            }),
    );
    Chunk { from, job, bytes }
}

fn run(
    orchestrator: &Orchestrator,
    request: &ReverseRequest,
    ring: &SampleRing,
    cancel: &CancelToken,
) -> AudioEnd {
    let mut chunks = request.chunks().into_iter();
    let Some(first) = chunks.next() else {
        return AudioEnd::Finished;
    };
    let mut pending = start_chunk(orchestrator, request, first, cancel);
    let mut written = 0_usize;
    loop {
        let Chunk { from, job, bytes } = pending;
        match job.wait() {
            Ok(_) => {}
            Err(JobError::Cancelled | JobError::ShuttingDown) => return AudioEnd::Stopped,
            Err(error) => return AudioEnd::Failed(error.to_string()),
        }
        let bytes = std::mem::take(&mut *bytes.lock().unwrap_or_else(PoisonError::into_inner));
        // The next chunk decodes while this one plays.
        let next = chunks
            .next()
            .map(|span| start_chunk(orchestrator, request, span, cancel));
        let mut samples: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let frames = request.frames_down_to(from).saturating_sub(written);
        samples.resize(frames * CHANNELS, 0.0);
        written += frames;
        if !ring.push(&samples, cancel) {
            return AudioEnd::Stopped;
        }
        match next {
            Some(chunk) => pending = chunk,
            None => return AudioEnd::Finished,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(from: f64, to: f64, tempo: f64) -> ReverseRequest {
        ReverseRequest {
            source: PathBuf::from("clip.mkv"),
            stream: 1,
            from_seconds: from,
            to_seconds: to,
            sample_rate: 48_000,
            tempo,
            filters: String::new(),
        }
    }

    #[test]
    fn the_chunks_run_from_the_newest_moment_down_to_the_in_point() {
        assert_eq!(
            request(5.0, 0.5, 1.0).chunks(),
            vec![(3.0, 5.0), (1.0, 3.0), (0.5, 1.0)]
        );
        assert!(request(1.0, 1.0, 1.0).chunks().is_empty());
    }

    #[test]
    fn lengths_are_counted_from_the_start_so_rounding_never_drifts() {
        let request = request(3.0, 0.0, 3.0);
        // A third of a sample per chunk would drift; counted from the start
        // it cannot.
        assert_eq!(request.frames_down_to(1.0), 32_000);
        assert_eq!(request.frames_down_to(0.0), 48_000);
    }

    #[test]
    fn the_chain_hears_the_sound_forwards_and_is_primed_before_the_chunk() {
        let mut request = request(5.0, 0.0, 2.0);
        request.filters = "volume=-3dB".to_owned();
        let command = request.command(3.0, 5.0).to_string();
        assert!(
            command.contains(
                "atrim=start=2.500000:end=5.000000,aresample=48000,volume=-3dB,atrim=start=3.000000,aformat=sample_fmts=flt:channel_layouts=stereo,areverse,atempo=2"
            ),
            "{command}"
        );
        assert!(command.contains("-ss 2.000000"), "{command}");
    }
}
