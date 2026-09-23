//! The bounded buffer between a decoder and the presenter.
//!
//! The bound is the whole point. A decoder that outruns the renderer — a
//! throttled window, a slow machine, a paused presenter — would otherwise
//! consume all available memory until the application is killed. Here the
//! producer blocks when the ring is full, which blocks the thread reading
//! FFmpeg's standard output, which fills the pipe, which stops FFmpeg. The
//! backpressure reaches the decoder process itself; nothing queues anywhere.
//!
//! Presentation is by timestamp, not by order of arrival: the presenter asks
//! for the frame that is due at the clock's current position, and every older
//! frame is dropped and counted. A late frame is never shown late — it is
//! skipped — so video under load drops frames rather than falling behind.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::VideoFrame;
use crate::orchestrator::CancelToken;

/// The fewest frames a ring holds, whatever the frame size: one being shown,
/// one due next, one being decoded.
const MIN_FRAMES: usize = 3;

/// The most frames a ring holds, whatever the budget. Beyond this more
/// buffering buys no smoothness, only seek latency.
const MAX_FRAMES: usize = 32;

/// How many recent arrivals the decode rate is measured over.
const RATE_WINDOW: usize = 32;

/// How long a blocked producer sleeps before re-checking for close. A close
/// notifies it at once; this only bounds a missed wake-up.
const PRODUCER_RECHECK: Duration = Duration::from_millis(100);

#[derive(Debug, Default)]
struct State {
    frames: VecDeque<VideoFrame>,
    /// The consumer is gone; producers stop.
    closed: bool,
    /// The producer has delivered its last frame.
    finished: bool,
    arrivals: VecDeque<Instant>,
}

/// A snapshot of a ring, for diagnosis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingStats {
    pub buffered_frames: usize,
    pub capacity_frames: usize,
    pub buffered_bytes: usize,
    pub decoded_frames: u64,
    pub dropped_frames: u64,
    pub presented_frames: u64,
    /// Frames per second arriving from the decoder, over the recent window.
    pub decode_fps: f64,
    pub finished: bool,
}

/// A bounded, timestamp-ordered buffer of decoded frames.
#[derive(Debug)]
pub struct FrameRing {
    capacity: usize,
    state: Mutex<State>,
    changed: Condvar,
    decoded: AtomicU64,
    dropped: AtomicU64,
    presented: AtomicU64,
}

impl FrameRing {
    /// A ring that holds exactly `capacity` frames (at least one).
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            decoded: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            presented: AtomicU64::new(0),
        }
    }

    /// How many frames of `frame_bytes` fit in `budget_bytes`, clamped to a
    /// range that keeps playback smooth and seeks quick.
    #[must_use]
    pub fn capacity_for(frame_bytes: usize, budget_bytes: usize) -> usize {
        budget_bytes
            .checked_div(frame_bytes)
            .unwrap_or(MAX_FRAMES)
            .clamp(MIN_FRAMES, MAX_FRAMES)
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Add a decoded frame, blocking while the ring is full. Returns `false`
    /// if the ring was closed or `cancel` fired, in which case the frame is
    /// discarded and the producer should stop.
    ///
    /// The cancel matters as much as the close: a decoder being replaced is
    /// blocked right here, on a full ring nobody will drain until it is gone.
    pub fn push(&self, frame: VideoFrame, cancel: &CancelToken) -> bool {
        let mut state = self.lock();
        while !state.closed && !cancel.is_cancelled() && state.frames.len() >= self.capacity {
            state = self
                .changed
                .wait_timeout(state, PRODUCER_RECHECK)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if state.closed || cancel.is_cancelled() {
            return false;
        }
        // Frames arrive in presentation order from one decoder; keep them
        // sorted anyway, so presentation never depends on that assumption.
        let at = state.frames.partition_point(|f| f.pts <= frame.pts);
        state.frames.insert(at, frame);
        if state.arrivals.len() == RATE_WINDOW {
            state.arrivals.pop_front();
        }
        state.arrivals.push_back(Instant::now());
        drop(state);
        self.decoded.fetch_add(1, Ordering::Relaxed);
        self.changed.notify_all();
        true
    }

    /// Wake every blocked producer so it re-checks its cancel token.
    pub fn wake_producers(&self) {
        self.changed.notify_all();
    }

    /// The producer has delivered everything it will.
    pub fn finish(&self) {
        self.lock().finished = true;
        self.changed.notify_all();
    }

    /// The consumer is gone: wake and stop every producer, drop every frame.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.frames.clear();
        drop(state);
        self.changed.notify_all();
    }

    /// Drop every buffered frame and reopen for a new run of the decoder — a
    /// seek, or a resynchronisation. Dropped frames here are not counted as
    /// dropped under load: nobody was ever going to see them.
    pub fn reset(&self) {
        let mut state = self.lock();
        state.frames.clear();
        state.finished = false;
        state.arrivals.clear();
        drop(state);
        self.changed.notify_all();
    }

    /// The newest frame due at `now` — its timestamp at or before it — with
    /// every older frame dropped and counted. `None` when nothing is due yet.
    pub fn take_due(&self, now: i64) -> Option<VideoFrame> {
        let mut state = self.lock();
        let due = state.frames.partition_point(|f| f.pts <= now);
        if due == 0 {
            return None;
        }
        let mut taken = state.frames.drain(..due);
        let late = taken.len().saturating_sub(1);
        let frame = taken.next_back();
        drop(taken);
        drop(state);
        self.changed.notify_all();
        self.dropped
            .fetch_add(u64::try_from(late).unwrap_or(u64::MAX), Ordering::Relaxed);
        if frame.is_some() {
            self.presented.fetch_add(1, Ordering::Relaxed);
        }
        frame
    }

    /// The earliest buffered timestamp, if any.
    #[must_use]
    pub fn earliest_pts(&self) -> Option<i64> {
        self.lock().frames.front().map(|f| f.pts)
    }

    /// The latest buffered timestamp, if any.
    #[must_use]
    pub fn latest_pts(&self) -> Option<i64> {
        self.lock().frames.back().map(|f| f.pts)
    }

    /// Whether the producer has finished and every frame has been taken.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        let state = self.lock();
        state.finished && state.frames.is_empty()
    }

    /// Wait until a frame arrives, the producer finishes, or `timeout` passes.
    pub fn wait_for_frame(&self, timeout: Duration) {
        let state = self.lock();
        if !state.frames.is_empty() || state.finished || state.closed {
            return;
        }
        let _ = self
            .changed
            .wait_timeout(state, timeout)
            .unwrap_or_else(PoisonError::into_inner);
    }

    #[must_use]
    pub fn stats(&self) -> RingStats {
        let state = self.lock();
        let decode_fps = match (state.arrivals.front(), state.arrivals.back()) {
            (Some(first), Some(last)) if state.arrivals.len() > 1 => {
                let span = last.duration_since(*first).as_secs_f64();
                #[allow(clippy::cast_precision_loss)]
                let intervals = (state.arrivals.len() - 1) as f64;
                if span > 0.0 { intervals / span } else { 0.0 }
            }
            _ => 0.0,
        };
        RingStats {
            buffered_frames: state.frames.len(),
            capacity_frames: self.capacity,
            buffered_bytes: state.frames.iter().map(|f| f.pixels.len()).sum(),
            decoded_frames: self.decoded.load(Ordering::Relaxed),
            dropped_frames: self.dropped.load(Ordering::Relaxed),
            presented_frames: self.presented.load(Ordering::Relaxed),
            decode_fps,
            finished: state.finished,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    fn frame(pts: i64) -> VideoFrame {
        VideoFrame {
            pts,
            width: 2,
            height: 1,
            pixels: vec![0; 8],
        }
    }

    #[test]
    fn a_full_ring_blocks_its_producer_until_a_frame_is_taken() {
        let ring = Arc::new(FrameRing::new(2));
        assert!(ring.push(frame(0), &CancelToken::default()));
        assert!(ring.push(frame(1), &CancelToken::default()));
        let producer = {
            let ring = Arc::clone(&ring);
            thread::spawn(move || ring.push(frame(2), &CancelToken::default()))
        };
        thread::sleep(Duration::from_millis(100));
        assert!(!producer.is_finished(), "the third push must block");
        assert_eq!(ring.stats().buffered_frames, 2);

        assert_eq!(ring.take_due(0).map(|f| f.pts), Some(0));
        assert!(producer.join().expect("producer"));
        assert_eq!(ring.stats().buffered_frames, 2);
    }

    #[test]
    fn closing_releases_a_blocked_producer_and_refuses_more() {
        let ring = Arc::new(FrameRing::new(1));
        assert!(ring.push(frame(0), &CancelToken::default()));
        let producer = {
            let ring = Arc::clone(&ring);
            thread::spawn(move || ring.push(frame(1), &CancelToken::default()))
        };
        thread::sleep(Duration::from_millis(50));
        ring.close();
        assert!(!producer.join().expect("producer"));
        assert!(!ring.push(frame(2), &CancelToken::default()));
        assert_eq!(ring.stats().buffered_frames, 0);
    }

    #[test]
    fn the_due_frame_is_the_newest_at_or_before_now_and_older_ones_drop() {
        let ring = FrameRing::new(8);
        for pts in [0, 10, 20, 30] {
            ring.push(frame(pts), &CancelToken::default());
        }
        assert_eq!(ring.take_due(-1).map(|f| f.pts), None);
        assert_eq!(ring.take_due(25).map(|f| f.pts), Some(20));
        let stats = ring.stats();
        assert_eq!(stats.dropped_frames, 2, "0 and 10 were late");
        assert_eq!(stats.presented_frames, 1);
        assert_eq!(stats.buffered_frames, 1);
        assert_eq!(ring.take_due(30).map(|f| f.pts), Some(30));
        assert_eq!(ring.stats().dropped_frames, 2);
    }

    #[test]
    fn a_reset_is_not_counted_as_dropping_under_load() {
        let ring = FrameRing::new(4);
        ring.push(frame(0), &CancelToken::default());
        ring.push(frame(1), &CancelToken::default());
        ring.finish();
        ring.reset();
        let stats = ring.stats();
        assert_eq!(stats.buffered_frames, 0);
        assert_eq!(stats.dropped_frames, 0);
        assert!(!stats.finished);
    }

    #[test]
    fn capacity_follows_the_budget_within_its_clamp() {
        assert_eq!(FrameRing::capacity_for(1920 * 1080 * 4, 128 << 20), 16);
        assert_eq!(
            FrameRing::capacity_for(3840 * 2160 * 4, 16 << 20),
            MIN_FRAMES
        );
        assert_eq!(FrameRing::capacity_for(16, 1 << 30), MAX_FRAMES);
        assert_eq!(FrameRing::capacity_for(0, 1 << 20), MAX_FRAMES);
    }
}
