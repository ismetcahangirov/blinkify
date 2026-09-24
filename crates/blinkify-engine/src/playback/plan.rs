//! What the player plays: an ordered list of segments of sources, placed on a
//! timeline.
//!
//! The player does not read the edit graph. The shared evaluator (#30) turns
//! the graph into a [`Timeline`], [`PlaybackPlan::from_timeline`] turns that
//! into segments, and the plan is all the player knows — so there is exactly
//! one interpretation of the graph, and preview and export cannot disagree
//! about what it means. A plan can also be built directly: one segment for a
//! whole file, or several in tests.
//!
//! Timeline time ([`ProgramTime`]) is microseconds. A source range is kept in
//! the source's own time base, and the conversions between the two round in
//! opposite directions — a frame's place on the timeline up, a timeline
//! position's source tick down — so that a frame survives the round trip (see
//! `crate::time`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::keyframes::KeyframeIndex;
use crate::probe::{MediaInfo, Rational, StreamInfo, VideoInfo};
use crate::project::evaluate::{AudioOperation, Placement, Timeline};
use crate::project::{ClipId, SourceId, TrackKind};
use crate::proxy::Proxy;
use crate::time::{self, MICROSECONDS, Rounding};

/// The plan a player plays, shared with its workers and replaced whole when
/// the edit graph changes.
pub(crate) type PlanSlot = Arc<std::sync::Mutex<Arc<PlaybackPlan>>>;

/// Whether two segments play the same thing at the same place, so a decoder
/// for one serves the other.
pub(crate) fn same_segment(a: &Segment, b: &Segment) -> bool {
    Arc::ptr_eq(&a.source, &b.source)
        && a.source_in == b.source_in
        && a.source_out == b.source_out
        && a.timeline_start == b.timeline_start
        && a.speed == b.speed
}

/// A position on the playback timeline, in microseconds from its start.
pub type ProgramTime = i64;

/// A track of the timeline, for monitoring: solo and mute are per track.
pub type TrackId = u32;

/// The track the main segments — the pictures and their own sound — are on.
pub const MAIN_TRACK: TrackId = 0;

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
    #[error("track {0} is used twice")]
    DuplicateTrack(TrackId),
    #[error("clip {0} cannot be placed on the playback timeline")]
    Unplaceable(ClipId),
}

/// How far two segments may overlap and still be a seam: the rounding of two
/// exact frame positions to whole microseconds (see
/// [`PlaybackPlan::from_timeline`]). The later segment wins the overlap.
const SEAM_TOLERANCE: ProgramTime = 2;

const NORMAL: Rational = Rational { num: 1, den: 1 };

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
    /// How fast the source plays: `2/1` is double speed. From the edit graph,
    /// not the transport — the transport's speed is on top of it.
    pub speed: Rational,
    /// The clip's audio chain, as the evaluator resolved it.
    pub audio: Vec<AudioOperation>,
    /// The clip the segment plays, when it came from the edit graph.
    pub clip: Option<ClipId>,
}

impl Segment {
    /// A segment at normal speed with no audio chain.
    #[must_use]
    pub fn new(
        source: Arc<SourceMedia>,
        source_in: i64,
        source_out: i64,
        timeline_start: ProgramTime,
    ) -> Self {
        Self {
            source,
            source_in,
            source_out,
            timeline_start,
            speed: NORMAL,
            audio: Vec::new(),
            clip: None,
        }
    }

    #[must_use]
    pub fn time_base(&self) -> Rational {
        self.source.time_base()
    }

    /// The time base in which one source tick is one tick of timeline time:
    /// the source's, stretched by the speed.
    #[must_use]
    pub fn played_time_base(&self) -> Rational {
        let time_base = self.time_base();
        Rational {
            num: time_base.num.saturating_mul(self.speed.den),
            den: time_base.den.saturating_mul(self.speed.num),
        }
    }

    /// The speed as a factor, for the audio decoder's tempo.
    #[must_use]
    pub fn speed_factor(&self) -> f64 {
        self.speed
            .value()
            .filter(|speed| *speed > 0.0)
            .unwrap_or(1.0)
    }

    /// The segment's length on the timeline.
    #[must_use]
    pub fn duration(&self) -> ProgramTime {
        time::rescale(
            self.source_out.saturating_sub(self.source_in),
            self.played_time_base(),
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
            time::rescale(
                offset,
                MICROSECONDS,
                self.played_time_base(),
                Rounding::Down,
            )
            .unwrap_or(0),
        )
    }

    /// Where the frame at source tick `pts` starts on the timeline, rounded
    /// up, so that [`Segment::source_at`] of the result is `pts` again. A
    /// frame that began before the segment's in point starts at the segment.
    #[must_use]
    pub fn program_at(&self, pts: i64) -> ProgramTime {
        let offset = pts.saturating_sub(self.source_in).max(0);
        self.timeline_start.saturating_add(
            time::rescale(offset, self.played_time_base(), MICROSECONDS, Rounding::Up).unwrap_or(0),
        )
    }

    /// Source seconds at timeline position `t`, for the audio decoder.
    #[must_use]
    pub fn source_seconds_at(&self, t: ProgramTime) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let offset = t.saturating_sub(self.timeline_start) as f64 / 1_000_000.0;
        time::seconds(self.source_in, self.time_base()) + offset * self.speed_factor()
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
    /// Further tracks of sound only — detached audio, music — mixed with the
    /// main segments' own.
    audio_tracks: Vec<AudioTrack>,
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
        check(&segments)?;
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
            audio_tracks: Vec::new(),
            frame_rate,
        })
    }

    /// Add a track of sound only.
    ///
    /// # Errors
    ///
    /// Its segments overlap or run backwards, or its id is taken.
    pub fn with_audio_track(mut self, track: AudioTrack) -> Result<Self, PlanError> {
        if track.id == MAIN_TRACK || self.audio_tracks.iter().any(|t| t.id == track.id) {
            return Err(PlanError::DuplicateTrack(track.id));
        }
        check(&track.segments)?;
        self.audio_tracks.push(track);
        Ok(self)
    }

    /// Every track that carries sound, main first, as `(id, segments)`.
    #[must_use]
    pub fn sound_tracks(&self) -> Vec<(TrackId, &[Segment])> {
        std::iter::once((MAIN_TRACK, self.segments.as_slice()))
            .chain(
                self.audio_tracks
                    .iter()
                    .map(|track| (track.id, track.segments.as_slice())),
            )
            .collect()
    }

    /// The preview of an evaluated edit graph (#30).
    ///
    /// The pictures, and their own sound, come from the first video track;
    /// every audio track is a sound-only track with the project's track id.
    /// A further video track is not composited — Blinkify is not a
    /// compositor — so it is not previewed. A clip whose source is not in
    /// `sources` (offline, #32) leaves a gap: black and silence.
    ///
    /// `sources` decides the preview's *quality* — a source may carry a
    /// proxy. The *content* is the timeline's, which is what the export
    /// uses too: the same clips, ranges, speeds and chains.
    ///
    /// Frame positions become microseconds with the segment start rounded
    /// down and a seek target rounded up, so the source tick the preview
    /// asks for is never before the one the evaluator gives for that frame;
    /// two seams rounded that way may overlap by `SEAM_TOLERANCE`.
    ///
    /// # Errors
    ///
    /// Nothing to play, or a clip whose timing does not fit the playback
    /// timeline.
    pub fn from_timeline(
        timeline: &Timeline,
        sources: &BTreeMap<SourceId, Arc<SourceMedia>>,
    ) -> Result<Self, PlanError> {
        let segments_of = |placements: &[Placement]| -> Result<Vec<Segment>, PlanError> {
            placements
                .iter()
                // A held or reversed clip (#35) is decoded and re-encoded by
                // the full re-encode executor (#55), which does not exist
                // yet; the preview shows it as a gap rather than pretend, and
                // the diagnostic view says it is not previewed.
                .filter(|placement| placement.motion.is_none())
                .filter_map(|placement| {
                    let source = sources.get(&placement.source)?;
                    Some(segment_of(placement, source, timeline.time_base))
                })
                .collect()
        };
        let main = timeline
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Video)
            .map(|track| segments_of(&track.placements))
            .transpose()?
            .unwrap_or_default();
        let mut plan = if main.is_empty() {
            Self {
                segments: Vec::new(),
                audio_tracks: Vec::new(),
                frame_rate: timeline.frame_rate,
            }
        } else {
            check(&main)?;
            Self {
                segments: main,
                audio_tracks: Vec::new(),
                frame_rate: timeline.frame_rate,
            }
        };
        for track in timeline
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
        {
            plan = plan.with_audio_track(AudioTrack {
                id: track.id,
                segments: segments_of(&track.placements)?,
            })?;
        }
        if plan.segments.is_empty() && plan.audio_tracks.iter().all(|t| t.segments.is_empty()) {
            return Err(PlanError::Empty);
        }
        Ok(plan)
    }

    /// A whole file from its first frame to its end.
    ///
    /// # Errors
    ///
    /// As [`PlaybackPlan::new`].
    pub fn whole(source: Arc<SourceMedia>) -> Result<Self, PlanError> {
        let (source_in, source_out) = source.full_range();
        Self::new(vec![Segment::new(source, source_in, source_out, 0)])
    }

    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// The end of the last segment on any track.
    #[must_use]
    pub fn duration(&self) -> ProgramTime {
        self.sound_tracks()
            .iter()
            .filter_map(|(_, segments)| segments.last().map(Segment::timeline_end))
            .max()
            .unwrap_or(0)
    }

    /// The main segment playing at `t`, if `t` is not in a gap.
    #[must_use]
    pub fn segment_at(&self, t: ProgramTime) -> Option<(usize, &Segment)> {
        segment_at(&self.segments, t)
    }

    /// The first main segment starting after `t`.
    #[must_use]
    pub fn next_segment_after(&self, t: ProgramTime) -> Option<(usize, &Segment)> {
        let i = self.segments.partition_point(|s| s.timeline_start <= t);
        self.segments.get(i).map(|segment| (i, segment))
    }

    #[must_use]
    pub fn segment(&self, i: usize) -> Option<&Segment> {
        self.segments.get(i)
    }

    /// The next timeline position after `t` where what plays changes on any
    /// track: a segment's end, or the start of the next one after a gap.
    #[must_use]
    pub fn next_boundary(&self, t: ProgramTime) -> ProgramTime {
        let end = self.duration();
        self.sound_tracks()
            .iter()
            .map(|(_, segments)| next_boundary(segments, t, end))
            .min()
            .unwrap_or(end)
    }
}

/// The segment playing `placement` from `source`.
fn segment_of(
    placement: &Placement,
    source: &Arc<SourceMedia>,
    sequence: Rational,
) -> Result<Segment, PlanError> {
    let unplaceable = || PlanError::Unplaceable(placement.clip);
    let time_base = source.time_base();
    // The clip's stream time base and the segment's (the source's video)
    // are the same for a video clip; an audio clip of a file with video is
    // converted, outward, to whole ticks of the video's.
    let source_in = time::rescale(
        placement.source_in,
        placement.time_base,
        time_base,
        Rounding::Down,
    )
    .ok_or_else(unplaceable)?;
    let source_out = time::rescale(
        placement.source_out,
        placement.time_base,
        time_base,
        Rounding::Up,
    )
    .ok_or_else(unplaceable)?;
    let timeline_start = time::rescale(placement.start, sequence, MICROSECONDS, Rounding::Down)
        .ok_or_else(unplaceable)?;
    Ok(Segment {
        source: Arc::clone(source),
        source_in,
        source_out,
        timeline_start,
        speed: placement.speed,
        audio: placement.audio.clone(),
        clip: Some(placement.clip),
    })
}

/// A track of sound only.
#[derive(Debug, Clone)]
pub struct AudioTrack {
    pub id: TrackId,
    pub segments: Vec<Segment>,
}

/// Segments in order, none overlapping, none backwards.
fn check(segments: &[Segment]) -> Result<(), PlanError> {
    for (i, segment) in segments.iter().enumerate() {
        if segment.source_out <= segment.source_in {
            return Err(PlanError::Inverted(i));
        }
        if i > 0
            && segments.get(i - 1).is_some_and(|previous| {
                segment.timeline_start + SEAM_TOLERANCE < previous.timeline_end()
            })
        {
            return Err(PlanError::Overlap(i));
        }
    }
    Ok(())
}

/// The segment of `segments` playing at `t`, if `t` is not in a gap.
#[must_use]
pub fn segment_at(segments: &[Segment], t: ProgramTime) -> Option<(usize, &Segment)> {
    let after = segments.partition_point(|s| s.timeline_start <= t);
    let i = after.checked_sub(1)?;
    let segment = segments.get(i)?;
    segment.contains(t).then_some((i, segment))
}

/// The next position after `t` where what `segments` plays changes, or `end`.
#[must_use]
pub fn next_boundary(segments: &[Segment], t: ProgramTime, end: ProgramTime) -> ProgramTime {
    if let Some((_, segment)) = segment_at(segments, t) {
        return segment.timeline_end();
    }
    let i = segments.partition_point(|s| s.timeline_start <= t);
    segments.get(i).map_or(end, |s| s.timeline_start)
}
