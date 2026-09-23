//! A preview of one source: a decoder, its ring, a clock, and the frame that
//! is on screen.
//!
//! The session is what the renderer's frame requests are answered from. It
//! decides which frame is due — the renderer never computes that — and it
//! holds the rules that keep playback honest under load:
//!
//! - **Present by timestamp.** The frame shown is the newest one due at the
//!   clock's position; older frames are dropped and counted, never shown late.
//! - **Never fall behind.** If the decoder cannot keep up and the clock runs
//!   more than [`RESYNC_AFTER`] past the newest decoded frame, decoding
//!   restarts at the next keyframe ahead of the clock rather than grinding
//!   through frames nobody will see.
//! - **Pre-roll.** The clock starts when the first frame is ready, not when
//!   the decoder is asked for it, so a slow start drops nothing.
//! - **Close means gone.** [`PreviewSession::close`] returns once the decoder
//!   process has exited.

mod clock;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use clock::WallClock;

use crate::decode::{
    DecodeEnd, DecodeError, DecodeRequest, FrameRing, FrameSize, VideoDecoder, VideoFrame,
};
use crate::keyframes::KeyframeIndex;
use crate::orchestrator::Orchestrator;
use crate::probe::{MediaInfo, Rational, StreamInfo, VideoInfo};

/// How far the clock may run ahead of the newest decoded frame before the
/// decoder is restarted ahead of it.
const RESYNC_AFTER: Duration = Duration::from_secs(1);

/// The least time between two restarts, so a machine that cannot decode in
/// real time drops frames steadily instead of thrashing processes.
const RESYNC_INTERVAL: Duration = Duration::from_secs(2);

/// How far ahead of the clock a resynchronised decoder aims.
const RESYNC_LEAD: Duration = Duration::from_millis(300);

/// How long pre-roll waits for the first frame before starting the clock
/// anyway.
const PREROLL_WAIT: Duration = Duration::from_secs(3);

/// Memory the frame ring may hold. At 1080p that is fifteen frames.
pub const RING_BUDGET_BYTES: usize = 128 * 1024 * 1024;

/// What the renderer needs to draw a session's frames.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewInfo {
    /// The size frames arrive at: coded orientation, square pixels.
    pub frame: FrameSize,
    /// Counter-clockwise rotation to apply when drawing: 0, 90, 180 or 270.
    pub rotation: u32,
    /// The picture's size once rotated, at source resolution.
    pub display_width: u32,
    pub display_height: u32,
    /// The time base of every timestamp the session reports.
    pub time_base: Rational,
    pub duration_seconds: Option<f64>,
    /// The video stream being previewed.
    pub stream: u32,
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
    /// Times the decoder was restarted ahead of a clock it could not keep up
    /// with.
    pub resyncs: u32,
    /// The decoder has delivered its last frame and it has been shown.
    pub ended: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct Shown {
    seq: u64,
    frame: Option<Arc<VideoFrame>>,
}

/// A preview of one video stream of one source.
#[derive(Debug)]
pub struct PreviewSession {
    orchestrator: Orchestrator,
    source: PathBuf,
    info: PreviewInfo,
    /// The stream's first presentation timestamp.
    start_pts: i64,
    index: Arc<KeyframeIndex>,
    ring: Arc<FrameRing>,
    decoder: Mutex<Option<VideoDecoder>>,
    clock: WallClock,
    shown: Mutex<Shown>,
    resyncs: AtomicU32,
    last_resync: Mutex<Option<Instant>>,
    error: Mutex<Option<DecodeError>>,
    closed: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PreviewSession {
    /// A session for the first video stream of `source`, delivering frames
    /// that fit `max_width` by `max_height` once rotated. Nothing is decoded
    /// until [`PreviewSession::play_from`].
    ///
    /// # Errors
    ///
    /// The file has no video stream.
    pub fn open(
        orchestrator: Orchestrator,
        source: &Path,
        media: &MediaInfo,
        index: Arc<KeyframeIndex>,
        max_width: u32,
        max_height: u32,
    ) -> Result<Self, DecodeError> {
        let (stream, video) =
            preview_stream(media).ok_or_else(|| DecodeError::NoVideo(source.to_path_buf()))?;
        let frame = FrameSize::fit(video, max_width, max_height);
        let info = PreviewInfo {
            frame,
            rotation: video.rotation,
            display_width: video.display_width,
            display_height: video.display_height,
            time_base: stream
                .time_base
                .filter(|tb| tb.num > 0 && tb.den > 0)
                .unwrap_or(Rational { num: 1, den: 1000 }),
            duration_seconds: stream.duration_seconds.or(media.container.duration_seconds),
            stream: stream.index,
        };
        let start_seconds = stream.start_seconds.unwrap_or(0.0);
        // Saturating, and the time base is positive by the filter above.
        #[allow(clippy::cast_possible_truncation)]
        let start_pts = info
            .time_base
            .value()
            .map_or(0, |per_tick| (start_seconds / per_tick).round() as i64);
        Ok(Self {
            orchestrator,
            source: source.to_path_buf(),
            info,
            start_pts,
            index,
            ring: Arc::new(FrameRing::new(FrameRing::capacity_for(
                frame.bytes(),
                RING_BUDGET_BYTES,
            ))),
            decoder: Mutex::new(None),
            clock: WallClock::default(),
            shown: Mutex::new(Shown::default()),
            resyncs: AtomicU32::new(0),
            last_resync: Mutex::new(None),
            error: Mutex::new(None),
            closed: AtomicBool::new(false),
        })
    }

    #[must_use]
    pub fn info(&self) -> PreviewInfo {
        self.info
    }

    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Start playing at `pts`: decode from the keyframe at or before it,
    /// deliver from exactly it, and start the clock there once the first
    /// frame is ready.
    ///
    /// # Errors
    ///
    /// The session is closed, or the keyframe index could not be read.
    pub fn play_from(&self, pts: i64) -> Result<(), DecodeError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(DecodeError::Failed("the preview is closed".to_owned()));
        }
        self.clock.stop();
        let seek_to = self.seek_point(pts)?;
        self.restart(seek_to, pts);
        self.ring.wait_for_frame(PREROLL_WAIT);
        self.clock.start(self.seconds(pts));
        Ok(())
    }

    /// Start playing at the first frame of the stream.
    ///
    /// # Errors
    ///
    /// As [`PreviewSession::play_from`].
    pub fn play_from_start(&self) -> Result<(), DecodeError> {
        self.play_from(self.start_pts)
    }

    /// The keyframe to seek to for `pts`, or `None` when decoding from the
    /// start of the stream reaches it just as well.
    fn seek_point(&self, pts: i64) -> Result<Option<i64>, DecodeError> {
        let stream = self.info.stream;
        let keyframe = self
            .index
            .at_or_before(stream, pts)
            .map_err(|error| DecodeError::Failed(error.to_string()))?;
        let Some(keyframe) = keyframe else {
            return Ok(None);
        };
        let earlier = self
            .index
            .at_or_before(stream, keyframe.pts.saturating_sub(1))
            .map_err(|error| DecodeError::Failed(error.to_string()))?;
        Ok(earlier.map(|_| keyframe.pts))
    }

    fn restart(&self, seek_to: Option<i64>, first_pts: i64) {
        let previous = lock(&self.decoder).take();
        if let Some(previous) = previous {
            previous.stop();
        }
        self.ring.reset();
        *lock(&self.error) = None;
        let request = DecodeRequest {
            source: self.source.clone(),
            stream: self.info.stream,
            time_base: self.info.time_base,
            seek_to,
            first_pts,
            size: self.info.frame,
            max_frames: None,
        };
        let decoder = VideoDecoder::start(&self.orchestrator, &request, Arc::clone(&self.ring));
        *lock(&self.decoder) = Some(decoder);
    }

    fn seconds(&self, ticks: i64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let ticks = ticks as f64;
        ticks * self.info.time_base.value().unwrap_or(0.0)
    }

    fn ticks(&self, seconds: f64) -> i64 {
        let per_tick = self.info.time_base.value().unwrap_or(1.0);
        let ticks = (seconds / per_tick).floor();
        // `as` saturates, and a non-finite value cannot occur: the time base
        // is positive by construction.
        #[allow(clippy::cast_possible_truncation)]
        let ticks = ticks as i64;
        ticks
    }

    /// The frame to show now, if it is newer than `after` — waiting up to
    /// `timeout` for one to become due. Returns its sequence number, which the
    /// caller passes back as `after` next time.
    pub fn next_frame(&self, after: u64, timeout: Duration) -> Option<(u64, Arc<VideoFrame>)> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            let now = self.clock.position().map(|seconds| self.ticks(seconds));
            if let Some(now) = now {
                self.keep_up(now);
                if let Some(frame) = self.ring.take_due(now) {
                    let mut shown = lock(&self.shown);
                    shown.seq += 1;
                    shown.frame = Some(Arc::new(frame));
                }
            }
            {
                let shown = lock(&self.shown);
                if shown.seq > after
                    && let Some(frame) = &shown.frame
                {
                    return Some((shown.seq, Arc::clone(frame)));
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            let wait = match (now, self.ring.earliest_pts()) {
                (Some(now), Some(next)) => {
                    let seconds = self.seconds(next.saturating_sub(now)).max(0.0);
                    Duration::from_secs_f64(seconds.min(left.as_secs_f64()))
                        .max(Duration::from_millis(1))
                }
                _ => left.min(Duration::from_millis(5)),
            };
            if self.ring.earliest_pts().is_none() {
                self.ring.wait_for_frame(wait);
            } else {
                std::thread::sleep(wait);
            }
        }
    }

    /// Record how the decoder ended, and restart it ahead of the clock if it
    /// has fallen too far behind.
    fn keep_up(&self, now: i64) {
        let guard = lock(&self.decoder);
        let Some(decoder) = guard.as_ref() else {
            return;
        };
        if let Some(DecodeEnd::Failed(error)) = decoder.end() {
            *lock(&self.error) = Some(error);
            return;
        }
        if decoder.end().is_some() {
            return;
        }
        let Some(head) = decoder.head() else {
            return;
        };
        let behind = self.seconds(now.saturating_sub(head));
        if behind < RESYNC_AFTER.as_secs_f64() {
            return;
        }
        drop(guard);
        let mut last = lock(&self.last_resync);
        if last.is_some_and(|at| at.elapsed() < RESYNC_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        let target = now.saturating_add(self.ticks(RESYNC_LEAD.as_secs_f64()));
        if let Ok(Some(keyframe)) = self.index.at_or_after(self.info.stream, target) {
            self.resyncs.fetch_add(1, Ordering::Relaxed);
            self.restart(Some(keyframe.pts), keyframe.pts);
        }
    }

    #[must_use]
    pub fn stats(&self) -> DecodeStats {
        let ring = self.ring.stats();
        let decoder_ended = lock(&self.decoder)
            .as_ref()
            .and_then(VideoDecoder::end)
            .is_some_and(|end| end == DecodeEnd::Finished);
        let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        DecodeStats {
            buffered_frames: to_u32(ring.buffered_frames),
            capacity_frames: to_u32(ring.capacity_frames),
            buffered_bytes: u64::try_from(ring.buffered_bytes).unwrap_or(u64::MAX),
            decoded_frames: ring.decoded_frames,
            dropped_frames: ring.dropped_frames,
            presented_frames: ring.presented_frames,
            decode_fps: ring.decode_fps,
            resyncs: self.resyncs.load(Ordering::Relaxed),
            ended: decoder_ended && self.ring.is_drained(),
            error: lock(&self.error).as_ref().map(ToString::to_string),
        }
    }

    /// The running decoder process, if there is one.
    #[must_use]
    pub fn decoder_pid(&self) -> Option<u32> {
        lock(&self.decoder).as_ref().and_then(VideoDecoder::pid)
    }

    /// Stop everything. Returns once the decoder process has exited.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.clock.stop();
        self.ring.close();
        let decoder = lock(&self.decoder).take();
        if let Some(decoder) = decoder {
            decoder.stop();
        }
    }
}

impl Drop for PreviewSession {
    fn drop(&mut self) {
        self.close();
    }
}

/// The stream a preview shows: the default video stream if one is marked,
/// otherwise the first, never a cover image.
fn preview_stream(media: &MediaInfo) -> Option<(&StreamInfo, &VideoInfo)> {
    let moving: Vec<_> = media
        .video()
        .filter(|(_, video)| !video.is_attached_picture)
        .collect();
    moving
        .iter()
        .find(|(stream, _)| stream.is_default)
        .or_else(|| moving.first())
        .copied()
}
