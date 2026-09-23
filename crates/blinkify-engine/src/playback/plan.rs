//! What the player plays: an ordered list of segments of sources, placed on a
//! timeline.
//!
//! The player does not read the edit graph. The shared evaluator (#30) turns
//! the graph into a plan, and the plan is all the player knows — so there is
//! exactly one interpretation of the graph, and preview and export cannot
//! disagree about what it means. Until #30 exists, a plan is built directly:
//! one segment for a whole file, or several in tests.
//!
//! Timeline time ([`ProgramTime`]) is microseconds. A source range is kept in
//! the source's own time base, and the conversions between the two round in
//! opposite directions — a frame's place on the timeline up, a timeline
//! position's source tick down — so that a frame survives the round trip (see
//! `crate::time`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::keyframes::KeyframeIndex;
use crate::probe::{MediaInfo, Rational, StreamInfo, VideoInfo};
use crate::proxy::Proxy;
use crate::time::{self, MICROSECONDS, Rounding};

/// A position on the playback timeline, in microseconds from its start.
pub type ProgramTime = i64;

/// The frame rate timecode is counted in when a plan has no video.
const DEFAULT_FRAME_RATE: Rational = Rational { num: 30, den: 1 };

/// A source file, probed and indexed, with the streams the preview uses.
#[derive(Debug)]
pub struct SourceMedia {
    pub path: PathBuf,
    pub info: Arc<MediaInfo>,
    pub index: Arc<KeyframeIndex>,
    pub video: Option<VideoStream>,
    pub audio: Option<AudioStream>,
    /// A preview proxy to decode pictures from instead of the file. Preview
    /// only: an export takes an `ExportSource`, which cannot hold one.
    pub proxy: Option<Proxy>,
}

/// The video stream a preview shows.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub index: u32,
    pub time_base: Rational,
    /// Where its first frame is, in seconds of the file's timeline.
    pub start_seconds: f64,
    pub info: VideoInfo,
}

/// The audio stream a preview plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioStream {
    pub index: u32,
    pub time_base: Rational,
    /// Where its first sample is, in seconds of the file's timeline.
    pub start_seconds: f64,
}

/// Why a source or a plan cannot be played.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("{} has no audio or video to play", .0.display())]
    NothingToPlay(PathBuf),
    #[error("the timeline is empty")]
    Empty,
    #[error("segment {0} overlaps the one before it")]
    Overlap(usize),
    #[error("segment {0} ends before it starts")]
    Inverted(usize),
}

impl SourceMedia {
    /// The default video stream (never a cover image) and the default audio
    /// stream of a file.
    ///
    /// # Errors
    ///
    /// The file has neither.
    pub fn new(
        path: &Path,
        info: Arc<MediaInfo>,
        index: Arc<KeyframeIndex>,
    ) -> Result<Self, PlanError> {
        let video =
            pick(info.video().filter(|(_, v)| !v.is_attached_picture)).map(|(stream, video)| {
                VideoStream {
                    index: stream.index,
                    time_base: valid(stream.time_base),
                    start_seconds: stream.start_seconds.unwrap_or(0.0),
                    info: video.clone(),
                }
            });
        let audio = pick(info.audio()).map(|(stream, _)| AudioStream {
            index: stream.index,
            time_base: valid(stream.time_base),
            start_seconds: stream.start_seconds.unwrap_or(0.0),
        });
        if video.is_none() && audio.is_none() {
            return Err(PlanError::NothingToPlay(path.to_path_buf()));
        }
        Ok(Self {
            path: path.to_path_buf(),
            info,
            index,
            video,
            audio,
            proxy: None,
        })
    }

    /// Preview pictures from `proxy`. Sound, timing and every frame decision
    /// still come from the file; the proxy only supplies the pixels.
    #[must_use]
    pub fn with_proxy(mut self, proxy: Option<Proxy>) -> Self {
        self.proxy = proxy;
        self
    }

    /// The time base a segment of this source is measured in: the video's,
    /// so that frames are exact, or the audio's when there is no video.
    #[must_use]
    pub fn time_base(&self) -> Rational {
        self.video
            .as_ref()
            .map(|video| video.time_base)
            .or(self.audio.map(|audio| audio.time_base))
            .unwrap_or(MICROSECONDS)
    }

    /// The whole file, as a source range `[in, out)` in [`Self::time_base`].
    #[must_use]
    pub fn full_range(&self) -> (i64, i64) {
        let time_base = self.time_base();
        let stream = self.reference_stream();
        let start = stream.and_then(|s| s.start_seconds).unwrap_or(0.0);
        let duration = stream
            .and_then(|s| s.duration_seconds)
            .or(self.info.container.duration_seconds)
            .unwrap_or(0.0);
        let first = time::from_seconds(start, time_base, Rounding::Nearest);
        let length = time::from_seconds(duration, time_base, Rounding::Up);
        (first, first.saturating_add(length.max(1)))
    }

    fn reference_stream(&self) -> Option<&StreamInfo> {
        let index = self
            .video
            .as_ref()
            .map(|video| video.index)
            .or(self.audio.map(|audio| audio.index))?;
        self.info
            .streams
            .iter()
            .find(|stream| stream.index == index)
    }
}

fn valid(time_base: Option<Rational>) -> Rational {
    time_base
        .filter(|tb| tb.num > 0 && tb.den > 0)
        .unwrap_or(MICROSECONDS)
}

/// The default-flagged stream if there is one, otherwise the first.
fn pick<'a, T>(streams: impl Iterator<Item = (&'a StreamInfo, T)>) -> Option<(&'a StreamInfo, T)> {
    let streams: Vec<_> = streams.collect();
    let default = streams.iter().position(|(stream, _)| stream.is_default);
    streams.into_iter().nth(default.unwrap_or(0))
}

/// One contiguous piece of one source, placed on the timeline.
#[derive(Debug, Clone)]
pub struct Segment {
    pub source: Arc<SourceMedia>,
    /// The first source tick shown, in the source's time base.
    pub source_in: i64,
    /// The first source tick not shown.
    pub source_out: i64,
    /// Where the segment starts on the timeline.
    pub timeline_start: ProgramTime,
}

impl Segment {
    #[must_use]
    pub fn time_base(&self) -> Rational {
        self.source.time_base()
    }

    /// The segment's length on the timeline.
    #[must_use]
    pub fn duration(&self) -> ProgramTime {
        time::rescale(
            self.source_out.saturating_sub(self.source_in),
            self.time_base(),
            MICROSECONDS,
            Rounding::Up,
        )
        .unwrap_or(0)
    }

    #[must_use]
    pub fn timeline_end(&self) -> ProgramTime {
        self.timeline_start.saturating_add(self.duration())
    }

    /// The source tick shown at timeline position `t`, rounded down: the
    /// frame on screen is the newest one at or before it.
    #[must_use]
    pub fn source_at(&self, t: ProgramTime) -> i64 {
        let offset = t.saturating_sub(self.timeline_start);
        self.source_in.saturating_add(
            time::rescale(offset, MICROSECONDS, self.time_base(), Rounding::Down).unwrap_or(0),
        )
    }

    /// Where the frame at source tick `pts` starts on the timeline, rounded
    /// up, so that [`Segment::source_at`] of the result is `pts` again. A
    /// frame that began before the segment's in point starts at the segment.
    #[must_use]
    pub fn program_at(&self, pts: i64) -> ProgramTime {
        let offset = pts.saturating_sub(self.source_in).max(0);
        self.timeline_start.saturating_add(
            time::rescale(offset, self.time_base(), MICROSECONDS, Rounding::Up).unwrap_or(0),
        )
    }

    /// Source seconds at timeline position `t`, for the audio decoder.
    #[must_use]
    pub fn source_seconds_at(&self, t: ProgramTime) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let offset = t.saturating_sub(self.timeline_start) as f64 / 1_000_000.0;
        time::seconds(self.source_in, self.time_base()) + offset
    }

    #[must_use]
    pub fn contains(&self, t: ProgramTime) -> bool {
        self.timeline_start <= t && t < self.timeline_end()
    }
}

/// Everything the player plays, in timeline order.
#[derive(Debug, Clone)]
pub struct PlaybackPlan {
    segments: Vec<Segment>,
    /// The rate timecode is counted in.
    pub frame_rate: Rational,
}

impl PlaybackPlan {
    /// Segments in timeline order, gaps allowed, overlaps not. A later build
    /// takes its frame rate from the sequence settings (#57); until then it is
    /// the first video source's.
    ///
    /// # Errors
    ///
    /// No segments, or segments that overlap or run backwards.
    pub fn new(segments: Vec<Segment>) -> Result<Self, PlanError> {
        if segments.is_empty() {
            return Err(PlanError::Empty);
        }
        for (i, segment) in segments.iter().enumerate() {
            if segment.source_out <= segment.source_in {
                return Err(PlanError::Inverted(i));
            }
            if i > 0
                && segments
                    .get(i - 1)
                    .is_some_and(|previous| segment.timeline_start < previous.timeline_end())
            {
                return Err(PlanError::Overlap(i));
            }
        }
        let frame_rate = segments
            .iter()
            .find_map(|segment| segment.source.video.as_ref())
            .and_then(|video| {
                video
                    .info
                    .frame_rate
                    .average
                    .or(video.info.frame_rate.real_base)
            })
            .filter(|rate| rate.num > 0 && rate.den > 0)
            .unwrap_or(DEFAULT_FRAME_RATE);
        Ok(Self {
            segments,
            frame_rate,
        })
    }

    /// A whole file from its first frame to its end.
    ///
    /// # Errors
    ///
    /// As [`PlaybackPlan::new`].
    pub fn whole(source: Arc<SourceMedia>) -> Result<Self, PlanError> {
        let (source_in, source_out) = source.full_range();
        Self::new(vec![Segment {
            source,
            source_in,
            source_out,
            timeline_start: 0,
        }])
    }

    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The end of the last segment.
    #[must_use]
    pub fn duration(&self) -> ProgramTime {
        self.segments.last().map_or(0, Segment::timeline_end)
    }

    /// The segment playing at `t`, if `t` is not in a gap.
    #[must_use]
    pub fn segment_at(&self, t: ProgramTime) -> Option<(usize, &Segment)> {
        let after = self.segments.partition_point(|s| s.timeline_start <= t);
        let i = after.checked_sub(1)?;
        let segment = self.segments.get(i)?;
        segment.contains(t).then_some((i, segment))
    }

    /// The first segment starting after `t`.
    #[must_use]
    pub fn next_segment_after(&self, t: ProgramTime) -> Option<(usize, &Segment)> {
        let i = self.segments.partition_point(|s| s.timeline_start <= t);
        self.segments.get(i).map(|segment| (i, segment))
    }

    #[must_use]
    pub fn segment(&self, i: usize) -> Option<&Segment> {
        self.segments.get(i)
    }

    /// The next timeline position after `t` where what plays changes: a
    /// segment's end, or the start of the next one after a gap.
    #[must_use]
    pub fn next_boundary(&self, t: ProgramTime) -> ProgramTime {
        match self.segment_at(t) {
            Some((_, segment)) => segment.timeline_end(),
            None => self
                .next_segment_after(t)
                .map_or(self.duration(), |(_, s)| s.timeline_start),
        }
    }
}
