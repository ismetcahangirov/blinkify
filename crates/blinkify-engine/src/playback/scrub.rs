//! Scrubbing: dragging the playhead.
//!
//! A drag asks for far more positions than can be decoded. Queueing them makes
//! a scrub feel broken — the picture trails the pointer further with every
//! move — so requests are **coalesced**: there is one slot, a new position
//! replaces the old, and the worker always serves the latest. Positions that
//! were superseded before the worker got to them are never decoded.
//!
//! Each decode is a **window** around the target: the two-stage seek to the
//! target's frame ([`crate::seek`]), extended a little into the direction the
//! pointer is moving, and every frame of it goes into a [`FrameCache`]. The
//! next positions of a steady drag then come straight from the cache. The
//! cache is bounded in bytes by the same budget as playback, so prefetch can
//! never reintroduce the memory problem the ring bound exists to prevent.
//!
//! A window is not abandoned before the target's own frame is on screen —
//! otherwise a fast drag over a sparse-keyframe source would show nothing at
//! all — but once it is, a newer position elsewhere stops it.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::lanes::{frame_size, rotation_of, start_decoder};
use super::plan::{PlaybackPlan, ProgramTime, Segment};
use crate::decode::{FrameRing, VideoFrame};
use crate::orchestrator::Orchestrator;

/// The most frames one window decodes.
const WINDOW_MAX: usize = 48;

/// How far past the target a window may still be worth finishing for a
/// newer position.
const STILL_COMING: ProgramTime = 2_000_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Decoded frames by segment and source timestamp, bounded in bytes. The
/// oldest insertions go first.
#[derive(Debug)]
pub(crate) struct FrameCache {
    budget: usize,
    bytes: usize,
    frames: HashMap<(usize, i64), Arc<VideoFrame>>,
    order: VecDeque<(usize, i64)>,
}

impl FrameCache {
    pub(crate) fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            frames: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub(crate) fn get(&self, segment: usize, pts: i64) -> Option<Arc<VideoFrame>> {
        self.frames.get(&(segment, pts)).cloned()
    }

    pub(crate) fn insert(&mut self, segment: usize, frame: Arc<VideoFrame>) {
        let key = (segment, frame.pts);
        let size = frame.pixels.len();
        if size > self.budget {
            return;
        }
        if let Some(old) = self.frames.insert(key, frame) {
            self.bytes = self.bytes.saturating_sub(old.pixels.len());
        } else {
            self.order.push_back(key);
        }
        self.bytes += size;
        while self.bytes > self.budget {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.frames.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(evicted.pixels.len());
            }
        }
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    pub(crate) fn budget(&self) -> usize {
        self.budget
    }
}

#[derive(Debug, Default)]
struct State {
    /// The position asked for most recently.
    target: Option<ProgramTime>,
    /// The one before it, for the direction of travel.
    previous: Option<ProgramTime>,
    /// Negative while dragging backwards.
    direction: i64,
    stopped: bool,
    requests: u64,
    decodes: u64,
}

/// The single slot between the transport and the scrub worker.
#[derive(Debug, Default)]
pub(crate) struct ScrubSlot {
    state: Mutex<State>,
    changed: Condvar,
}

impl ScrubSlot {
    /// Ask for `t`, replacing whatever was asked for before.
    pub(crate) fn request(&self, t: ProgramTime) {
        let mut state = lock(&self.state);
        if let Some(previous) = state.target {
            state.direction = (t - previous).signum();
            state.previous = Some(previous);
        }
        state.target = Some(t);
        state.requests += 1;
        drop(state);
        self.changed.notify_all();
    }

    pub(crate) fn stop(&self) {
        lock(&self.state).stopped = true;
        self.changed.notify_all();
    }

    pub(crate) fn latest(&self) -> Option<ProgramTime> {
        lock(&self.state).target
    }

    /// Positions asked for, and windows decoded to serve them.
    pub(crate) fn counts(&self) -> (u64, u64) {
        let state = lock(&self.state);
        (state.requests, state.decodes)
    }

    /// Wait for a position other than `served`. `None` once stopped.
    fn next(&self, served: Option<ProgramTime>) -> Option<(ProgramTime, i64)> {
        let mut state = lock(&self.state);
        loop {
            if state.stopped {
                return None;
            }
            if let Some(target) = state.target
                && Some(target) != served
            {
                return Some((target, state.direction));
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn decoded_a_window(&self) {
        lock(&self.state).decodes += 1;
    }
}

/// What the worker puts on screen.
pub(crate) enum Picture {
    Frame {
        segment: usize,
        frame: Arc<VideoFrame>,
        rotation: u32,
    },
    Black,
}

/// Everything the worker needs.
pub(crate) struct Worker {
    pub slot: Arc<ScrubSlot>,
    pub orchestrator: Orchestrator,
    pub plan: Arc<PlaybackPlan>,
    pub bound: (u32, u32),
    pub cache: Arc<Mutex<FrameCache>>,
}

impl Worker {
    /// Serve positions until the slot is stopped. `show` is called with each
    /// picture and the position it was asked for.
    pub(crate) fn run(&self, show: &dyn Fn(Picture, ProgramTime)) {
        let mut served = None;
        while let Some((t, direction)) = self.slot.next(served) {
            served = Some(t);
            let Some((i, segment)) = self.plan.segment_at(t) else {
                show(Picture::Black, t);
                continue;
            };
            let Some(video) = segment.source.video.as_ref() else {
                show(Picture::Black, t);
                continue;
            };
            let pts = segment
                .source_at(t)
                .min(segment.source_out.saturating_sub(1));
            let index = &segment.source.index;
            let frame_pts = index
                .frame_at_or_before(video.index, pts)
                .ok()
                .flatten()
                .or_else(|| index.frame_after(video.index, pts).ok().flatten())
                .unwrap_or(pts);
            let rotation = rotation_of(segment);
            if let Some(frame) = lock(&self.cache).get(i, frame_pts) {
                show(
                    Picture::Frame {
                        segment: i,
                        frame,
                        rotation,
                    },
                    t,
                );
                continue;
            }
            self.window(i, segment, frame_pts, direction, rotation, t, show);
        }
    }

    /// Decode a window around `frame_pts` into the cache, showing that frame
    /// as soon as it arrives.
    #[allow(clippy::too_many_arguments)]
    fn window(
        &self,
        i: usize,
        segment: &Segment,
        frame_pts: i64,
        direction: i64,
        rotation: u32,
        t: ProgramTime,
        show: &dyn Fn(Picture, ProgramTime),
    ) {
        let Some(video) = segment.source.video.as_ref() else {
            return;
        };
        let Some(size) = frame_size(segment, self.bound) else {
            return;
        };
        let (budget, frame_bytes) = (lock(&self.cache).budget(), size.bytes().max(1));
        let fit = budget.div_euclid(frame_bytes).clamp(2, WINDOW_MAX);
        // Most of the window lies the way the pointer is moving.
        let behind = if direction < 0 {
            fit - fit.div_euclid(4)
        } else {
            fit.div_euclid(4)
        };
        let index = &segment.source.index;
        let mut first = frame_pts;
        for _ in 0..behind {
            match index.frame_before(video.index, first) {
                Ok(Some(previous)) if segment.program_at(previous) >= segment.timeline_start => {
                    first = previous;
                }
                _ => break,
            }
        }
        let ring = Arc::new(FrameRing::new(4));
        let Ok(decoder) = start_decoder(
            &self.orchestrator,
            segment,
            first,
            self.bound,
            u32::try_from(fit).ok(),
            &ring,
        ) else {
            show(Picture::Black, t);
            return;
        };
        self.slot.decoded_a_window();
        let mut shown = false;
        loop {
            ring.wait_for_frame(Duration::from_millis(50));
            while let Some(frame) = ring.pop() {
                let frame = Arc::new(frame);
                lock(&self.cache).insert(i, Arc::clone(&frame));
                if frame.pts == frame_pts {
                    shown = true;
                    show(
                        Picture::Frame {
                            segment: i,
                            frame,
                            rotation,
                        },
                        t,
                    );
                }
            }
            if decoder.end().is_some() && ring.earliest_pts().is_none() {
                break;
            }
            // Once the target is up, a newer position this window will not
            // reach soon is worth more than the rest of the window.
            if shown
                && let Some(newer) = self.slot.latest()
                && newer != t
                && !(direction >= 0 && newer > t && newer - t < STILL_COMING)
            {
                break;
            }
        }
        if !shown {
            // The frame was not in the window — the decode ended early.
            show(Picture::Black, t);
        }
        decoder.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts: i64, bytes: usize) -> Arc<VideoFrame> {
        Arc::new(VideoFrame {
            pts,
            width: 1,
            height: 1,
            pixels: vec![0; bytes],
        })
    }

    #[test]
    fn the_cache_never_holds_more_than_its_budget() {
        let mut cache = FrameCache::new(100);
        for pts in 0..20 {
            cache.insert(0, frame(pts, 30));
            assert!(cache.bytes() <= 100, "{} bytes", cache.bytes());
        }
        // The newest survive; the oldest went first.
        assert!(cache.get(0, 19).is_some());
        assert!(cache.get(0, 0).is_none());
        // A frame larger than the whole budget is not kept at all.
        cache.insert(1, frame(0, 500));
        assert!(cache.get(1, 0).is_none());
    }

    #[test]
    fn a_new_position_replaces_the_one_waiting() {
        let slot = ScrubSlot::default();
        slot.request(1_000);
        slot.request(2_000);
        slot.request(500);
        assert_eq!(slot.next(None), Some((500, -1)));
        assert_eq!(slot.counts(), (3, 0));
    }
}
