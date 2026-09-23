//! The player: playback of a [`PlaybackPlan`] with the audio master clock and
//! the standard transport (#28).
//!
//! What it holds together:
//!
//! - **Audio is the clock.** The position is read from the samples the
//!   device has played ([`clock`]); video frames are presented against it and
//!   dropped when late. Nothing accumulates time from a timer or from
//!   `requestAnimationFrame`.
//! - **Frames are exact.** A step moves to the next or previous real frame of
//!   the source, from the keyframe index's frame table — never by a
//!   floating-point frame duration — and crosses clip boundaries onto the
//!   neighbouring clip's first or last frame.
//! - **Boundaries are seamless.** The next segment's video and audio decoders
//!   start before the boundary; audio switches on the exact sample.
//! - **Playback never waits for the device.** No device, or one that fails,
//!   is replaced by a silent sink running at real time; the clock carries on.
//!
//! The renderer asks for frames ([`Player::next_frame`]) and sends transport
//! commands ([`Player::command`]); it computes nothing about timing.

mod clock;
mod feeder;
mod lanes;
pub mod plan;
mod scrub;
pub mod timecode;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use plan::{
    AudioStream, PlanError, PlaybackPlan, ProgramTime, Segment, SourceMedia, VideoStream,
};

use crate::audio::sink::SilentSink;
use crate::audio::{AudioOutputState, BufferSlot, OutputBuffer, Sink};
use crate::decode::VideoFrame;
use crate::orchestrator::Orchestrator;
use clock::PlaybackClock;
use feeder::{Feeder, FeederConfig, LoopRange as FeederLoop};
use lanes::{LANE_BUDGET_BYTES, Lanes};
use scrub::{FrameCache, Picture, ScrubSlot, Worker};

/// How far ahead of a boundary the next segment's video is started.
const PREFETCH: ProgramTime = 1_500_000;

/// How long starting playback waits for the first frame before starting the
/// clock anyway.
const VIDEO_PREROLL: Duration = Duration::from_secs(3);

/// How often the monitor checks for the end of playback and a failed device.
const MONITOR_PERIOD: Duration = Duration::from_millis(50);

/// Bytes in front of the pixels of a frame on the wire.
pub const WIRE_HEADER_BYTES: usize = 48;

/// The magic number that opens a frame on the wire.
pub const WIRE_MAGIC: [u8; 4] = *b"BKF2";

/// The wire flag for "no picture here": a gap in the timeline, shown black.
pub const WIRE_FLAG_BLACK: u32 = 1 << 2;

/// Whether the player is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PlaybackState {
    Paused,
    Playing,
    /// Reached the end of the timeline and stopped there.
    Ended,
}

/// Preview speed. A monitoring convenience with pitch-preserving audio — not
/// the clip speed operation, which is lossless and belongs to Epic #6.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PreviewSpeed {
    Quarter,
    Half,
    Normal,
    Double,
}

impl PreviewSpeed {
    #[must_use]
    pub fn factor(self) -> f64 {
        match self {
            Self::Quarter => 0.25,
            Self::Half => 0.5,
            Self::Normal => 1.0,
            Self::Double => 2.0,
        }
    }
}

/// A timeline range played over and over, `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoopRange {
    #[ts(type = "number")]
    pub start: ProgramTime,
    #[ts(type = "number")]
    pub end: ProgramTime,
}

/// A transport action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum TransportCommand {
    Play,
    Pause,
    Toggle,
    /// Pause and return to the start.
    Stop,
    /// Pause and move by whole frames: positive forwards.
    Step {
        frames: i32,
    },
    JumpToStart,
    JumpToEnd,
    /// Move to a timeline position, in microseconds, and show exactly the
    /// frame there.
    Seek {
        #[ts(type = "number")]
        position: ProgramTime,
    },
    /// Follow a dragged playhead: pause, and show the frame at the latest of
    /// a stream of positions as fast as it can be decoded, skipping any that
    /// a newer one overtakes. End a drag with `Seek`.
    Scrub {
        #[ts(type = "number")]
        position: ProgramTime,
    },
    SetSpeed {
        speed: PreviewSpeed,
    },
    SetLoop {
        range: Option<LoopRange>,
    },
}

/// Where the player is, as the renderer shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaybackStatus {
    pub state: PlaybackState,
    /// The timeline position being heard, in microseconds. Paused, it is the
    /// start of the frame on screen.
    #[ts(type = "number")]
    pub position: ProgramTime,
    #[ts(type = "number")]
    pub duration: ProgramTime,
    /// The rate timecode counts frames in.
    pub frame_rate: crate::probe::Rational,
    /// The frame at `position`, `HH:MM:SS:FF`.
    pub timecode: String,
    pub duration_timecode: String,
    pub speed: PreviewSpeed,
    pub loop_range: Option<LoopRange>,
    pub audio: AudioOutputState,
    /// A seek or a scrub has not yet put its frame on screen. On a source
    /// with far-apart keyframes that takes a moment, and the interface says
    /// so rather than showing a frozen frame as if it were the answer.
    pub resolving: bool,
    /// The picture at `position` comes from a preview proxy, not the file.
    /// The interface must say so, persistently, whenever it does (#26).
    pub proxy: bool,
}

/// Decode statistics, for diagnosis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecodeStats {
    pub buffered_frames: u32,
    pub capacity_frames: u32,
    #[ts(type = "number")]
    pub buffered_bytes: u64,
    #[ts(type = "number")]
    pub decoded_frames: u64,
    /// Frames that were due but superseded before they could be shown.
    #[ts(type = "number")]
    pub dropped_frames: u64,
    #[ts(type = "number")]
    pub presented_frames: u64,
    pub decode_fps: f64,
    /// Times a decoder was restarted ahead of a clock it could not keep up
    /// with.
    pub resyncs: u32,
    pub error: Option<String>,
}

/// Where the player's audio goes.
#[derive(Debug, Clone)]
pub enum AudioChoice {
    /// The default output device, or silence if there is none.
    Device,
    /// Silence at real time.
    Silent,
    /// Silence at real time, keeping every sample played — for tests and
    /// diagnosis.
    Capture {
        sample_rate: u32,
        samples: Arc<Mutex<Vec<f32>>>,
    },
}

/// How a player is set up.
#[derive(Debug, Clone)]
pub struct PlayerOptions {
    pub audio: AudioChoice,
    /// The largest frame the renderer can show, in device pixels; zero for
    /// no bound.
    pub max_width: u32,
    pub max_height: u32,
}

/// A frame on screen.
#[derive(Debug)]
pub struct ShownFrame {
    /// Increases by one for every frame presented.
    pub seq: u64,
    /// `None` in a gap, which is shown black.
    pub picture: Option<Arc<VideoFrame>>,
    /// Counter-clockwise rotation to apply when drawing.
    pub rotation: u32,
    /// Where this frame starts on the timeline.
    pub position: ProgramTime,
    /// The clock when this frame was chosen as the one due. `chosen_at -
    /// position` is how far the picture trailed the sound at that moment:
    /// the A/V sync, measured where it is decided.
    pub chosen_at: ProgramTime,
    /// The timeline frame number at the plan's rate.
    pub frame_number: i64,
}

impl ShownFrame {
    /// The frame as the renderer receives it: a 48-byte little-endian header,
    /// then the RGBA pixels.
    ///
    /// | offset | size | field                                          |
    /// | ------ | ---- | ---------------------------------------------- |
    /// | 0      | 4    | magic `BKF2`                                   |
    /// | 4      | 4    | width, `u32` (0 for a black frame)             |
    /// | 8      | 4    | height, `u32`                                  |
    /// | 12     | 4    | flags: bits 0–1 quarter turns, bit 2 black     |
    /// | 16     | 8    | sequence number, `u64`                         |
    /// | 24     | 8    | source presentation timestamp, `i64`           |
    /// | 32     | 8    | timeline position, microseconds, `i64`         |
    /// | 40     | 8    | timeline frame number, `i64`                   |
    #[must_use]
    pub fn wire(&self) -> Vec<u8> {
        let (width, height, pts, pixels): (u32, u32, i64, &[u8]) = match &self.picture {
            Some(frame) => (frame.width, frame.height, frame.pts, &frame.pixels),
            None => (0, 0, 0, &[]),
        };
        let quarter_turns = self.rotation.div_euclid(90) % 4;
        let flags = quarter_turns
            | if self.picture.is_none() {
                WIRE_FLAG_BLACK
            } else {
                0
            };
        let mut bytes = Vec::with_capacity(WIRE_HEADER_BYTES + pixels.len());
        bytes.extend_from_slice(&WIRE_MAGIC);
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&self.seq.to_le_bytes());
        bytes.extend_from_slice(&pts.to_le_bytes());
        bytes.extend_from_slice(&self.position.to_le_bytes());
        bytes.extend_from_slice(&self.frame_number.to_le_bytes());
        bytes.extend_from_slice(pixels);
        bytes
    }
}

type Listener = Arc<dyn Fn(PlaybackStatus) + Send + Sync>;

struct Control {
    state: PlaybackState,
    speed: PreviewSpeed,
    feeder: Option<Feeder>,
}

#[derive(Default)]
struct Shown {
    seq: u64,
    frame: Option<Arc<ShownFrame>>,
}

struct Inner {
    orchestrator: Orchestrator,
    plan: Arc<PlaybackPlan>,
    slot: BufferSlot,
    buffer: Mutex<Arc<OutputBuffer>>,
    sink: Mutex<Option<Sink>>,
    clock: Arc<PlaybackClock>,
    generations: Arc<AtomicU64>,
    loop_range: Arc<Mutex<FeederLoop>>,
    ended: Arc<AtomicBool>,
    control: Mutex<Control>,
    lanes: Mutex<Lanes>,
    shown: Mutex<Shown>,
    listener: Mutex<Option<Listener>>,
    closed: AtomicBool,
    /// Scrubbing: the worker, not the lanes, decides what is on screen.
    scrubbing: AtomicBool,
    scrub: Arc<ScrubSlot>,
    cache: Arc<Mutex<FrameCache>>,
    /// The position whose frame a seek or a scrub is still waiting for.
    pending: Mutex<Option<ProgramTime>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A player for one plan.
pub struct Player {
    inner: Arc<Inner>,
    monitor: Mutex<Option<JoinHandle<()>>>,
    scrubber: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player").finish_non_exhaustive()
    }
}

impl Player {
    /// A paused player at the start of `plan`, with its first frame being
    /// decoded.
    #[must_use]
    pub fn new(orchestrator: Orchestrator, plan: PlaybackPlan, options: &PlayerOptions) -> Self {
        let slot: BufferSlot = Arc::new(Mutex::new(None));
        let sink = open_sink(&slot, &options.audio);
        let buffer = Arc::new(OutputBuffer::new(sink.sample_rate()));
        *lock(&slot) = Some(Arc::clone(&buffer));
        let start = plan.segments().first().map_or(0, |s| s.timeline_start);
        let inner = Arc::new(Inner {
            lanes: Mutex::new(Lanes::new(
                orchestrator.clone(),
                options.max_width,
                options.max_height,
            )),
            orchestrator,
            plan: Arc::new(plan),
            slot,
            buffer: Mutex::new(buffer),
            sink: Mutex::new(Some(sink)),
            clock: Arc::new(PlaybackClock::new(start, 1)),
            generations: Arc::new(AtomicU64::new(1)),
            loop_range: Arc::new(Mutex::new(None)),
            ended: Arc::new(AtomicBool::new(false)),
            control: Mutex::new(Control {
                state: PlaybackState::Paused,
                speed: PreviewSpeed::Normal,
                feeder: None,
            }),
            shown: Mutex::new(Shown::default()),
            listener: Mutex::new(None),
            closed: AtomicBool::new(false),
            scrubbing: AtomicBool::new(false),
            scrub: Arc::new(ScrubSlot::default()),
            cache: Arc::new(Mutex::new(FrameCache::new(LANE_BUDGET_BYTES * 2))),
            pending: Mutex::new(None),
        });
        inner.prepare(1, start);
        let scrubber = {
            let inner = Arc::clone(&inner);
            thread::Builder::new()
                .name("playback-scrub".to_owned())
                .spawn(move || inner.scrub_worker())
                .ok()
        };
        let monitor = {
            let inner = Arc::clone(&inner);
            thread::Builder::new()
                .name("playback-monitor".to_owned())
                .spawn(move || inner.monitor())
                .ok()
        };
        Self {
            inner,
            monitor: Mutex::new(monitor),
            scrubber: Mutex::new(scrubber),
        }
    }

    #[must_use]
    pub fn plan(&self) -> &PlaybackPlan {
        &self.inner.plan
    }

    /// Call `listener` with the new status whenever the state, the speed,
    /// the loop or the audio output changes — not on every frame.
    pub fn set_listener(&self, listener: impl Fn(PlaybackStatus) + Send + Sync + 'static) {
        *lock(&self.inner.listener) = Some(Arc::new(listener));
    }

    /// Carry out a transport command and return the status after it.
    pub fn command(&self, command: TransportCommand) -> PlaybackStatus {
        self.inner.command(command);
        let status = self.status();
        self.inner.notify(&status);
        status
    }

    #[must_use]
    pub fn status(&self) -> PlaybackStatus {
        self.inner.status()
    }

    /// The generation and timeline position being heard now.
    #[must_use]
    pub fn position(&self) -> (u64, ProgramTime) {
        self.inner.position()
    }

    /// The frame to show now, if it is newer than `after`, waiting up to
    /// `timeout` for one.
    pub fn next_frame(&self, after: u64, timeout: Duration) -> Option<Arc<ShownFrame>> {
        self.inner.next_frame(after, timeout)
    }

    /// Replace the audio output — a device change — without stopping.
    pub fn switch_audio(&self, choice: &AudioChoice) {
        self.inner.switch_audio(choice);
        let status = self.status();
        self.inner.notify(&status);
    }

    #[must_use]
    pub fn stats(&self) -> DecodeStats {
        self.inner.stats()
    }

    /// The running video decoder processes.
    #[must_use]
    pub fn decoder_pids(&self) -> Vec<u32> {
        lock(&self.inner.lanes).pids()
    }

    /// Scrub positions asked for, and windows decoded to serve them. The
    /// second is the smaller when positions were coalesced.
    #[must_use]
    pub fn scrub_counts(&self) -> (u64, u64) {
        self.inner.scrub.counts()
    }

    /// Bytes held by the scrub cache, and its bound.
    #[must_use]
    pub fn scrub_cache_bytes(&self) -> (usize, usize) {
        let cache = lock(&self.inner.cache);
        (cache.bytes(), cache.budget())
    }

    /// Stop everything. Returns once every decoder process has exited.
    pub fn close(&self) {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(feeder) = lock(&self.inner.control).feeder.take() {
            feeder.stop();
        }
        self.inner.scrub.stop();
        if let Some(scrubber) = lock(&self.scrubber).take() {
            let _ = scrubber.join();
        }
        lock(&self.inner.lanes).clear();
        *lock(&self.inner.sink) = None;
        if let Some(monitor) = lock(&self.monitor).take() {
            let _ = monitor.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.close();
    }
}

fn open_sink(slot: &BufferSlot, choice: &AudioChoice) -> Sink {
    match choice {
        AudioChoice::Device => Sink::open(slot, true),
        AudioChoice::Silent => Sink::open(slot, false),
        AudioChoice::Capture {
            sample_rate,
            samples,
        } => Sink::Silent(
            SilentSink::capturing(Arc::clone(slot), *sample_rate, Arc::clone(samples)),
            *sample_rate,
            "capturing".to_owned(),
        ),
    }
}

impl Inner {
    fn buffer(&self) -> Arc<OutputBuffer> {
        Arc::clone(&lock(&self.buffer))
    }

    fn position(&self) -> (u64, ProgramTime) {
        let buffer = self.buffer();
        let heard = buffer.heard(Instant::now());
        self.clock.position(heard, buffer.sample_rate())
    }

    fn notify(&self, status: &PlaybackStatus) {
        let listener = lock(&self.listener).clone();
        if let Some(listener) = listener {
            listener(status.clone());
        }
    }

    fn end(&self) -> ProgramTime {
        self.plan.duration()
    }

    /// Start the lane that will show `t` in `generation`.
    fn prepare(&self, generation: u64, t: ProgramTime) {
        if let Some((i, segment)) = self.plan.segment_at(t) {
            let pts = segment.source_at(t);
            let _ = lock(&self.lanes).lane(&self.plan, i, generation, pts);
        }
    }

    fn command(&self, command: TransportCommand) {
        let mut control = lock(&self.control);
        match command {
            TransportCommand::Scrub { .. }
            | TransportCommand::SetSpeed { .. }
            | TransportCommand::SetLoop { .. } => {}
            // These start from wherever the scrub left the clock.
            TransportCommand::Play | TransportCommand::Pause | TransportCommand::Toggle => {
                self.end_scrub(&mut control, true);
            }
            // These go somewhere of their own.
            _ => self.end_scrub(&mut control, false),
        }
        match command {
            TransportCommand::Play => self.play(&mut control),
            TransportCommand::Pause => self.pause(&mut control),
            TransportCommand::Toggle => {
                if control.state == PlaybackState::Playing {
                    self.pause(&mut control);
                } else {
                    self.play(&mut control);
                }
            }
            TransportCommand::Stop => {
                self.pause(&mut control);
                self.seek(&mut control, self.start_position());
            }
            TransportCommand::Step { frames } => self.step(&mut control, frames),
            TransportCommand::JumpToStart => self.seek(&mut control, self.start_position()),
            TransportCommand::JumpToEnd => {
                let last = self.last_frame_position();
                self.seek(&mut control, last);
            }
            TransportCommand::Seek { position } => self.seek(&mut control, position),
            TransportCommand::Scrub { position } => {
                self.pause(&mut control);
                if control.state == PlaybackState::Ended {
                    control.state = PlaybackState::Paused;
                }
                let t = position.clamp(0, self.end().saturating_sub(1).max(0));
                let (generation, _) = self.position();
                self.scrubbing.store(true, Ordering::SeqCst);
                self.clock.pause(t, generation);
                *lock(&self.pending) = Some(t);
                self.scrub.request(t);
            }
            TransportCommand::SetSpeed { speed } => {
                control.speed = speed;
                if control.state == PlaybackState::Playing {
                    let (generation, t) = self.position();
                    self.stop_feeder(&mut control);
                    self.clock.pause(t, generation);
                    self.start_feeder(&mut control, t, generation);
                }
            }
            TransportCommand::SetLoop { range } => {
                let range = range
                    .filter(|r| r.start < r.end)
                    .map(|r| LoopRange {
                        start: r.start.clamp(0, self.end()),
                        end: r.end.clamp(0, self.end()),
                    })
                    .filter(|r| r.start < r.end);
                *lock(&self.loop_range) = range.map(|r| (r.start, r.end));
            }
        }
    }

    fn start_position(&self) -> ProgramTime {
        lock(&self.loop_range)
            .map(|(start, _)| start)
            .or_else(|| self.plan.segments().first().map(|s| s.timeline_start))
            .unwrap_or(0)
    }

    /// Where the last frame of the timeline starts.
    fn last_frame_position(&self) -> ProgramTime {
        let end = self.end();
        let Some((_, segment)) = self.plan.segment_at(end.saturating_sub(1)) else {
            return end.saturating_sub(1).max(0);
        };
        let Some(video) = segment.source.video.as_ref() else {
            return end.saturating_sub(1).max(0);
        };
        segment
            .source
            .index
            .frame_before(video.index, segment.source_out)
            .ok()
            .flatten()
            .map_or(end.saturating_sub(1), |pts| segment.program_at(pts))
    }

    fn play(&self, control: &mut Control) {
        if control.state == PlaybackState::Playing {
            return;
        }
        let (mut generation, mut t) = self.position();
        let range_end = lock(&self.loop_range).map_or(self.end(), |(_, end)| end);
        if control.state == PlaybackState::Ended || t >= range_end {
            self.seek(control, self.start_position());
            (generation, t) = self.position();
        }
        // Pre-roll: the clock starts once the first frame is decoded, so a
        // slow start costs a moment of waiting, not the first frames.
        self.wait_for_video(generation, t);
        self.start_feeder(control, t, generation);
        control.state = PlaybackState::Playing;
    }

    fn wait_for_video(&self, generation: u64, t: ProgramTime) {
        let Some((i, segment)) = self.plan.segment_at(t) else {
            return;
        };
        let ring = lock(&self.lanes)
            .lane(&self.plan, i, generation, segment.source_at(t))
            .map(|lane| Arc::clone(&lane.ring));
        if let Some(ring) = ring {
            ring.wait_for_frame(VIDEO_PREROLL);
        }
    }

    fn pause(&self, control: &mut Control) {
        if control.state != PlaybackState::Playing {
            return;
        }
        let (generation, t) = self.position();
        self.stop_feeder(control);
        self.clock.pause(t, generation);
        control.state = PlaybackState::Paused;
    }

    fn start_feeder(&self, control: &mut Control, t: ProgramTime, generation: u64) {
        let buffer = self.buffer();
        buffer.clear();
        self.ended.store(false, Ordering::SeqCst);
        control.feeder = Some(Feeder::start(FeederConfig {
            orchestrator: self.orchestrator.clone(),
            plan: Arc::clone(&self.plan),
            buffer,
            clock: Arc::clone(&self.clock),
            start: t,
            generation,
            generations: Arc::clone(&self.generations),
            speed: control.speed.factor(),
            loop_range: Arc::clone(&self.loop_range),
            ended: Arc::clone(&self.ended),
        }));
    }

    fn stop_feeder(&self, control: &mut Control) {
        if let Some(feeder) = control.feeder.take() {
            feeder.stop();
        }
        self.buffer().clear();
    }

    /// Leave scrubbing: the lanes take over what is on screen again —
    /// restarted exactly where the scrub left the clock, if `at_clock`.
    fn end_scrub(&self, control: &mut Control, at_clock: bool) {
        if self.scrubbing.swap(false, Ordering::SeqCst) && at_clock {
            let (_, t) = self.position();
            self.seek(control, t);
        }
    }

    fn seek(&self, control: &mut Control, t: ProgramTime) {
        let t = t.clamp(0, self.end().saturating_sub(1).max(0));
        *lock(&self.pending) = Some(t);
        let playing = control.state == PlaybackState::Playing;
        if playing {
            self.stop_feeder(control);
        }
        let generation = self.generations.fetch_add(1, Ordering::SeqCst) + 1;
        self.clock.pause(t, generation);
        lock(&self.lanes).clear();
        self.prepare(generation, t);
        if playing {
            self.start_feeder(control, t, generation);
        } else {
            control.state = PlaybackState::Paused;
        }
    }

    /// Pause, then move `frames` real frames of the sources from the frame on
    /// screen.
    fn step(&self, control: &mut Control, frames: i32) {
        self.pause(control);
        if control.state == PlaybackState::Ended {
            control.state = PlaybackState::Paused;
        }
        let (generation, t) = self.position();
        let Some(mut at) = self.frame_at(t) else {
            return;
        };
        for _ in 0..frames.unsigned_abs() {
            let next = if frames > 0 {
                self.frame_after(at)
            } else {
                self.frame_before(at)
            };
            match next {
                Some(next) => at = next,
                None => break,
            }
        }
        let (segment, pts) = at;
        let Some(target) = self.plan.segment(segment).map(|s| s.program_at(pts)) else {
            return;
        };
        // A frame the lane already holds: just move the clock to it.
        let reusable = {
            let mut lanes = lock(&self.lanes);
            lanes
                .lane(&self.plan, segment, generation, pts)
                .is_some_and(|lane| {
                    lane.ring.earliest_pts().is_some_and(|first| first <= pts)
                        && lane.ring.latest_pts().is_some_and(|last| last >= pts)
                })
        };
        if reusable {
            *lock(&self.pending) = Some(target);
            self.clock.pause(target, generation);
        } else {
            self.seek(control, target);
        }
    }

    /// The segment and source timestamp of the frame due at `t` — from the
    /// clock, not from what the renderer last fetched, which can lag a step.
    fn frame_at(&self, t: ProgramTime) -> Option<(usize, i64)> {
        let (i, segment) = self
            .plan
            .segment_at(t)
            .or_else(|| self.plan.next_segment_after(t))?;
        let video = segment.source.video.as_ref()?;
        let pts = segment
            .source
            .index
            .frame_at_or_before(
                video.index,
                segment.source_at(t.max(segment.timeline_start)),
            )
            .ok()
            .flatten()?;
        Some((i, pts))
    }

    /// The first frame of segment `i`: the one on screen at its in point.
    fn first_frame(&self, i: usize) -> Option<i64> {
        let segment = self.plan.segment(i)?;
        let video = segment.source.video.as_ref()?;
        let index = &segment.source.index;
        index
            .frame_at_or_before(video.index, segment.source_in)
            .ok()
            .flatten()
            .or_else(|| {
                index
                    .frame_after(video.index, segment.source_in)
                    .ok()
                    .flatten()
            })
    }

    fn frame_after(&self, (i, pts): (usize, i64)) -> Option<(usize, i64)> {
        let segment = self.plan.segment(i)?;
        let video = segment.source.video.as_ref()?;
        if let Ok(Some(next)) = segment.source.index.frame_after(video.index, pts)
            && next < segment.source_out
        {
            return Some((i, next));
        }
        // The first frame of the next segment that has pictures.
        (i + 1..self.plan.segments().len()).find_map(|k| self.first_frame(k).map(|pts| (k, pts)))
    }

    fn frame_before(&self, (i, pts): (usize, i64)) -> Option<(usize, i64)> {
        let segment = self.plan.segment(i)?;
        let video = segment.source.video.as_ref()?;
        let first = self.first_frame(i)?;
        if pts > first
            && let Ok(Some(previous)) = segment.source.index.frame_before(video.index, pts)
        {
            return Some((i, previous.max(first)));
        }
        // The last frame of the previous segment that has pictures.
        (0..i).rev().find_map(|k| {
            let segment = self.plan.segment(k)?;
            let video = segment.source.video.as_ref()?;
            let last = segment
                .source
                .index
                .frame_before(video.index, segment.source_out)
                .ok()
                .flatten()?;
            Some((k, last))
        })
    }

    fn next_frame(&self, after: u64, timeout: Duration) -> Option<Arc<ShownFrame>> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            let (generation, t) = self.position();
            self.present(generation, t);
            {
                let shown = lock(&self.shown);
                if shown.seq > after
                    && let Some(frame) = &shown.frame
                {
                    return Some(Arc::clone(frame));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            thread::sleep(left.min(Duration::from_millis(2)));
        }
    }

    /// Put the frame due at `t` on screen, and start what plays next.
    fn present(&self, generation: u64, t: ProgramTime) {
        if self.scrubbing.load(Ordering::SeqCst) {
            // The scrub worker owns the screen.
            return;
        }
        if t >= self.end() {
            // Past the end the last frame stays up.
            return;
        }
        let Some((i, segment)) = self.plan.segment_at(t) else {
            self.show_black(t);
            return;
        };
        if segment.source.video.is_none() {
            self.show_black(t);
            return;
        }
        // Never a frame from past the out point, even though the decoder
        // runs on beyond it.
        let pts = segment
            .source_at(t)
            .min(segment.source_out.saturating_sub(1));
        let mut lanes = lock(&self.lanes);
        lanes.keep_up(&self.plan, i, generation, pts);
        let frame = lanes
            .lane(&self.plan, i, generation, pts)
            .and_then(|lane| lane.ring.take_due(pts));
        // What plays next: the following segment, and the far side of a loop.
        if segment.timeline_end() - t < PREFETCH
            && let Some((k, next)) = self.plan.segment_at(segment.timeline_end())
        {
            let _ = lanes.lane(&self.plan, k, generation, next.source_in);
        }
        if let Some((start, end)) = *lock(&self.loop_range)
            && end - t < PREFETCH
            && t < end
            && let Some((k, looped)) = self.plan.segment_at(start)
        {
            let _ = lanes.lane(&self.plan, k, generation + 1, looped.source_at(start));
        }
        lanes.retain(generation, Some(i));
        drop(lanes);

        if let Some(frame) = frame {
            self.show_picture(i, Arc::new(frame), lanes::rotation_of(segment), t);
            // Any frame a lane presents answers the seek that started it.
            self.resolve(None);
        }
    }

    /// Put `frame` of segment `i` on screen, chosen at clock position `t`.
    fn show_picture(&self, i: usize, frame: Arc<VideoFrame>, rotation: u32, t: ProgramTime) {
        let Some(segment) = self.plan.segment(i) else {
            return;
        };
        let position = segment.program_at(frame.pts);
        let mut shown = lock(&self.shown);
        shown.seq += 1;
        shown.frame = Some(Arc::new(ShownFrame {
            seq: shown.seq,
            rotation,
            position,
            chosen_at: t,
            frame_number: timecode::frame_number(position, self.plan.frame_rate),
            picture: Some(frame),
        }));
    }

    /// The frame for a pending seek or scrub is up: `for_target` is the
    /// position it was asked for, or `None` for "whatever was pending".
    fn resolve(&self, for_target: Option<ProgramTime>) {
        let mut pending = lock(&self.pending);
        let answered = match (*pending, for_target) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(waiting), Some(target)) => waiting == target,
        };
        if answered {
            *pending = None;
            drop(pending);
            let status = self.status();
            self.notify(&status);
        }
    }

    /// Serve scrub positions until the player closes.
    fn scrub_worker(self: Arc<Self>) {
        let bound = lock(&self.lanes).bound;
        let worker = Worker {
            slot: Arc::clone(&self.scrub),
            orchestrator: self.orchestrator.clone(),
            plan: Arc::clone(&self.plan),
            bound,
            cache: Arc::clone(&self.cache),
        };
        worker.run(&|picture, target| {
            if !self.scrubbing.load(Ordering::SeqCst) {
                return;
            }
            match picture {
                Picture::Frame {
                    segment,
                    frame,
                    rotation,
                } => self.show_picture(segment, frame, rotation, target),
                Picture::Black => self.show_black(target),
            }
            if self.scrub.latest() == Some(target) {
                self.resolve(Some(target));
            }
        });
    }

    fn show_black(&self, t: ProgramTime) {
        let mut shown = lock(&self.shown);
        if shown
            .frame
            .as_ref()
            .is_some_and(|frame| frame.picture.is_none())
        {
            return;
        }
        shown.seq += 1;
        shown.frame = Some(Arc::new(ShownFrame {
            seq: shown.seq,
            picture: None,
            rotation: 0,
            position: t,
            chosen_at: t,
            frame_number: timecode::frame_number(t, self.plan.frame_rate),
        }));
    }

    fn status(&self) -> PlaybackStatus {
        let control = lock(&self.control);
        let (_, t) = self.position();
        let position = t.clamp(0, self.end());
        let frame_rate = self.plan.frame_rate;
        PlaybackStatus {
            state: control.state,
            position,
            duration: self.end(),
            frame_rate,
            timecode: timecode::format(timecode::frame_number(position, frame_rate), frame_rate),
            duration_timecode: timecode::format(
                timecode::frame_number(self.end(), frame_rate),
                frame_rate,
            ),
            speed: control.speed,
            loop_range: lock(&self.loop_range).map(|(start, end)| LoopRange { start, end }),
            resolving: lock(&self.pending).is_some(),
            proxy: self
                .plan
                .segment_at(position.min(self.end().saturating_sub(1)))
                .is_some_and(|(_, segment)| segment.source.proxy.is_some()),
            audio: lock(&self.sink).as_ref().map_or(
                AudioOutputState::Silent {
                    reason: "closed".to_owned(),
                },
                Sink::state,
            ),
        }
    }

    fn stats(&self) -> DecodeStats {
        let (generation, t) = self.position();
        let segment = self.plan.segment_at(t).map(|(i, _)| i);
        let lanes = lock(&self.lanes);
        let (current, decoded, dropped, presented) = lanes.stats(segment, generation);
        let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        DecodeStats {
            buffered_frames: to_u32(current.buffered_frames),
            capacity_frames: to_u32(current.capacity_frames),
            buffered_bytes: u64::try_from(current.buffered_bytes).unwrap_or(u64::MAX),
            decoded_frames: decoded,
            dropped_frames: dropped,
            presented_frames: presented,
            decode_fps: current.decode_fps,
            resyncs: lanes.resyncs,
            error: lanes.error.as_ref().map(ToString::to_string),
        }
    }

    fn switch_audio(&self, choice: &AudioChoice) {
        let mut control = lock(&self.control);
        let playing = control.state == PlaybackState::Playing;
        let (generation, t) = self.position();
        if playing {
            self.stop_feeder(&mut control);
            self.clock.pause(t, generation);
        }
        // Close the old output before opening the new one: a device that
        // failed may still hold the endpoint.
        *lock(&self.sink) = None;
        let sink = open_sink(&self.slot, choice);
        if sink.sample_rate() != self.buffer().sample_rate() {
            let buffer = Arc::new(OutputBuffer::new(sink.sample_rate()));
            *lock(&self.slot) = Some(Arc::clone(&buffer));
            *lock(&self.buffer) = buffer;
        }
        *lock(&self.sink) = Some(sink);
        if playing {
            self.start_feeder(&mut control, t, generation);
        }
    }

    /// The end of playback, and a failed device, handled off the feeder's
    /// thread so that every state change goes through one lock.
    fn monitor(self: Arc<Self>) {
        while !self.closed.load(Ordering::SeqCst) {
            thread::sleep(MONITOR_PERIOD);
            if self.ended.swap(false, Ordering::SeqCst) {
                let mut control = lock(&self.control);
                if control.state == PlaybackState::Playing {
                    let (generation, _) = self.position();
                    if let Some(feeder) = control.feeder.take() {
                        feeder.stop();
                    }
                    self.clock.pause(self.end(), generation);
                    control.state = PlaybackState::Ended;
                    drop(control);
                    let status = self.status();
                    self.notify(&status);
                }
            }
            let failed = lock(&self.sink).as_ref().is_some_and(Sink::has_failed);
            if failed {
                self.switch_audio(&AudioChoice::Device);
                let status = self.status();
                self.notify(&status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_commands_have_a_stable_wire_shape() {
        let json = |command: TransportCommand| serde_json::to_string(&command).unwrap_or_default();
        assert_eq!(json(TransportCommand::Toggle), r#"{"type":"toggle"}"#);
        assert_eq!(
            json(TransportCommand::Step { frames: -1 }),
            r#"{"type":"step","frames":-1}"#
        );
        assert_eq!(
            json(TransportCommand::SetSpeed {
                speed: PreviewSpeed::Half
            }),
            r#"{"type":"set-speed","speed":"half"}"#
        );
        assert_eq!(
            json(TransportCommand::SetLoop {
                range: Some(LoopRange { start: 1, end: 2 })
            }),
            r#"{"type":"set-loop","range":{"start":1,"end":2}}"#
        );
    }

    #[test]
    fn a_black_frame_on_the_wire_has_no_pixels_and_says_so() {
        let frame = ShownFrame {
            seq: 3,
            picture: None,
            rotation: 0,
            position: 1_000_000,
            chosen_at: 1_000_000,
            frame_number: 30,
        };
        let bytes = frame.wire();
        assert_eq!(bytes.len(), WIRE_HEADER_BYTES);
        assert_eq!(bytes.get(..4), Some(&WIRE_MAGIC[..]));
        assert_eq!(bytes.get(12..16), Some(&WIRE_FLAG_BLACK.to_le_bytes()[..]));
        assert_eq!(bytes.get(32..40), Some(&1_000_000_i64.to_le_bytes()[..]));
    }

    #[test]
    fn rotation_travels_as_quarter_turns() {
        let frame = ShownFrame {
            seq: 1,
            picture: Some(Arc::new(VideoFrame {
                pts: 7,
                width: 1,
                height: 1,
                pixels: vec![1, 2, 3, 4],
            })),
            rotation: 270,
            position: 0,
            chosen_at: 0,
            frame_number: 0,
        };
        let bytes = frame.wire();
        assert_eq!(bytes.get(12..16), Some(&3_u32.to_le_bytes()[..]));
        assert_eq!(bytes.get(48..), Some(&[1_u8, 2, 3, 4][..]));
    }
}
