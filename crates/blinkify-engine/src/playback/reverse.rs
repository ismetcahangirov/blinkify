//! A reversed clip's pictures, backwards, in bounded memory (#113, ADR-0017).
//!
//! No decoder runs backwards, and decoding the whole clip to turn it round
//! holds all of it. So the clip is decoded a **chunk** at a time from its end
//! to its start, as the export does it (#55): each chunk is decoded forwards
//! by the ordinary two-stage seek ([`start_decoder`]), collected, and handed
//! to the lane's ring newest frame first. Every frame keeps its real source
//! timestamp, so the presenter picks the frame the evaluator names exactly as
//! it does forwards — only the direction of "due" changes
//! ([`FrameRing::take_due_backwards`]).
//!
//! - **A chunk** ends where the last one began and reaches back at most
//!   [`chunk_frames`] frames, starting at the earliest keyframe within that
//!   reach if there is one: a whole group of pictures when groups are short,
//!   nothing decoded twice, and never more frames than the bound however
//!   long a group is.
//! - **The next chunk is decoded while this one plays.** Its process starts
//!   before this chunk's frames go into the ring, so playback never waits for
//!   a process to start at a chunk boundary.
//! - **The bound.** At most the ring, the chunk being handed over and the
//!   chunk being decoded: [`memory_bound`], whatever the clip's length. The
//!   decoder measures what it holds and keeps the peak, and a test asserts
//!   it stays under the bound on a long clip.
//! - **Falling behind** skips ahead. A chunk the clock has already passed is
//!   never decoded: the next chunk ends at the frame the presenter last asked
//!   for, if that is earlier.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::lanes::{LANE_BUDGET_BYTES, start_decoder};
use super::plan::Segment;
use crate::decode::{DecodeEnd, DecodeError, FrameRing, VideoDecoder, VideoFrame};
use crate::orchestrator::{CancelToken, Orchestrator};

/// Memory one chunk of a reversed clip may hold.
const CHUNK_BUDGET_BYTES: usize = 32 * 1024 * 1024;

/// The fewest frames in a chunk: fewer and a process start per frame or two
/// costs more than the decode.
const MIN_CHUNK_FRAMES: usize = 4;

/// The most frames in a chunk: a second at 30 fps. More costs latency at a
/// seek — the whole chunk is decoded before its newest frame can be shown —
/// and buys nothing once the next chunk is prefetched.
const MAX_CHUNK_FRAMES: usize = 30;

/// How often a wait re-checks for a stop.
const RECHECK: Duration = Duration::from_millis(20);

/// Frames of `frame_bytes` in one chunk.
#[must_use]
pub(crate) fn chunk_frames(frame_bytes: usize) -> usize {
    CHUNK_BUDGET_BYTES
        .checked_div(frame_bytes)
        .unwrap_or(MAX_CHUNK_FRAMES)
        .clamp(MIN_CHUNK_FRAMES, MAX_CHUNK_FRAMES)
}

/// The most bytes of pictures a reversed lane holds at once, for frames of
/// `frame_bytes`: its ring, the chunk being handed to it and the chunk being
/// decoded. Independent of the clip's length.
#[must_use]
pub fn memory_bound(frame_bytes: usize) -> usize {
    let ring = FrameRing::capacity_for(frame_bytes, LANE_BUDGET_BYTES);
    ring.saturating_add(2 * chunk_frames(frame_bytes))
        .saturating_mul(frame_bytes)
}

/// One reversed segment's pictures being decoded into a ring.
#[derive(Debug)]
pub(crate) struct ReverseDecoder {
    cancel: CancelToken,
    ring: Arc<FrameRing>,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Default)]
struct Shared {
    end: Mutex<Option<DecodeEnd>>,
    /// The chunk decoder running now, if any.
    pid: Mutex<Option<u32>>,
    /// The most bytes of pictures held at once so far.
    peak: AtomicUsize,
}

impl ReverseDecoder {
    /// Start delivering `segment` backwards from the frame on screen at
    /// source tick `pts` to its first frame, into `ring`. Returns at once.
    pub(crate) fn start(
        orchestrator: &Orchestrator,
        segment: &Segment,
        pts: i64,
        bound: (u32, u32),
        ring: &Arc<FrameRing>,
    ) -> Self {
        let cancel = CancelToken::default();
        let shared = Arc::new(Shared::default());
        let thread = {
            let run = Run {
                orchestrator: orchestrator.clone(),
                segment: segment.clone(),
                bound,
                ring: Arc::clone(ring),
                cancel: cancel.clone(),
                shared: Arc::clone(&shared),
            };
            thread::Builder::new()
                .name("preview-reverse".to_owned())
                .spawn(move || {
                    let end = run.run(pts);
                    run.ring.finish();
                    *run.shared
                        .end
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(end);
                })
                .ok()
        };
        Self {
            cancel,
            ring: Arc::clone(ring),
            shared,
            thread,
        }
    }

    /// How the decode ended, or `None` while it runs.
    pub(crate) fn end(&self) -> Option<DecodeEnd> {
        self.shared
            .end
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The running chunk decoder's process id, if one is running.
    pub(crate) fn pid(&self) -> Option<u32> {
        *self
            .shared
            .pid
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The most bytes of pictures this lane has held at once.
    pub(crate) fn peak_bytes(&self) -> usize {
        self.shared.peak.load(Ordering::Relaxed)
    }

    /// Stop decoding and wait until every process has exited.
    pub(crate) fn stop(mut self) {
        self.stop_in_place();
    }

    fn stop_in_place(&mut self) {
        self.cancel.cancel();
        self.ring.wake_producers();
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

/// One chunk: its frames' range in the source, and the decode filling it.
struct Chunk {
    first: i64,
    last: i64,
    ring: Arc<FrameRing>,
    decoder: VideoDecoder,
}

/// The decoding thread's state.
struct Run {
    orchestrator: Orchestrator,
    segment: Segment,
    bound: (u32, u32),
    ring: Arc<FrameRing>,
    cancel: CancelToken,
    shared: Arc<Shared>,
}

impl Run {
    fn run(&self, pts: i64) -> DecodeEnd {
        let Some(video) = self.segment.source.video.as_ref() else {
            return DecodeEnd::Failed(DecodeError::NoVideo(self.segment.source.path.clone()));
        };
        let index = &self.segment.source.index;
        let stream = video.index;
        let frame_at = |tick: i64| index.frame_at_or_before(stream, tick).ok().flatten();
        // The clip's first frame: the one on screen at its in point.
        let Some(floor) = frame_at(self.segment.source_in).or_else(|| {
            index
                .frame_after(stream, self.segment.source_in)
                .ok()
                .flatten()
        }) else {
            return DecodeEnd::Finished;
        };
        let Some(newest) = frame_at(pts).map(|newest| newest.max(floor)) else {
            return DecodeEnd::Finished;
        };
        let frame_bytes = super::lanes::frame_size(&self.segment, self.bound)
            .map_or(1, crate::decode::FrameSize::bytes)
            .max(1);
        let limit = chunk_frames(frame_bytes);
        let mut pending = match self.chunk(newest, floor, limit) {
            Ok(chunk) => chunk,
            Err(end) => return end,
        };
        loop {
            let frames = match self.collect(&pending) {
                Ok(frames) => frames,
                Err(end) => return end,
            };
            // Decode the next chunk while this one is shown — ending where
            // this one began, or where the presenter has got to if that is
            // earlier: a chunk the clock has passed is never decoded.
            let mut next_end = index
                .frame_before(stream, pending.first)
                .ok()
                .flatten()
                .filter(|end| *end >= floor);
            if let (Some(end), Some(asked)) = (next_end, self.ring.asked())
                && asked < end
            {
                next_end = frame_at(asked).filter(|at| *at >= floor);
            }
            let next = match next_end.map(|end| self.chunk(end, floor, limit)) {
                Some(Ok(chunk)) => Some(chunk),
                Some(Err(end)) => return end,
                None => None,
            };
            if let Err(end) = self.hand_over(frames, next.as_ref()) {
                return end;
            }
            match next {
                Some(chunk) => pending = chunk,
                None => return DecodeEnd::Finished,
            }
        }
    }

    /// Start decoding the chunk ending at frame `last`: back at most `limit`
    /// frames, but no further than `floor`, starting at the earliest
    /// keyframe in that reach if there is one.
    fn chunk(&self, last: i64, floor: i64, limit: usize) -> Result<Chunk, DecodeEnd> {
        let video = self.segment.source.video.as_ref().ok_or_else(|| {
            DecodeEnd::Failed(DecodeError::NoVideo(self.segment.source.path.clone()))
        })?;
        let index = &self.segment.source.index;
        let mut frames = vec![last];
        while frames.len() < limit {
            let Some(&earliest) = frames.last() else {
                break;
            };
            match index.frame_before(video.index, earliest) {
                Ok(Some(previous)) if previous >= floor => frames.push(previous),
                _ => break,
            }
        }
        let keyframe = frames
            .iter()
            .rev()
            .find(|pts| index.is_keyframe(video.index, **pts).unwrap_or(false))
            .copied();
        let first = keyframe.or_else(|| frames.last().copied()).unwrap_or(last);
        let count = frames.iter().filter(|pts| **pts >= first).count();
        let ring = Arc::new(FrameRing::new(count));
        let decoder = start_decoder(
            &self.orchestrator,
            &self.segment,
            first,
            self.bound,
            u32::try_from(count).ok(),
            &ring,
        )
        .map_err(DecodeEnd::Failed)?;
        Ok(Chunk {
            first,
            last,
            ring,
            decoder,
        })
    }

    /// Wait for `chunk` to finish decoding and take its frames, oldest first.
    fn collect(&self, chunk: &Chunk) -> Result<Vec<VideoFrame>, DecodeEnd> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(DecodeEnd::Stopped);
            }
            self.running(Some(chunk));
            match chunk.decoder.end() {
                None => thread::sleep(RECHECK),
                Some(DecodeEnd::Finished) => break,
                Some(other) => return Err(other),
            }
        }
        self.running(None);
        let mut frames = Vec::new();
        while let Some(frame) = chunk.ring.pop() {
            if (chunk.first..=chunk.last).contains(&frame.pts) {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    /// Hand `frames` to the ring newest first, blocking while it is full,
    /// and keep the peak of what is held: the ring, what is left of these,
    /// and the chunk `next` being decoded meanwhile.
    fn hand_over(
        &self,
        mut frames: Vec<VideoFrame>,
        next: Option<&Chunk>,
    ) -> Result<(), DecodeEnd> {
        let mut left: usize = frames.iter().map(|frame| frame.pixels.len()).sum();
        while let Some(frame) = frames.pop() {
            self.running(next);
            let decoding = next.map_or(0, |chunk| chunk.ring.stats().buffered_bytes);
            let held = self
                .ring
                .stats()
                .buffered_bytes
                .saturating_add(left)
                .saturating_add(decoding);
            self.shared.peak.fetch_max(held, Ordering::Relaxed);
            let bytes = frame.pixels.len();
            if !self.ring.push(frame, &self.cancel) {
                return Err(DecodeEnd::Stopped);
            }
            left = left.saturating_sub(bytes);
        }
        Ok(())
    }

    /// Record which chunk's process is running, for teardown checks.
    fn running(&self, chunk: Option<&Chunk>) {
        *self
            .shared
            .pid
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = chunk.and_then(|chunk| chunk.decoder.pid());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chunk_holds_a_second_at_most_and_a_few_frames_at_least() {
        assert_eq!(chunk_frames(160 * 90 * 4), MAX_CHUNK_FRAMES);
        assert_eq!(chunk_frames(3840 * 2160 * 4), MIN_CHUNK_FRAMES);
        assert_eq!(chunk_frames(0), MAX_CHUNK_FRAMES);
    }

    #[test]
    fn the_bound_is_the_ring_and_two_chunks_whatever_the_clip() {
        let frame = 320 * 180 * 4;
        assert_eq!(
            memory_bound(frame),
            (FrameRing::capacity_for(frame, LANE_BUDGET_BYTES) + 2 * MAX_CHUNK_FRAMES) * frame
        );
    }
}
