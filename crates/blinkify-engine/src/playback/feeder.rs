//! The audio feeder: the thread that keeps the output buffer a little ahead of
//! the speaker.
//!
//! It walks the timeline in output-sample steps, taking each track's sound
//! from its current segment's decoder — silence for gaps and for segments
//! with no audio — passing it through the filter-chain insertion point, and
//! mixing the tracks the monitor hears. It records every jump in the timeline
//! as a clock anchor. What it guarantees:
//!
//! - **A clip boundary is sample-exact.** Chunks are cut at the boundary, the
//!   segment's decoder is dropped there and the next one — started ahead of
//!   time — takes over on the very next sample. No gap, no overlap.
//! - **The timeline position never drifts.** It is computed from the anchor
//!   and an integer count of frames written since, never accumulated.
//! - **A changed audio chain takes over without a gap** (#47, #49). When
//!   only clips' audio chains changed — a gain moved, noise reduction
//!   bypassed for an A/B comparison — the feeder keeps playing: it starts a
//!   decoder with the new chain a little ahead of what it is writing, keeps
//!   the old one going until then, and switches on the exact sample.
//! - **Audio never stalls.** A decoder that falls behind costs a moment of
//!   silence, not a stopped clock; a decoder that failed costs its segment's
//!   sound, not the playback. A gap in every track is silence, and the clock
//!   runs through it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::clock::{Anchor, PlaybackClock};
use super::monitor::{AudioInsert, MonitorSettings};
use super::plan::{PlaybackPlan, ProgramTime, Segment, TrackId, next_boundary, segment_at};
use crate::audio::denoise::Models;
use crate::audio::{AudioDecoder, AudioRequest, CHANNELS, OutputBuffer, SampleRing, chain};
use crate::orchestrator::Orchestrator;
use crate::project::evaluate::AudioOperation;

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

/// How far ahead of what is being written a retuned decoder starts: time for
/// it to produce its first samples before they are needed.
const RETUNE_AHEAD: Duration = Duration::from_millis(300);

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
    /// Which tracks the monitor hears.
    pub monitor: Arc<Mutex<MonitorSettings>>,
    /// The filter chain's insertion point.
    pub insert: Arc<Mutex<Arc<dyn AudioInsert>>>,
    /// The bundled models noise reduction runs with, if they were found.
    pub models: Option<Models>,
    /// A plan that differs from the one playing only in its audio chains,
    /// handed over while playing (see [`Feeder::retune`]).
    pub retune: Arc<Mutex<Option<Arc<PlaybackPlan>>>>,
}

/// A running feeder.
pub(crate) struct Feeder {
    stop: Arc<AtomicBool>,
    retune: Arc<Mutex<Option<Arc<PlaybackPlan>>>>,
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
        let retune = Arc::clone(&config.retune);
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("audio-feeder".to_owned())
                .spawn(move || run(&config, &stop))
                .ok()
        };
        Self {
            stop,
            retune,
            thread,
        }
    }

    /// Play `plan` from here on without stopping: it must differ from the
    /// playing plan only in its segments' audio chains. Each track whose
    /// chain changed switches to it shortly, on an exact sample.
    pub(crate) fn retune(&self, plan: Arc<PlaybackPlan>) {
        *self.retune.lock().unwrap_or_else(PoisonError::into_inner) = Some(plan);
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

fn start_lane(
    config: &FeederConfig,
    segments: &[Segment],
    segment: usize,
    at: ProgramTime,
) -> Option<Lane> {
    let rate = config.buffer.sample_rate();
    let request = audio_request(
        segments.get(segment)?,
        at,
        rate,
        config.speed,
        config.models.as_ref(),
    )?;
    let ring = Arc::new(SampleRing::new(
        LANE_SECONDS * usize::try_from(rate).unwrap_or(48_000),
    ));
    let decoder = AudioDecoder::start(&config.orchestrator, &request, Arc::clone(&ring));
    Some(Lane {
        segment,
        starts_at: at,
        ring,
        _decoder: decoder,
    })
}

/// What the preview decodes to play `segment` from timeline position `at`
/// at `sample_rate` and transport `speed`: `None` when it has no sound.
#[must_use]
pub fn audio_request(
    segment: &Segment,
    at: ProgramTime,
    sample_rate: u32,
    speed: f64,
    models: Option<&Models>,
) -> Option<AudioRequest> {
    let audio = segment.source.audio?;
    Some(AudioRequest {
        source: segment.source.path.clone(),
        stream: audio.index,
        start_seconds: segment.source_seconds_at(at),
        stream_start_seconds: audio.start_seconds,
        sample_rate,
        // The clip's own speed (#30), under the transport's.
        tempo: speed * segment.speed_factor(),
        // The clip's audio chain, exactly as the export builds it.
        filters: chain::playable(&segment.audio, sample_rate, models),
    })
}

/// One track's place: the lane playing now, and the one prepared for its next
/// boundary or for the far side of a loop.
struct Track {
    id: TrackId,
    segments: Vec<Segment>,
    current: Option<Lane>,
    upcoming: Option<Lane>,
    /// The current segment with a changed chain, from a point just ahead.
    retuned: Option<Lane>,
}

impl Track {
    fn start(&self, config: &FeederConfig, at: ProgramTime) -> Option<Lane> {
        let (i, _) = segment_at(&self.segments, at)?;
        start_lane(config, &self.segments, i, at)
    }
}

/// Where the feeder is on the timeline, and every track's decoders.
struct Walk {
    generation: u64,
    /// The timeline position of the last anchor…
    anchor_at: ProgramTime,
    /// …and the frames written since it.
    written: u64,
    tracks: Vec<Track>,
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
    let mut mix = Vec::new();
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
        let retuned = config
            .retune
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(plan) = retuned {
            retune(config, &mut walk, &plan, t);
        }
        // Chunks are cut where a retuned decoder takes over, as at a clip
        // boundary.
        let boundary = walk
            .tracks
            .iter()
            .filter_map(|track| track.retuned.as_ref().map(|lane| lane.starts_at))
            .filter(|&at| at > t)
            .fold(config.plan.next_boundary(t).min(end), ProgramTime::min);
        let frames = chunk.min(frames_covering(boundary - t, rate, config.speed));
        write_chunk(config, &mut walk, t, frames, &mut mix, &mut samples);
        prefetch(config, &mut walk, loop_range, end);
    }
}

/// Start every track's first decoder and wait for its first samples before
/// the clock starts, so the picture does not start ahead of the sound.
fn preroll(config: &FeederConfig, chunk: u64) -> Walk {
    let mut walk = Walk {
        generation: config.generation,
        anchor_at: config.start,
        written: 0,
        tracks: config
            .plan
            .sound_tracks()
            .into_iter()
            .map(|(id, segments)| Track {
                id,
                segments: segments.to_vec(),
                current: None,
                upcoming: None,
                retuned: None,
            })
            .collect(),
    };
    let deadline = Instant::now() + PREROLL;
    for track in &mut walk.tracks {
        track.current = track.start(config, config.start);
        if let Some(lane) = &track.current {
            lane.ring.wait_for(
                CHANNELS * usize::try_from(chunk).unwrap_or(1),
                deadline.saturating_duration_since(Instant::now()),
            );
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

/// Take `plan`'s audio chains: every track whose current segment's chain
/// changed gets a decoder with the new one, starting a little ahead of `t`;
/// what was prepared for later segments is dropped and prepared again.
fn retune(config: &FeederConfig, walk: &mut Walk, plan: &PlaybackPlan, t: ProgramTime) {
    let rate = config.buffer.sample_rate();
    let ahead = micros(frames_for(RETUNE_AHEAD, rate), rate, config.speed);
    let at = t.saturating_add(ahead);
    for (track, (id, segments)) in walk.tracks.iter_mut().zip(plan.sound_tracks()) {
        if track.id != id || track.segments.len() != segments.len() {
            continue;
        }
        let restart = track.current.as_ref().and_then(|lane| {
            let before = track.segments.get(lane.segment)?;
            let after = segments.get(lane.segment)?;
            (before.audio != after.audio && at < after.timeline_end()).then_some(lane.segment)
        });
        track.retuned = restart.and_then(|i| start_lane(config, segments, i, at));
        track.upcoming = None;
        track.segments = segments.to_vec();
    }
}

/// Jump back to the start of the loop: a new generation, a new anchor, and
/// every track's decoder prepared for it.
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
    for track in &mut walk.tracks {
        track.current = match track.upcoming.take() {
            Some(lane) if lane.starts_at == start => Some(lane),
            _ => track.start(config, start),
        };
    }
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

/// Write `frames` frames from `t`: every track's sound through the insertion
/// point, mixed if the monitor hears the track.
fn write_chunk(
    config: &FeederConfig,
    walk: &mut Walk,
    t: ProgramTime,
    frames: u64,
    mix: &mut Vec<f32>,
    samples: &mut Vec<f32>,
) {
    let rate = config.buffer.sample_rate();
    let count = usize::try_from(frames).unwrap_or(0) * CHANNELS;
    mix.clear();
    mix.resize(count, 0.0);
    let monitor = config
        .monitor
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let insert = Arc::clone(&config.insert.lock().unwrap_or_else(PoisonError::into_inner));
    for track in &mut walk.tracks {
        let Some((i, segment)) = segment_at(&track.segments, t) else {
            track.current = None;
            continue;
        };
        if segment.source.audio.is_none() {
            track.current = None;
            continue;
        }
        if track.current.as_ref().is_none_or(|lane| lane.segment != i) {
            track.current = match track.upcoming.take() {
                Some(lane) if lane.segment == i => Some(lane),
                _ => start_lane(config, &track.segments, i, t),
            };
        }
        take_retuned(track, i, t, count, rate, config.speed);
        samples.clear();
        samples.resize(count, 0.0);
        if let Some(lane) = &track.current {
            lane.ring.wait_for(count, DECODER_PATIENCE);
            // Whatever did not arrive in time stays silent.
            lane.ring.take(samples);
        }
        // The clip's audio chain ran in its decoder; the insert is after it,
        // and both are before the mix and the meter.
        insert.process(track.id, i, samples, rate);
        if monitor.audible(track.id) {
            for (out, sample) in mix.iter_mut().zip(samples.iter()) {
                *out += *sample;
            }
        }
    }
    config.buffer.push(mix);
    walk.written += frames;
}

/// Switch `track` to its retuned decoder once the walk reaches the point it
/// starts at and it has the samples to take over — first dropping any it
/// made for a stretch already written. Until then the old decoder plays, so
/// there is never a gap.
fn take_retuned(
    track: &mut Track,
    segment: usize,
    t: ProgramTime,
    count: usize,
    rate: u32,
    speed: f64,
) {
    let Some(lane) = &track.retuned else {
        return;
    };
    if lane.segment != segment {
        track.retuned = None;
        return;
    }
    if t < lane.starts_at {
        return;
    }
    let behind = usize::try_from(frames_covering(t - lane.starts_at, rate, speed))
        .unwrap_or(0)
        .saturating_mul(CHANNELS);
    if lane.ring.buffered() >= behind + count {
        lane.ring.discard(behind);
        track.current = track.retuned.take();
    }
}

/// Whether the preview renders `operation` itself: what the chain applies.
#[must_use]
pub fn chain_rendered(operation: &AudioOperation) -> bool {
    chain::applies(operation)
}

/// Start what plays next on each track — its following segment, or the
/// loop's start — while this plays.
fn prefetch(config: &FeederConfig, walk: &mut Walk, loop_range: LoopRange, end: ProgramTime) {
    let now = walk.position(config.buffer.sample_rate(), config.speed);
    for track in &mut walk.tracks {
        if track.upcoming.is_some() {
            continue;
        }
        let boundary = next_boundary(&track.segments, now, end).min(end);
        if let Some((start, loop_end)) = loop_range
            && loop_end - now < PREFETCH
            && boundary >= loop_end
        {
            track.upcoming = track.start(config, start);
        } else if boundary - now < PREFETCH
            && boundary < end
            && let Some((i, next)) = segment_at(&track.segments, boundary)
            && next.timeline_start == boundary
        {
            track.upcoming = start_lane(config, &track.segments, i, boundary);
        }
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
