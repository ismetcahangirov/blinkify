//! Video lanes: one decoder per segment being shown or about to be.
//!
//! A lane is a decoder and its frame ring for one segment, started at a given
//! source position, serving one clock generation. The lane for the segment
//! playing is the one presented from; the lane for the next segment is
//! started before the boundary so the first frame after it is already
//! decoded — a clip boundary never waits for a process to start.
//!
//! Every decoder is started through [`start_decoder`], which is the two-stage
//! seek of [`crate::seek`] — or, for a source with a proxy, the same decode
//! of the proxy with each frame mapped back to the source frame it shows.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::plan::{PlaybackPlan, Segment, VideoStream, same_segment};
use crate::decode::{
    DecodeEnd, DecodeError, DecodeRequest, FrameRing, FrameSize, PtsMap, RingStats, VideoDecoder,
};
use crate::orchestrator::Orchestrator;
use crate::probe::{Rational, VideoInfo};
use crate::proxy::PROXY_HEIGHT;
use crate::seek;
use crate::time::{self, Rounding};

/// Memory one lane's ring may hold. Two lanes are live at a boundary.
pub(crate) const LANE_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// How far the clock may run past a lane's newest frame before the lane is
/// restarted ahead of it.
const RESYNC_AFTER: Duration = Duration::from_secs(1);

/// The least time between two restarts of one lane.
const RESYNC_INTERVAL: Duration = Duration::from_secs(2);

/// How far ahead of the clock a restarted lane aims.
const RESYNC_LEAD_SECONDS: f64 = 0.3;

/// A proxy's timestamps are Matroska milliseconds.
const PROXY_TIME_BASE: Rational = Rational { num: 1, den: 1000 };

/// Counter-clockwise rotation to draw a segment's frames with: the source's,
/// or none for a proxy, which was made upright.
pub(crate) fn rotation_of(segment: &Segment) -> u32 {
    match (&segment.source.proxy, &segment.source.video) {
        (None, Some(video)) => video.info.rotation,
        _ => 0,
    }
}

/// The size frames of `segment` are decoded at for a `bound` surface.
pub(crate) fn frame_size(segment: &Segment, bound: (u32, u32)) -> Option<FrameSize> {
    let video = segment.source.video.as_ref()?;
    Some(if segment.source.proxy.is_some() {
        proxy_size(&video.info, bound)
    } else {
        FrameSize::fit(&video.info, bound.0, bound.1)
    })
}

/// Start decoding `segment` so that the frame on screen at source tick `pts`
/// is the first delivered, then up to `max_frames` in all, at the size
/// [`frame_size`] gives. From the proxy when the source has one.
///
/// # Errors
///
/// The segment has no video, the index could not be read, or the proxy's
/// segment list could not be written.
pub(crate) fn start_decoder(
    orchestrator: &Orchestrator,
    segment: &Segment,
    pts: i64,
    bound: (u32, u32),
    max_frames: Option<u32>,
    ring: &Arc<FrameRing>,
) -> Result<VideoDecoder, DecodeError> {
    let no_video = || DecodeError::NoVideo(segment.source.path.clone());
    let video = segment.source.video.as_ref().ok_or_else(no_video)?;
    let size = frame_size(segment, bound).ok_or_else(no_video)?;
    let failed = |error: seek::SeekError| DecodeError::Failed(error.to_string());
    let plan = seek::plan(&segment.source.index, video.index, pts).map_err(failed)?;
    if let Some(proxy) = &segment.source.proxy {
        let list = proxy
            .concat_list()
            .map_err(|error| DecodeError::Failed(error.to_string()))?;
        // Where the planned frame sits in the proxy, rounded down half a
        // millisecond so the proxy's own rounding cannot put it after.
        let seconds = time::seconds(plan.frame, video.time_base) - video.start_seconds;
        let first =
            time::from_seconds((seconds - 0.0005).max(0.0), PROXY_TIME_BASE, Rounding::Down);
        let request = DecodeRequest {
            source: list,
            stream: 0,
            time_base: PROXY_TIME_BASE,
            // Every proxy frame is a keyframe: the seek is the frame.
            seek_to: (first > 0).then_some(first),
            first_pts: first,
            size,
            max_frames,
            concat: true,
        };
        return Ok(VideoDecoder::start_mapped(
            orchestrator,
            &request,
            Arc::clone(ring),
            Some(source_frame_of_proxy(segment, video)),
        ));
    }
    let request = plan.request(
        &segment.source.path,
        video.index,
        video.time_base,
        size,
        max_frames,
    );
    Ok(VideoDecoder::start(
        orchestrator,
        &request,
        Arc::clone(ring),
    ))
}

/// A proxy frame's millisecond timestamp to the source frame it pictures: the
/// source frame nearest its time, found in the frame table, so the frame
/// shown is the same source frame whether the preview reads the proxy or the
/// original.
fn source_frame_of_proxy(segment: &Segment, video: &VideoStream) -> PtsMap {
    let index = Arc::clone(&segment.source.index);
    let stream = video.index;
    let time_base = video.time_base;
    let start = video.start_seconds;
    // Half a millisecond: the most the proxy's rounding moved a frame.
    let slack = time::from_seconds(0.0005, time_base, Rounding::Up);
    Arc::new(move |millis| {
        #[allow(clippy::cast_precision_loss)]
        let seconds = millis as f64 / 1000.0 + start;
        let ticks = time::from_seconds(seconds, time_base, Rounding::Nearest);
        index
            .frame_at_or_before(stream, ticks.saturating_add(slack))
            .ok()
            .flatten()
            .unwrap_or(ticks)
    })
}

/// The size to decode a proxy at: the picture upright, as the proxy was
/// made, no taller than the proxy, fitted to the bound.
fn proxy_size(video: &VideoInfo, bound: (u32, u32)) -> FrameSize {
    let upright = VideoInfo {
        width: video.display_width,
        height: video.display_height,
        rotation: 0,
        sample_aspect_ratio: None,
        ..video.clone()
    };
    let height = if bound.1 == 0 {
        PROXY_HEIGHT
    } else {
        bound.1.min(PROXY_HEIGHT)
    };
    FrameSize::fit(&upright, bound.0, height)
}

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
    pub bound: (u32, u32),
    live: Vec<Lane>,
    retired: Retired,
    pub resyncs: u32,
    pub error: Option<DecodeError>,
}

impl Lanes {
    pub(crate) fn new(orchestrator: Orchestrator, max_width: u32, max_height: u32) -> Self {
        Self {
            orchestrator,
            bound: (max_width, max_height),
            live: Vec::new(),
            retired: Retired::default(),
            resyncs: 0,
            error: None,
        }
    }

    /// The lane for segment `index` in `generation`, started so that the
    /// frame on screen at `pts` is the first it delivers, if there is none
    /// yet. `None` for a segment without video.
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
            let size = frame_size(segment, self.bound)?;
            let ring = Arc::new(FrameRing::new(FrameRing::capacity_for(
                size.bytes(),
                LANE_BUDGET_BYTES,
            )));
            let decoder =
                match start_decoder(&self.orchestrator, segment, pts, self.bound, None, &ring) {
                    Ok(decoder) => Some(decoder),
                    Err(error) => {
                        self.error = Some(error);
                        None
                    }
                };
            self.live.push(Lane {
                segment: index,
                generation,
                ring,
                decoder,
                last_resync: None,
            });
        }
        self.live
            .iter_mut()
            .find(|lane| lane.segment == index && lane.generation == generation)
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

    /// Carry the lanes of `generation` over to a new plan: a lane whose
    /// segment is unchanged in `new` keeps its decoder under its new index;
    /// every other lane is dropped.
    pub(crate) fn remap(&mut self, old: &PlaybackPlan, new: &PlaybackPlan, generation: u64) {
        let lanes = std::mem::take(&mut self.live);
        for mut lane in lanes {
            let moved = (lane.generation == generation)
                .then(|| old.segment(lane.segment))
                .flatten()
                .and_then(|before| {
                    new.segments()
                        .iter()
                        .position(|after| same_segment(before, after))
                });
            match moved {
                Some(index) => {
                    lane.segment = index;
                    self.live.push(lane);
                }
                None => self.retire(lane),
            }
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
        let decoder = match start_decoder(
            &self.orchestrator,
            segment,
            keyframe.pts,
            self.bound,
            None,
            &ring,
        ) {
            Ok(decoder) => Some(decoder),
            Err(error) => {
                self.error = Some(error);
                None
            }
        };
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
