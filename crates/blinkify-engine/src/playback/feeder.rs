//! The audio feeder: the thread that keeps the output buffer a little ahead of
//! the speaker.
//!
//! It walks the timeline in output-sample steps, taking each segment's audio
//! from that segment's decoder, silence for gaps and for segments with no
//! audio, and it records every jump in the timeline as a clock anchor. What
//! it guarantees:
//!
//! - **A clip boundary is sample-exact.** Chunks are cut at the boundary, the
//!   segment's decoder is dropped there and the next one — started ahead of
//!   time — takes over on the very next sample. No gap, no overlap.
//! - **The timeline position never drifts.** It is computed from the anchor
//!   and an integer count of frames written since, never accumulated.
//! - **Audio never stalls.** A decoder that falls behind costs a moment of
//!   silence, not a stopped clock; a decoder that failed costs its segment's
//!   sound, not the playback.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::clock::{Anchor, PlaybackClock};
use super::plan::{PlaybackPlan, ProgramTime};
use crate::audio::{AudioDecoder, AudioRequest, CHANNELS, OutputBuffer, SampleRing};
use crate::orchestrator::Orchestrator;

/// How far ahead of the speaker the buffer is kept.
const LEAD: Duration = Duration::from_millis(200);

/// The most written in one step.
const CHUNK: Duration = Duration::from_millis(10);

/// How long one step waits for a decoder before writing silence instead.
const DECODER_PATIENCE: Duration = Duration::from_millis(250);

/// How long playback waits for the first samples before starting anyway.
const PREROLL: Duration = Duration::from_secs(1);

/// How far ahead of a boundary the next segment's decoder is started.
const PREFETCH: ProgramTime = 1_500_000;

/// How much decoded audio a segment's decoder may hold.
const LANE_SECONDS: usize = 2;

/// A loop over part of the timeline, `[start, end)`.
pub(crate) type LoopRange = Option<(ProgramTime, ProgramTime)>;

pub(crate) struct FeederConfig {
    pub orchestrator: Orchestrator,
    pub plan: Arc<PlaybackPlan>,
    pub buffer: Arc<OutputBuffer>,
    pub clock: Arc<PlaybackClock>,
    pub start: ProgramTime,
    pub generation: u64,
    /// The player's generation counter; a loop takes the next number from it.
    pub generations: Arc<AtomicU64>,
    pub speed: f64,
    pub loop_range: Arc<Mutex<LoopRange>>,
    /// Set once every sample up to the end of the timeline has been heard.
    pub ended: Arc<AtomicBool>,
}

/// A running feeder.
pub(crate) struct Feeder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Feeder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Feeder").finish_non_exhaustive()
    }
}

impl Feeder {
    pub(crate) fn start(config: FeederConfig) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("audio-feeder".to_owned())
                .spawn(move || run(&config, &stop))
                .ok()
        };
        Self { stop, thread }
    }

    /// Stop writing and wait for the thread, and its decoders, to finish.
    pub(crate) fn stop(mut self) {
        self.stop_in_place();
    }

    fn stop_in_place(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop_in_place();
    }
}

/// One segment's audio, from a given timeline position.
struct Lane {
    segment: usize,
    /// The timeline position of the first sample the decoder produces.
    starts_at: ProgramTime,
    ring: Arc<SampleRing>,
    _decoder: AudioDecoder,
}

fn frames_for(duration: Duration, rate: u32) -> u64 {
    let frames = (duration.as_secs_f64() * f64::from(rate)).round();
    // A few thousand at most.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = frames as u64;
    frames.max(1)
}

/// Timeline microseconds covered by `frames` output frames.
fn micros(frames: u64, rate: u32, speed: f64) -> ProgramTime {
    #[allow(clippy::cast_precision_loss)]
    let micros = (frames as f64 * 1_000_000.0 * speed / f64::from(rate.max(1))).round();
    #[allow(clippy::cast_possible_truncation)]
    let micros = micros as i64;
    micros
}

/// Output frames needed to cover `span` timeline microseconds, at least one.
fn frames_covering(span: ProgramTime, rate: u32, speed: f64) -> u64 {
    #[allow(clippy::cast_precision_loss)]
    let frames = (span as f64 / 1_000_000.0 / speed * f64::from(rate)).ceil();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frames = frames.max(1.0) as u64;
    frames
}

fn start_lane(config: &FeederConfig, segment: usize, at: ProgramTime) -> Option<Lane> {
    let seg = config.plan.segment(segment)?;
    let audio = seg.source.audio?;
    let rate = config.buffer.sample_rate();
    let ring = Arc::new(SampleRing::new(
        LANE_SECONDS * usize::try_from(rate).unwrap_or(48_000),
    ));
    let decoder = AudioDecoder::start(
        &config.orchestrator,
        &AudioRequest {
            source: seg.source.path.clone(),
            stream: audio.index,
            start_seconds: seg.source_seconds_at(at),
            stream_start_seconds: audio.start_seconds,
            sample_rate: rate,
            tempo: config.speed,
        },
        Arc::clone(&ring),
    );
    Some(Lane {
        segment,
        starts_at: at,
        ring,
        _decoder: decoder,
    })
}

/// Where the feeder is on the timeline, and the decoders it is using.
struct Walk {
    generation: u64,
    /// The timeline position of the last anchor…
    anchor_at: ProgramTime,
    /// …and the frames written since it.
    written: u64,
    /// The lane playing now, and the one prepared for the next boundary or for
    /// the far side of a loop.
    current: Option<Lane>,
    upcoming: Option<Lane>,
}

impl Walk {
    fn position(&self, rate: u32, speed: f64) -> ProgramTime {
        self.anchor_at
            .saturating_add(micros(self.written, rate, speed))
    }
}

fn run(config: &FeederConfig, stop: &AtomicBool) {
    let rate = config.buffer.sample_rate();
    let lead = frames_for(LEAD, rate);
    let chunk = frames_for(CHUNK, rate);
    let mut walk = preroll(config, chunk);
    let mut samples = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        let t = walk.position(rate, config.speed);
        let loop_range = *config
            .loop_range
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let end = loop_range.map_or(config.plan.duration(), |(_, end)| end);
        if t >= end {
            let Some((start, _)) = loop_range else {
                wait_until_heard(config, stop);
                return;
            };
            wrap(config, &mut walk, start);
            continue;
        }
        if config.buffer.queued_frames() >= lead {
            config.buffer.wait_below(lead, Duration::from_millis(20));
            continue;
        }
        let boundary = config.plan.next_boundary(t).min(end);
        write_chunk(config, &mut walk, t, boundary, chunk, &mut samples);
        prefetch(config, &mut walk, loop_range, boundary, end);
    }
}

/// Start the first decoder and wait for its first samples before the clock
/// starts, so the picture does not start ahead of the sound.
fn preroll(config: &FeederConfig, chunk: u64) -> Walk {
    let mut walk = Walk {
        generation: config.generation,
        anchor_at: config.start,
        written: 0,
        current: None,
        upcoming: None,
    };
    if let Some((i, _)) = config.plan.segment_at(config.start) {
        walk.current = start_lane(config, i, config.start);
        if let Some(lane) = &walk.current {
            lane.ring
                .wait_for(CHANNELS * usize::try_from(chunk).unwrap_or(1), PREROLL);
        }
    }
    config.clock.run(Anchor {
        frame: config.buffer.write_index(),
        at: walk.anchor_at,
        speed: config.speed,
        generation: walk.generation,
    });
    walk
}

/// Jump back to the start of the loop: a new generation, a new anchor, and
/// the decoder prepared for it.
fn wrap(config: &FeederConfig, walk: &mut Walk, start: ProgramTime) {
    walk.generation = config.generations.fetch_add(1, Ordering::SeqCst) + 1;
    walk.anchor_at = start;
    walk.written = 0;
    config.clock.anchor(Anchor {
        frame: config.buffer.write_index(),
        at: start,
        speed: config.speed,
        generation: walk.generation,
    });
    walk.current = walk
        .upcoming
        .take()
        .filter(|lane| lane.starts_at == start)
        .or_else(|| {
            config
                .plan
                .segment_at(start)
                .and_then(|(i, _)| start_lane(config, i, start))
        });
}

/// The end of the timeline: wait until the last sample has been heard.
fn wait_until_heard(config: &FeederConfig, stop: &AtomicBool) {
    let last = config.buffer.write_index();
    while !stop.load(Ordering::SeqCst) {
        #[allow(clippy::cast_precision_loss)]
        let heard_all = config.buffer.heard(Instant::now()) >= last as f64 - 0.5;
        if heard_all {
            config.ended.store(true, Ordering::SeqCst);
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Write up to `chunk` frames from `t`, never past `boundary`.
fn write_chunk(
    config: &FeederConfig,
    walk: &mut Walk,
    t: ProgramTime,
    boundary: ProgramTime,
    chunk: u64,
    samples: &mut Vec<f32>,
) {
    let rate = config.buffer.sample_rate();
    let frames = chunk.min(frames_covering(boundary - t, rate, config.speed));
    let count = usize::try_from(frames).unwrap_or(0) * CHANNELS;
    samples.clear();
    samples.resize(count, 0.0);
    match config.plan.segment_at(t) {
        Some((i, segment)) if segment.source.audio.is_some() => {
            if walk.current.as_ref().is_none_or(|lane| lane.segment != i) {
                walk.current = walk
                    .upcoming
                    .take()
                    .filter(|lane| lane.segment == i)
                    .or_else(|| start_lane(config, i, t));
            }
            if let Some(lane) = &walk.current {
                lane.ring.wait_for(count, DECODER_PATIENCE);
                // Whatever did not arrive in time stays silent.
                lane.ring.take(samples);
            }
        }
        _ => walk.current = None,
    }
    config.buffer.push(samples);
    walk.written += frames;
}

/// Start what plays next — the following segment, or the loop's start — while
/// this plays.
fn prefetch(
    config: &FeederConfig,
    walk: &mut Walk,
    loop_range: LoopRange,
    boundary: ProgramTime,
    end: ProgramTime,
) {
    if walk.upcoming.is_some() {
        return;
    }
    let now = walk.position(config.buffer.sample_rate(), config.speed);
    if let Some((start, loop_end)) = loop_range
        && loop_end - now < PREFETCH
        && boundary >= loop_end
    {
        walk.upcoming = config
            .plan
            .segment_at(start)
            .and_then(|(i, _)| start_lane(config, i, start));
    } else if boundary - now < PREFETCH
        && boundary < end
        && let Some((i, next)) = config.plan.segment_at(boundary)
        && next.timeline_start == boundary
    {
        walk.upcoming = start_lane(config, i, boundary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_and_sample_counts_convert_both_ways() {
        assert_eq!(micros(48_000, 48_000, 1.0), 1_000_000);
        assert_eq!(micros(48_000, 48_000, 2.0), 2_000_000);
        assert_eq!(micros(441, 44_100, 1.0), 10_000);
        assert_eq!(frames_covering(1_000_000, 48_000, 1.0), 48_000);
        assert_eq!(frames_covering(1_000_000, 48_000, 0.5), 96_000);
        assert_eq!(frames_covering(1, 48_000, 1.0), 1);
    }
}
