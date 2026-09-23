//! Video lanes: one decoder per segment being shown or about to be.
//!
//! A lane is a decoder and its frame ring for one segment, started at a given
//! source position, serving one clock generation. The lane for the segment
//! playing is the one presented from; the lane for the next segment is
//! started before the boundary so the first frame after it is already
//! decoded — a clip boundary never waits for a process to start.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::plan::{PlaybackPlan, Segment};
use crate::decode::{
    DecodeEnd, DecodeError, DecodeRequest, FrameRing, FrameSize, RingStats, VideoDecoder,
};
use crate::orchestrator::Orchestrator;
use crate::time::{self, Rounding};

/// Memory one lane's ring may hold. Two lanes are live at a boundary.
const LANE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// How far the clock may run past a lane's newest frame before the lane is
/// restarted ahead of it.
const RESYNC_AFTER: Duration = Duration::from_secs(1);

/// The least time between two restarts of one lane.
const RESYNC_INTERVAL: Duration = Duration::from_secs(2);

/// How far ahead of the clock a restarted lane aims.
const RESYNC_LEAD_SECONDS: f64 = 0.3;

pub(crate) struct Lane {
    pub segment: usize,
    pub generation: u64,
    pub ring: Arc<FrameRing>,
    decoder: Option<VideoDecoder>,
    last_resync: Option<Instant>,
}

impl std::fmt::Debug for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lane")
            .field("segment", &self.segment)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// Totals over every lane, including retired ones.
#[derive(Debug, Default, Clone, Copy)]
struct Retired {
    decoded: u64,
    dropped: u64,
    presented: u64,
}

/// The live lanes, and what they have done.
#[derive(Debug)]
pub(crate) struct Lanes {
    orchestrator: Orchestrator,
    max_width: u32,
    max_height: u32,
    live: Vec<Lane>,
    retired: Retired,
    pub resyncs: u32,
    pub error: Option<DecodeError>,
}

impl Lanes {
    pub(crate) fn new(orchestrator: Orchestrator, max_width: u32, max_height: u32) -> Self {
        Self {
            orchestrator,
            max_width,
            max_height,
            live: Vec::new(),
            retired: Retired::default(),
            resyncs: 0,
            error: None,
        }
    }

    /// The lane for segment `index` in `generation`, started so that `pts`
    /// is the first frame it delivers — the frame on screen at `pts` — if
    /// there is none yet. `None` for a segment without video.
    pub(crate) fn lane(
        &mut self,
        plan: &PlaybackPlan,
        index: usize,
        generation: u64,
        pts: i64,
    ) -> Option<&mut Lane> {
        let exists = self
            .live
            .iter()
            .any(|lane| lane.segment == index && lane.generation == generation);
        if !exists {
            let segment = plan.segment(index)?;
            let lane = self.start(segment, index, generation, pts)?;
            self.live.push(lane);
        }
        self.live
            .iter_mut()
            .find(|lane| lane.segment == index && lane.generation == generation)
    }

    fn start(
        &mut self,
        segment: &Segment,
        index: usize,
        generation: u64,
        pts: i64,
    ) -> Option<Lane> {
        let video = segment.source.video.as_ref()?;
        let size = FrameSize::fit(&video.info, self.max_width, self.max_height);
        let ring = Arc::new(FrameRing::new(FrameRing::capacity_for(
            size.bytes(),
            LANE_BUDGET_BYTES,
        )));
        let index_of = &segment.source.index;
        // The frame on screen at `pts`, so that is the first one delivered.
        let first = index_of
            .frame_at_or_before(video.index, pts)
            .ok()
            .flatten()
            .unwrap_or(pts);
        let decoder = self.decode(segment, first, size, &ring);
        Some(Lane {
            segment: index,
            generation,
            ring,
            decoder,
            last_resync: None,
        })
    }

    fn decode(
        &mut self,
        segment: &Segment,
        first: i64,
        size: FrameSize,
        ring: &Arc<FrameRing>,
    ) -> Option<VideoDecoder> {
        let video = segment.source.video.as_ref()?;
        let seek_to = match seek_point(segment, video.index, first) {
            Ok(seek) => seek,
            Err(error) => {
                self.error = Some(error);
                return None;
            }
        };
        let request = DecodeRequest {
            source: segment.source.path.clone(),
            stream: video.index,
            time_base: video.time_base,
            seek_to,
            first_pts: first,
            size,
            max_frames: None,
        };
        Some(VideoDecoder::start(
            &self.orchestrator,
            &request,
            Arc::clone(ring),
        ))
    }

    /// Drop every lane not serving `generation`, or serving a segment before
    /// `segment` in it.
    pub(crate) fn retain(&mut self, generation: u64, segment: Option<usize>) {
        let (keep, retire): (Vec<_>, Vec<_>) = self.live.drain(..).partition(|lane| {
            lane.generation > generation
                || (lane.generation == generation && segment.is_none_or(|s| lane.segment >= s))
        });
        self.live = keep;
        for lane in retire {
            self.retire(lane);
        }
    }

    /// Drop every lane.
    pub(crate) fn clear(&mut self) {
        let lanes = std::mem::take(&mut self.live);
        for lane in lanes {
            self.retire(lane);
        }
    }

    fn retire(&mut self, mut lane: Lane) {
        lane.ring.close();
        if let Some(decoder) = lane.decoder.take() {
            decoder.stop();
        }
        let stats = lane.ring.stats();
        self.retired.decoded += stats.decoded_frames;
        self.retired.dropped += stats.dropped_frames;
        self.retired.presented += stats.presented_frames;
    }

    /// Record a failed decode, and restart a lane ahead of a clock it has
    /// fallen more than [`RESYNC_AFTER`] behind.
    pub(crate) fn keep_up(
        &mut self,
        plan: &PlaybackPlan,
        index: usize,
        generation: u64,
        now_pts: i64,
    ) {
        let Some(position) = self
            .live
            .iter()
            .position(|lane| lane.segment == index && lane.generation == generation)
        else {
            return;
        };
        let Some(segment) = plan.segment(index) else {
            return;
        };
        let Some(video) = segment.source.video.as_ref() else {
            return;
        };
        let (end, head) = match self
            .live
            .get(position)
            .and_then(|lane| lane.decoder.as_ref())
        {
            Some(decoder) => (decoder.end(), decoder.head()),
            None => return,
        };
        if let Some(DecodeEnd::Failed(error)) = end {
            self.error = Some(error);
            return;
        }
        let Some(head) = head.filter(|_| end.is_none()) else {
            return;
        };
        let behind = time::seconds(now_pts.saturating_sub(head), video.time_base);
        if behind < RESYNC_AFTER.as_secs_f64() {
            return;
        }
        let recently = self
            .live
            .get(position)
            .and_then(|lane| lane.last_resync)
            .is_some_and(|at| at.elapsed() < RESYNC_INTERVAL);
        if recently {
            return;
        }
        let lead = time::from_seconds(RESYNC_LEAD_SECONDS, video.time_base, Rounding::Nearest);
        let Ok(Some(keyframe)) = segment
            .source
            .index
            .at_or_after(video.index, now_pts.saturating_add(lead))
        else {
            return;
        };
        let size = FrameSize::fit(&video.info, self.max_width, self.max_height);
        let Some(ring) = self.live.get(position).map(|lane| Arc::clone(&lane.ring)) else {
            return;
        };
        if let Some(old) = self
            .live
            .get_mut(position)
            .and_then(|lane| lane.decoder.take())
        {
            old.stop();
        }
        ring.reset();
        let decoder = self.decode(segment, keyframe.pts, size, &ring);
        if let Some(lane) = self.live.get_mut(position) {
            lane.decoder = decoder;
            lane.last_resync = Some(Instant::now());
        }
        self.resyncs += 1;
    }

    /// The process ids of every running decoder, for teardown checks.
    pub(crate) fn pids(&self) -> Vec<u32> {
        self.live
            .iter()
            .filter_map(|lane| lane.decoder.as_ref().and_then(VideoDecoder::pid))
            .collect()
    }

    /// Statistics over every lane, with the buffer figures of the lane for
    /// `segment` in `generation`.
    pub(crate) fn stats(
        &self,
        segment: Option<usize>,
        generation: u64,
    ) -> (RingStats, u64, u64, u64) {
        let current = self
            .live
            .iter()
            .find(|lane| Some(lane.segment) == segment && lane.generation == generation)
            .map(|lane| lane.ring.stats());
        let live = self.live.iter().map(|lane| lane.ring.stats());
        let (mut decoded, mut dropped, mut presented) = (
            self.retired.decoded,
            self.retired.dropped,
            self.retired.presented,
        );
        for stats in live {
            decoded += stats.decoded_frames;
            dropped += stats.dropped_frames;
            presented += stats.presented_frames;
        }
        let current = current.unwrap_or(RingStats {
            buffered_frames: 0,
            capacity_frames: 0,
            buffered_bytes: 0,
            decoded_frames: 0,
            dropped_frames: 0,
            presented_frames: 0,
            decode_fps: 0.0,
            finished: false,
        });
        (current, decoded, dropped, presented)
    }
}

/// The keyframe to seek to for `pts`, or `None` when decoding from the start
/// of the stream reaches it as well.
fn seek_point(segment: &Segment, stream: u32, pts: i64) -> Result<Option<i64>, DecodeError> {
    let index = &segment.source.index;
    let failed = |error: crate::keyframes::IndexError| DecodeError::Failed(error.to_string());
    let Some(keyframe) = index.at_or_before(stream, pts).map_err(failed)? else {
        return Ok(None);
    };
    let earlier = index
        .at_or_before(stream, keyframe.pts.saturating_sub(1))
        .map_err(failed)?;
    Ok(earlier.map(|_| keyframe.pts))
}
