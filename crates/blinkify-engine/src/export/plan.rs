//! The export planner (#39): the evaluated graph, compiled into segments, each
//! one decided — copied, smart-cut or re-encoded — with every reason as data.
//!
//! This is the question the editors Blinkify competes with never ask. Its
//! answer is the export: the executor (#40, #41, #55) does exactly what the
//! plan says, the export dialog (#50) shows it before anything runs, and the
//! report (#52) states it afterwards.
//!
//! What the planner holds to:
//!
//! - **It reads, it does not re-derive.** The graph comes from the shared
//!   evaluator (#30); copy eligibility from [`copy_eligibility`] (ADR-0008);
//!   a speed's effect from [`speed::verdict`] (ADR-0009); a hold or a
//!   reverse from the placement's recorded reason. Three copies of a rule
//!   become three rules.
//! - **Every precondition is its own [`Cause`].** Keyframe alignment, the
//!   sequence's shape, the speed, the operations, and whether the copied
//!   sources share their encoding parameters are reported separately, and a
//!   segment carries all of them, not only the first.
//! - **Video and audio are planned apart.** A gain change re-encodes the
//!   sound and leaves the pictures a copy — the single most valuable case in
//!   the product, and the easiest to lose by planning one segment per clip.
//! - **When it cannot prove a copy safe, it does not copy.** An unindexed
//!   cut point re-encodes and says why. A wrong copy is a broken file; a
//!   conservative re-encode is a correct file and an honest report.
//! - **It is pure and cheap**, so the interface can plan on every graph
//!   change: facts in, plan out, no file, no process, no clock.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::profile::{EncoderChoice, Unmatched};
use crate::probe::Rational;
use crate::project::evaluate::{AudioOperation, Motion, Placement, Timeline, crop};
use crate::project::settings::{Mismatch, SequenceSettings, StreamGeometry, copy_eligibility};
use crate::project::speed;
use crate::project::{ClipId, SourceId, TrackId, TrackKind};
use crate::tier::{ExportTier, ReEncodeReason, SeamReason};
use crate::time::{Rounding, rescale};

/// A keyframe, as far as the planner needs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyframePoint {
    /// Presentation timestamp, in the stream's time base.
    #[ts(type = "number")]
    pub pts: i64,
    /// Pictures shown before it are decoded after it — the leading pictures
    /// of an open GOP, or HEVC's decodable RADL pictures. A copy may *start*
    /// here, since those pictures are before the cut and are dropped, but a
    /// copy may not *end* here: the pictures just before the cut would need
    /// the packets after it.
    pub open: bool,
    /// A decoder can start here and decode everything after it cleanly. Not
    /// so for an H.264 recovery-point picture (a non-IDR keyframe): the
    /// pictures after it carry reference commands for pictures before it,
    /// and a stream that starts there decodes with errors. A copy starts only
    /// on a random-access keyframe.
    pub random_access: bool,
}

/// The parameters a decoder carries across a join. Two sources whose
/// copied packets meet in one output stream must agree on every one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EncodingSignature {
    pub codec: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i32>,
    pub pixel_format: Option<String>,
    /// Coded size, for video.
    pub width: u32,
    pub height: u32,
    /// Primaries, transfer, matrix and range, as FFmpeg names them.
    pub colour: [Option<String>; 4],
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// The hash of the codec configuration record: the SPS and PPS.
    pub configuration: Option<String>,
}

/// One parameter two signatures disagree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum EncodingField {
    Codec,
    Profile,
    Level,
    PixelFormat,
    Resolution,
    Colour,
    SampleRate,
    Channels,
    /// The codec configuration — the SPS and PPS — differs even though
    /// everything above agrees.
    Configuration,
}

impl EncodingSignature {
    /// Every field on which `self` and `other` disagree, in a fixed order.
    #[must_use]
    pub fn differences(&self, other: &Self) -> Vec<EncodingField> {
        let mut fields = Vec::new();
        let checks = [
            (self.codec != other.codec, EncodingField::Codec),
            (self.profile != other.profile, EncodingField::Profile),
            (self.level != other.level, EncodingField::Level),
            (
                self.pixel_format != other.pixel_format,
                EncodingField::PixelFormat,
            ),
            (
                (self.width, self.height) != (other.width, other.height),
                EncodingField::Resolution,
            ),
            (self.colour != other.colour, EncodingField::Colour),
            (
                self.sample_rate != other.sample_rate,
                EncodingField::SampleRate,
            ),
            (self.channels != other.channels, EncodingField::Channels),
        ];
        fields.extend(
            checks
                .into_iter()
                .filter(|(differs, _)| *differs)
                .map(|(_, field)| field),
        );
        if fields.is_empty() && self.configuration != other.configuration {
            fields.push(EncodingField::Configuration);
        }
        fields
    }
}

/// What the planner knows about a source's video stream.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoFacts {
    pub stream: u32,
    /// The unit of every timestamp below.
    pub time_base: Rational,
    pub geometry: StreamGeometry,
    pub encoding: EncodingSignature,
    /// In presentation order.
    pub keyframes: Vec<KeyframePoint>,
    /// `keyframes` lists every keyframe of the stream. While the index is
    /// still being built it does not, and a cut point that is not in the
    /// list cannot be proved to be off a keyframe *or* on one.
    pub keyframes_complete: bool,
    /// The first tick after the last frame, where known.
    pub end: Option<i64>,
    /// Decode order differs from presentation order (B-frames). Without
    /// reordering every frame boundary is a clean place to end a copy.
    pub reorders: bool,
    /// The encoder on this machine that makes this stream's kind of
    /// stream (#44), or why none does: what a seam or a re-encode that joins
    /// its copied packets would be made with.
    pub encoder: Result<EncoderChoice, Unmatched>,
}

/// What the planner knows about a source's audio stream: the one a video
/// clip's own sound plays from.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFacts {
    pub stream: u32,
    pub time_base: Rational,
    pub encoding: EncodingSignature,
}

/// What the planner knows about one source file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SourceFacts {
    pub video: Option<VideoFacts>,
    /// The default audio stream, and every audio stream an audio clip plays,
    /// by stream index.
    pub audio: BTreeMap<u32, AudioFacts>,
    /// The stream a video clip's own sound comes from.
    pub default_audio: Option<u32>,
}

/// Which of the output's streams a segment belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Media {
    Video,
    Audio,
}

/// One precondition of a copy that does not hold, with what the user needs
/// to know about it. Recorded as data — the export dialog (#50) phrases it
/// and the tests assert on it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(
    tag = "cause",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Cause {
    /// The in-point is not on a keyframe.
    InPointNotKeyframe {
        #[ts(type = "number | null")]
        keyframe_before: Option<i64>,
        #[ts(type = "number | null")]
        keyframe_after: Option<i64>,
    },
    /// The out-point falls inside a GOP of a stream whose pictures are
    /// decoded out of order, so the pictures before it depend on one after.
    OutPointNotKeyframe {
        #[ts(type = "number | null")]
        keyframe_before: Option<i64>,
        #[ts(type = "number | null")]
        keyframe_after: Option<i64>,
    },
    /// The out-point is an open-GOP keyframe: the pictures just before it
    /// are decoded after it.
    OpenGopAtOutPoint {
        #[ts(type = "number")]
        keyframe: i64,
    },
    /// The keyframes around a cut are not indexed yet.
    KeyframesUnknown,
    /// A hold or a reverse (#35).
    Operation { reason: ReEncodeReason },
    /// The speed makes a frame rate no file can carry (ADR-0009).
    SpeedOutsideContainer { rate: Rational },
    /// The sequence is not the source's shape (ADR-0008).
    SequenceMismatch { mismatch: Mismatch },
    /// The source's encoding parameters are not those of the rest of the
    /// copied output, so its packets cannot join the same stream.
    IncompatibleEncoding {
        /// The source whose parameters the output takes.
        reference: SourceId,
        fields: Vec<EncodingField>,
    },
    /// Nothing plays here.
    Gap,
    /// Gain, denoise or normalise.
    AudioChain,
    /// The sound plays at a speed other than normal.
    AudioSpeed { speed: Rational },
    /// Several sounds play at once.
    AudioMix { count: u32 },
}

/// What a cause does to its segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// A seam at a boundary: smart-cut.
    Seam(SeamReason),
    /// The whole segment is re-encoded.
    Whole(ReEncodeReason),
}

impl Cause {
    #[must_use]
    pub fn effect(&self) -> Effect {
        match self {
            Self::InPointNotKeyframe { .. } => Effect::Seam(SeamReason::InPointNotKeyframeAligned),
            Self::OutPointNotKeyframe { .. } => {
                Effect::Seam(SeamReason::OutPointNotKeyframeAligned)
            }
            Self::OpenGopAtOutPoint { .. } => Effect::Seam(SeamReason::OpenGopBoundary),
            Self::KeyframesUnknown => Effect::Whole(ReEncodeReason::CopyNotProvable),
            Self::Operation { reason } => Effect::Whole(*reason),
            Self::SpeedOutsideContainer { .. } => {
                Effect::Whole(ReEncodeReason::SpeedFrameRateOutsideContainer)
            }
            Self::SequenceMismatch { .. } => Effect::Whole(ReEncodeReason::SequenceSettingsDiffer),
            Self::IncompatibleEncoding { .. } => {
                Effect::Whole(ReEncodeReason::IncompatibleSourceParameters)
            }
            Self::Gap => Effect::Whole(ReEncodeReason::Gap),
            Self::AudioChain => Effect::Whole(ReEncodeReason::AudioFilter),
            Self::AudioSpeed { .. } => Effect::Whole(ReEncodeReason::AudioSpeed),
            Self::AudioMix { .. } => Effect::Whole(ReEncodeReason::AudioMix),
        }
    }

    /// The cause in a sentence, for the report and the log. The interface
    /// phrases the data itself; this is the engine's plain statement of it.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::InPointNotKeyframe { .. } => "The clip starts between keyframes, so the \
                pictures up to the next keyframe are re-encoded and the rest is copied."
                .to_owned(),
            Self::OutPointNotKeyframe { .. } => "The clip ends between keyframes, so the \
                pictures after the last keyframe are re-encoded and the rest is copied."
                .to_owned(),
            Self::OpenGopAtOutPoint { .. } => "The clip ends on a keyframe whose earlier \
                pictures depend on it, so those pictures are re-encoded."
                .to_owned(),
            Self::KeyframesUnknown => "The keyframes around this cut are not known yet, so \
                a copy cannot be proved safe and the clip is re-encoded."
                .to_owned(),
            Self::Operation {
                reason: ReEncodeReason::Reverse,
            } => "The clip plays backwards, so its pictures are re-encoded.".to_owned(),
            Self::Operation { .. } => {
                "The clip holds a frame, so the held picture is encoded.".to_owned()
            }
            Self::SpeedOutsideContainer { rate } => format!(
                "At this speed the clip would play at {} frames a second, which no video \
                 file can carry, so it is re-encoded at the sequence's frame rate.",
                rate_text(*rate)
            ),
            Self::SequenceMismatch { mismatch } => match mismatch {
                Mismatch::Resolution {
                    sequence_width,
                    sequence_height,
                    source_width,
                    source_height,
                } => format!(
                    "The clip is {source_width}×{source_height} and the sequence is \
                     {sequence_width}×{sequence_height}, so its pictures are re-encoded to fit."
                ),
                Mismatch::FrameRate { sequence, source } => format!(
                    "The clip runs at {} frames a second and the sequence at {}, so its \
                     pictures are re-encoded to fit.",
                    rate_text(*source),
                    rate_text(*sequence)
                ),
                Mismatch::PixelAspect { .. } => "The clip's pixel shape is not the \
                    sequence's, so its pictures are re-encoded to fit."
                    .to_owned(),
            },
            Self::IncompatibleEncoding { fields, .. } => format!(
                "The clip was encoded differently from the rest of the export ({}), so its \
                 packets cannot join the same stream and it is re-encoded to match.",
                fields
                    .iter()
                    .map(|field| field_text(*field))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Gap => "Nothing plays here, so black or silence is encoded.".to_owned(),
            Self::AudioChain => "The sound is adjusted, so it is re-encoded; the pictures \
                are not touched."
                .to_owned(),
            Self::AudioSpeed { speed } => format!(
                "The sound plays at {}× speed, so it is resampled and re-encoded.",
                rate_text(*speed)
            ),
            Self::AudioMix { count } => {
                format!("{count} sounds play at once, so they are mixed and re-encoded.")
            }
        }
    }
}

fn rate_text(rate: Rational) -> String {
    match rate.value() {
        Some(value) if rate.den == 1 => format!("{value:.0}"),
        Some(value) => format!("{value:.2}"),
        None => "?".to_owned(),
    }
}

fn field_text(field: EncodingField) -> &'static str {
    match field {
        EncodingField::Codec => "codec",
        EncodingField::Profile => "profile",
        EncodingField::Level => "level",
        EncodingField::PixelFormat => "pixel format",
        EncodingField::Resolution => "resolution",
        EncodingField::Colour => "colour",
        EncodingField::SampleRate => "sample rate",
        EncodingField::Channels => "channels",
        EncodingField::Configuration => "codec configuration",
    }
}

/// The pictures or sound a segment takes from one clip.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SegmentSource {
    pub track: TrackId,
    pub clip: ClipId,
    pub source: SourceId,
    pub stream: u32,
    /// The unit of `sourceIn` and `sourceOut`: the stream's time base.
    pub time_base: Rational,
    #[ts(type = "number")]
    pub source_in: i64,
    #[ts(type = "number")]
    pub source_out: i64,
    pub speed: Rational,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub motion: Option<Motion>,
    /// The clip's audio chain, as the evaluator resolved it: what the audio
    /// path (#43) applies to this source's sound. Empty for pictures.
    pub audio: Vec<AudioOperation>,
}

/// A range of source ticks a smart-cut re-encodes; everything else in the
/// segment is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SeamWindow {
    #[ts(type = "number")]
    pub from: i64,
    #[ts(type = "number")]
    pub to: i64,
}

/// Why a segment cannot be exported at all as the graph stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "decline",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Decline {
    /// Part of an HDR source would have to be rendered, and v1 renders only
    /// SDR. It is declined, never tone-mapped (ADR-0008).
    HdrWouldBeRendered,
    /// No encoder on this machine makes the source's kind of stream, so a
    /// seam or a re-encode that joins it cannot be made safely (ADR-0003).
    NoEncoder { unmatched: Unmatched },
}

/// The lossless alternative to a smart-cut: move the cut points to the
/// nearest place a copy can start and end (ADR-0003 part 4). For a declined
/// smart-cut it is the way to export at all; for any other, the way to export
/// with nothing re-encoded (#50).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KeyframeAlternative {
    #[ts(type = "number")]
    pub source_in: i64,
    #[ts(type = "number")]
    pub source_out: i64,
}

/// One stretch of one output stream, and what happens to it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Segment {
    pub media: Media,
    /// Sequence frames.
    #[ts(type = "number")]
    pub start: i64,
    #[ts(type = "number")]
    pub length: i64,
    /// Empty for a gap; more than one where sounds are mixed.
    pub sources: Vec<SegmentSource>,
    pub tier: ExportTier,
    /// Every precondition that failed. Empty exactly when the tier is a copy.
    pub causes: Vec<Cause>,
    /// For a smart-cut, the source ranges re-encoded.
    pub windows: Vec<SeamWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub decline: Option<Decline>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub alternative: Option<KeyframeAlternative>,
    /// For pictures that are encoded — a smart-cut's windows or a whole
    /// segment — the encoder that makes them and how it is asked (#44).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub encoder: Option<EncoderChoice>,
}

impl Segment {
    #[must_use]
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.length)
    }
}

/// How much of one stream each tier takes, in sequence frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Totals {
    /// Copied packet for packet, including the copied part of a smart-cut.
    #[ts(type = "number")]
    pub copied: i64,
    /// Re-encoded: whole segments and the windows of smart-cuts.
    #[ts(type = "number")]
    pub re_encoded: i64,
}

/// The plan in three numbers and two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanSummary {
    pub video: Totals,
    pub audio: Totals,
    /// Nothing is re-encoded: the output's pictures and sound are the
    /// source's packets.
    pub lossless: bool,
    /// No segment is declined.
    pub exportable: bool,
}

/// What an export will do, segment by segment.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportPlan {
    /// The unit of every segment's `start` and `length`.
    pub time_base: Rational,
    /// The sequence's shape: what a re-encoded segment is rendered at.
    pub sequence: SequenceSettings,
    /// The source whose encoding parameters the output's pictures take:
    /// the copied material's, or the first source's where nothing is copied.
    /// A re-encoded segment is encoded to match it (#55).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reference: Option<SourceId>,
    #[ts(type = "number")]
    pub length: i64,
    /// In output order: every video segment, then every audio segment. Each
    /// stream's segments tile `0..length` exactly, where the stream exists.
    pub segments: Vec<Segment>,
    pub summary: PlanSummary,
}

/// Why no plan could be made.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum PlanError {
    #[error("the timeline is empty")]
    Empty,
    #[error("source {0} has not been read yet, or is missing")]
    UnknownSource(SourceId),
    #[error("source {0} has no video stream, but it is on a video track")]
    NoVideo(SourceId),
    #[error("source {file} has no audio stream {stream}")]
    NoAudio { file: SourceId, stream: u32 },
    #[error("clip {0}'s timing does not fit in 64 bits")]
    Overflow(ClipId),
}

/// Plan the export of `timeline`, made in a sequence with `settings`, over
/// sources described by `sources`.
///
/// # Errors
///
/// An empty timeline, or a clip whose source has not been described.
pub fn plan(
    timeline: &Timeline,
    settings: &SequenceSettings,
    sources: &BTreeMap<SourceId, SourceFacts>,
) -> Result<ExportPlan, PlanError> {
    let length = timeline.length();
    if length <= 0 {
        return Err(PlanError::Empty);
    }
    let (mut segments, reference) = plan_video(timeline, settings, sources, length)?;
    segments.extend(plan_audio(timeline, sources, length)?);
    let summary = summarise(&segments, timeline.time_base);
    Ok(ExportPlan {
        time_base: timeline.time_base,
        sequence: *settings,
        reference,
        length,
        segments,
        summary,
    })
}

fn facts(
    sources: &BTreeMap<SourceId, SourceFacts>,
    source: SourceId,
) -> Result<&SourceFacts, PlanError> {
    sources.get(&source).ok_or(PlanError::UnknownSource(source))
}

/// Decide a segment's tier from its causes: any whole-segment cause makes it
/// a re-encode, named by the first; otherwise any seam makes it a smart-cut;
/// otherwise it is a copy.
fn tier_of(causes: &[Cause]) -> ExportTier {
    let effects: Vec<Effect> = causes.iter().map(Cause::effect).collect();
    if let Some(reason) = effects.iter().find_map(|effect| match effect {
        Effect::Whole(reason) => Some(*reason),
        Effect::Seam(_) => None,
    }) {
        return ExportTier::FullReEncode { reason };
    }
    effects
        .iter()
        .find_map(|effect| match effect {
            Effect::Seam(reason) => Some(ExportTier::SmartCut { reason: *reason }),
            Effect::Whole(_) => None,
        })
        .unwrap_or(ExportTier::StreamCopy)
}

fn source_of(placement: &Placement, stream: u32, time_base: Rational) -> SegmentSource {
    SegmentSource {
        track: placement.track,
        clip: placement.clip,
        source: placement.source,
        stream,
        time_base,
        source_in: placement.source_in,
        source_out: placement.source_out,
        speed: placement.speed,
        motion: placement.motion,
        audio: Vec::new(),
    }
}

fn gap(media: Media, start: i64, end: i64) -> Segment {
    Segment {
        media,
        start,
        length: end - start,
        sources: Vec::new(),
        tier: ExportTier::FullReEncode {
            reason: ReEncodeReason::Gap,
        },
        causes: vec![Cause::Gap],
        windows: Vec::new(),
        decline: None,
        alternative: None,
        encoder: None,
    }
}

/// The first keyframe of `video` after `pts` that a copy may start on.
fn next_random_access(video: &VideoFacts, pts: i64) -> Option<KeyframePoint> {
    video
        .keyframes
        .iter()
        .find(|k| k.pts > pts && k.random_access)
        .copied()
}

/// The last keyframe of `video` at or before `pts` that a copy may start on.
fn random_access_before(video: &VideoFacts, pts: i64) -> Option<KeyframePoint> {
    video
        .keyframes
        .iter()
        .rev()
        .find(|k| k.pts <= pts && k.random_access)
        .copied()
}

/// The keyframes of `video` at or before and at or after `pts`.
fn around(video: &VideoFacts, pts: i64) -> (Option<KeyframePoint>, Option<KeyframePoint>) {
    let after = video.keyframes.partition_point(|k| k.pts < pts);
    let at_or_after = video.keyframes.get(after).copied();
    let at_or_before = match at_or_after {
        Some(k) if k.pts == pts => Some(k),
        _ => after
            .checked_sub(1)
            .and_then(|i| video.keyframes.get(i))
            .copied(),
    };
    (at_or_before, at_or_after)
}

/// Where a copy of `video` may end: at the end of the stream, on a keyframe
/// whose GOP is closed, or anywhere at all in a stream that never reorders.
fn clean_end(video: &VideoFacts, pts: i64) -> bool {
    !video.reorders
        || video.end.is_some_and(|end| pts >= end)
        || around(video, pts)
            .1
            .is_some_and(|k| k.pts == pts && !k.open)
}

/// The seam causes and windows of a copy of `in_..out` from `video`.
fn boundaries(video: &VideoFacts, in_: i64, out: i64) -> (Vec<Cause>, Vec<SeamWindow>) {
    let mut causes = Vec::new();
    let mut windows = Vec::new();
    let (before_in, _) = around(video, in_);
    let in_aligned = before_in.is_some_and(|k| k.pts == in_ && k.random_access);
    if !in_aligned {
        if !video.keyframes_complete && before_in.is_none_or(|k| k.pts != in_) {
            return (vec![Cause::KeyframesUnknown], Vec::new());
        }
        let after_in = next_random_access(video, in_);
        causes.push(Cause::InPointNotKeyframe {
            keyframe_before: random_access_before(video, in_).map(|k| k.pts),
            keyframe_after: after_in.map(|k| k.pts),
        });
        windows.push(SeamWindow {
            from: in_,
            to: after_in.map_or(out, |k| k.pts.min(out)),
        });
    }
    if !clean_end(video, out) {
        let (before_out, after_out) = around(video, out);
        match after_out.filter(|k| k.pts == out) {
            Some(keyframe) => causes.push(Cause::OpenGopAtOutPoint {
                keyframe: keyframe.pts,
            }),
            None if !video.keyframes_complete => {
                return (vec![Cause::KeyframesUnknown], Vec::new());
            }
            None => causes.push(Cause::OutPointNotKeyframe {
                keyframe_before: before_out.map(|k| k.pts),
                keyframe_after: after_out.map(|k| k.pts),
            }),
        }
        // The GOP the out-point ends in, from its keyframe — at most; the
        // executor narrows it to the pictures that actually depend on what
        // follows (#41).
        let from = video
            .keyframes
            .iter()
            .rev()
            .find(|k| k.pts < out)
            .map_or(in_, |k| k.pts)
            .max(in_);
        match windows.last_mut() {
            Some(head) if head.to >= from => head.to = out,
            _ => windows.push(SeamWindow { from, to: out }),
        }
    }
    (causes, windows)
}

/// The nearest place to `in_` and `out` a copy can start and end.
fn keyframe_alternative(video: &VideoFacts, in_: i64, out: i64) -> Option<KeyframeAlternative> {
    let nearest = |pts: i64, usable: &dyn Fn(&KeyframePoint) -> bool| {
        video
            .keyframes
            .iter()
            .filter(|k| usable(k))
            .min_by_key(|k| (k.pts - pts).unsigned_abs())
            .map(|k| k.pts)
    };
    let source_in = nearest(in_, &|k| k.random_access)?;
    let source_out = if clean_end(video, out) {
        out
    } else {
        nearest(out, &|k| !k.open && k.pts > source_in)
            .or(video.end)
            .filter(|&end| end > source_in)?
    };
    Some(KeyframeAlternative {
        source_in,
        source_out,
    })
}

fn plan_video(
    timeline: &Timeline,
    settings: &SequenceSettings,
    sources: &BTreeMap<SourceId, SourceFacts>,
    length: i64,
) -> Result<(Vec<Segment>, Option<SourceId>), PlanError> {
    let picture = timeline.picture();
    if picture.is_empty() {
        return Ok((Vec::new(), None));
    }
    let mut segments = Vec::with_capacity(picture.len() * 2 + 1);
    let mut signatures: Vec<Option<&EncodingSignature>> = Vec::with_capacity(picture.len() * 2);
    let mut at = 0;
    for piece in &picture {
        if piece.start > at {
            segments.push(gap(Media::Video, at, piece.start));
            signatures.push(None);
        }
        let video = facts(sources, piece.source)?
            .video
            .as_ref()
            .ok_or(PlanError::NoVideo(piece.source))?;
        let mut causes = Vec::new();
        if let Some(reason) = piece.forced {
            causes.push(Cause::Operation { reason });
        }
        let verdict = speed::verdict(piece, &video.geometry);
        if let ExportTier::FullReEncode { .. } = verdict.tier {
            causes.push(Cause::SpeedOutsideContainer {
                rate: verdict.output_frame_rate,
            });
        }
        causes.extend(
            copy_eligibility(settings, &video.geometry)
                .mismatches
                .into_iter()
                .map(|mismatch| Cause::SequenceMismatch { mismatch }),
        );
        let mut windows = Vec::new();
        if causes.is_empty() {
            let (in_, out) = in_stream_ticks(piece, video)?;
            let (seams, seam_windows) = boundaries(video, in_, out);
            causes.extend(seams);
            windows = seam_windows;
        }
        segments.push(Segment {
            media: Media::Video,
            start: piece.start,
            length: piece.length,
            sources: vec![source_of(piece, video.stream, piece.time_base)],
            tier: ExportTier::StreamCopy,
            causes,
            windows,
            decline: None,
            alternative: None,
            encoder: None,
        });
        signatures.push(Some(&video.encoding));
        at = piece.end();
    }
    if at < length {
        segments.push(gap(Media::Video, at, length));
        signatures.push(None);
    }
    let reference = share_encoding(&mut segments, &signatures).or_else(|| {
        segments
            .iter()
            .find_map(|segment| segment.sources.first().map(|s| s.source))
    });
    for segment in &mut segments {
        segment.tier = tier_of(&segment.causes);
        if !matches!(segment.tier, ExportTier::SmartCut { .. }) {
            segment.windows.clear();
        }
        decline_unencodable(segment, sources, reference);
    }
    Ok((segments, reference))
}

/// A piece's in- and out-point in its stream's own ticks.
fn in_stream_ticks(piece: &Placement, video: &VideoFacts) -> Result<(i64, i64), PlanError> {
    let overflow = || PlanError::Overflow(piece.clip);
    let in_ = rescale(
        piece.source_in,
        piece.time_base,
        video.time_base,
        Rounding::Down,
    )
    .ok_or_else(overflow)?;
    let out = rescale(
        piece.source_out,
        piece.time_base,
        video.time_base,
        Rounding::Up,
    )
    .ok_or_else(overflow)?;
    Ok((in_, out))
}

/// The output takes the encoding parameters of the copied material that
/// covers most of it; every other copy candidate is re-encoded to match,
/// with the fields that differ named. Ties go to the earlier material.
fn share_encoding(
    segments: &mut [Segment],
    signatures: &[Option<&EncodingSignature>],
) -> Option<SourceId> {
    let candidates: Vec<usize> = (0..segments.len())
        .filter(|&i| {
            signatures.get(i).copied().flatten().is_some()
                && segments.get(i).is_some_and(|segment| {
                    segment
                        .causes
                        .iter()
                        .all(|cause| matches!(cause.effect(), Effect::Seam(_)))
                })
        })
        .collect();
    let mut weight: Vec<(&EncodingSignature, i64, usize, SourceId)> = Vec::new();
    for &i in &candidates {
        let (Some(Some(signature)), Some(segment)) = (signatures.get(i), segments.get(i)) else {
            continue;
        };
        let source = segment.sources.first().map_or(0, |s| s.source);
        match weight.iter_mut().find(|(known, ..)| *known == *signature) {
            Some(entry) => entry.1 += segment.length,
            None => weight.push((signature, segment.length, i, source)),
        }
    }
    let &(reference, _, _, reference_source) = weight
        .iter()
        .max_by_key(|(_, total, first, _)| (*total, std::cmp::Reverse(*first)))?;
    for i in candidates {
        let (Some(Some(signature)), Some(segment)) = (signatures.get(i), segments.get_mut(i))
        else {
            continue;
        };
        let fields = signature.differences(reference);
        if !fields.is_empty() {
            segment.causes.push(Cause::IncompatibleEncoding {
                reference: reference_source,
                fields,
            });
        }
    }
    Some(reference_source)
}

/// The pictures of a segment that are encoded need an encoder that makes the
/// output's kind of stream. A smart-cut's windows join its own source's copied
/// packets, so they match that source; a whole re-encode — a hold, a reverse,
/// a clip in another shape, black in a gap — joins the rest of the output, so
/// it matches the output's reference source (#55). Where there is such an
/// encoder the segment names it; where there is none, the segment is declined
/// (ADR-0003). A smart-cut offers the keyframe-aligned cut either way. HDR
/// is declined whatever the encoders: v1 renders only SDR, and never
/// tone-maps (ADR-0008).
fn decline_unencodable(
    segment: &mut Segment,
    sources: &BTreeMap<SourceId, SourceFacts>,
    reference: Option<SourceId>,
) {
    if segment.tier.is_lossless() || segment.media != Media::Video {
        return;
    }
    let own = segment.sources.first().map(|s| s.source);
    let matched = match segment.tier {
        ExportTier::SmartCut { .. } => own,
        _ => reference.or(own),
    };
    let video_of = |id: Option<SourceId>| {
        id.and_then(|id| sources.get(&id))
            .and_then(|facts| facts.video.as_ref())
    };
    let (Some(target), rendered) = (video_of(matched), video_of(own)) else {
        return;
    };
    if target.geometry.hdr || rendered.is_some_and(|video| video.geometry.hdr) {
        segment.decline = Some(Decline::HdrWouldBeRendered);
    } else {
        match &target.encoder {
            Ok(choice) => segment.encoder = Some(choice.clone()),
            Err(unmatched) => {
                segment.decline = Some(Decline::NoEncoder {
                    unmatched: unmatched.clone(),
                });
            }
        }
    }
    let (Some(first), Some(video)) = (segment.sources.first(), rendered) else {
        return;
    };
    // Every smart-cut offers its keyframe-aligned cut: where the smart-cut
    // is declined it is the way to export at all, and otherwise it is the
    // way to export without re-encoding anything (#50).
    if let ExportTier::SmartCut { .. } = segment.tier {
        let in_ = rescale(
            first.source_in,
            first.time_base,
            video.time_base,
            Rounding::Down,
        );
        let out = rescale(
            first.source_out,
            first.time_base,
            video.time_base,
            Rounding::Up,
        );
        if let (Some(in_), Some(out)) = (in_, out) {
            segment.alternative = keyframe_alternative(video, in_, out);
        }
    }
}

/// A placement that sounds, with the stream it sounds from.
struct Sound {
    placement: Placement,
    stream: u32,
    /// The audio stream's time base, which the ranges below are in.
    time_base: Rational,
    source_in: i64,
    source_out: i64,
}

fn sounds(
    timeline: &Timeline,
    sources: &BTreeMap<SourceId, SourceFacts>,
) -> Result<Vec<Sound>, PlanError> {
    let mut sounds = Vec::new();
    for track in timeline.tracks.iter().filter(|track| track.audible) {
        for placement in &track.placements {
            let facts = facts(sources, placement.source)?;
            let stream = match track.kind {
                TrackKind::Audio => placement.stream,
                TrackKind::Video if placement.silent => continue,
                TrackKind::Video => match facts.default_audio {
                    Some(stream) => stream,
                    None => continue,
                },
            };
            let audio = facts.audio.get(&stream).ok_or(PlanError::NoAudio {
                file: placement.source,
                stream,
            })?;
            let overflow = || PlanError::Overflow(placement.clip);
            let source_in = rescale(
                placement.source_in,
                placement.time_base,
                audio.time_base,
                Rounding::Down,
            )
            .ok_or_else(overflow)?;
            let source_out = rescale(
                placement.source_out,
                placement.time_base,
                audio.time_base,
                Rounding::Up,
            )
            .ok_or_else(overflow)?;
            sounds.push(Sound {
                placement: placement.clone(),
                stream,
                time_base: audio.time_base,
                source_in,
                source_out,
            });
        }
    }
    Ok(sounds)
}

/// The segment of one sound playing alone, and its encoding, if it may be
/// copied.
fn single_sound<'a>(
    sound: &Sound,
    piece: &SegmentSource,
    from: i64,
    to: i64,
    sources: &'a BTreeMap<SourceId, SourceFacts>,
) -> Result<(Segment, Option<&'a EncodingSignature>), PlanError> {
    let mut causes = Vec::new();
    if let Some(reason) = sound.placement.forced {
        causes.push(Cause::Operation { reason });
    }
    // A bypassed step is kept in the graph and applied to nothing: it does
    // not stop the sound being copied.
    if sound.placement.audio.iter().any(|step| !step.bypassed()) {
        causes.push(Cause::AudioChain);
    }
    if sound.placement.speed != (Rational { num: 1, den: 1 }) {
        causes.push(Cause::AudioSpeed {
            speed: sound.placement.speed,
        });
    }
    let signature = facts(sources, piece.source)?
        .audio
        .get(&piece.stream)
        .map(|audio| &audio.encoding);
    Ok((
        Segment {
            media: Media::Audio,
            start: from,
            length: to - from,
            sources: vec![piece.clone()],
            tier: ExportTier::StreamCopy,
            causes,
            windows: Vec::new(),
            decline: None,
            alternative: None,
            encoder: None,
        },
        signature,
    ))
}

fn plan_audio(
    timeline: &Timeline,
    sources: &BTreeMap<SourceId, SourceFacts>,
    length: i64,
) -> Result<Vec<Segment>, PlanError> {
    let mut sounds = sounds(timeline, sources)?;
    if sounds.is_empty() {
        return Ok(Vec::new());
    }
    sounds.sort_by_key(|sound| sound.placement.start);
    let mut edges: Vec<i64> = sounds
        .iter()
        .flat_map(|sound| [sound.placement.start, sound.placement.end()])
        .chain([0, length])
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let mut segments: Vec<Segment> = Vec::new();
    let mut signatures: Vec<Option<&EncodingSignature>> = Vec::new();
    // A sweep: the sounds playing at each edge, kept as the edges advance,
    // so a long timeline plans in time proportional to its length.
    let mut next = 0;
    let mut playing: Vec<&Sound> = Vec::new();
    for pair in edges.windows(2) {
        let &[from, to] = pair else { continue };
        playing.retain(|sound| from < sound.placement.end());
        while let Some(sound) = sounds.get(next).filter(|s| s.placement.start <= from) {
            if from < sound.placement.end() {
                playing.push(sound);
            }
            next += 1;
        }
        let mut pieces = Vec::with_capacity(playing.len());
        for sound in &playing {
            let piece = crop(&sound.placement, from, to)
                .ok_or(PlanError::Overflow(sound.placement.clip))?;
            let overflow = || PlanError::Overflow(piece.clip);
            let source_in = rescale(
                piece.source_in,
                piece.time_base,
                sound.time_base,
                Rounding::Down,
            )
            .ok_or_else(overflow)?
            .max(sound.source_in);
            let source_out = rescale(
                piece.source_out,
                piece.time_base,
                sound.time_base,
                Rounding::Up,
            )
            .ok_or_else(overflow)?
            .min(sound.source_out);
            pieces.push(SegmentSource {
                source_in,
                source_out,
                audio: piece.audio.clone(),
                ..source_of(&piece, sound.stream, sound.time_base)
            });
        }
        let (segment, signature) = match (playing.as_slice(), pieces.as_slice()) {
            ([], _) => (gap(Media::Audio, from, to), None),
            ([sound], [piece]) => single_sound(sound, piece, from, to, sources)?,
            _ => (
                Segment {
                    media: Media::Audio,
                    start: from,
                    length: to - from,
                    sources: pieces,
                    tier: ExportTier::StreamCopy,
                    causes: vec![Cause::AudioMix {
                        count: u32::try_from(playing.len()).unwrap_or(u32::MAX),
                    }],
                    windows: Vec::new(),
                    decline: None,
                    alternative: None,
                    encoder: None,
                },
                None,
            ),
        };
        segments.push(segment);
        signatures.push(signature);
    }
    let _ = share_encoding(&mut segments, &signatures);
    for segment in &mut segments {
        segment.tier = tier_of(&segment.causes);
    }
    Ok(segments)
}

/// The sequence frames of `segment` that a smart-cut re-encodes.
fn window_frames(segment: &Segment, sequence: Rational) -> i64 {
    let Some(source) = segment.sources.first() else {
        return 0;
    };
    let played = Rational {
        num: source.time_base.num.saturating_mul(source.speed.den),
        den: source.time_base.den.saturating_mul(source.speed.num),
    };
    segment
        .windows
        .iter()
        .map(|window| {
            rescale(window.to - window.from, played, sequence, Rounding::Up).unwrap_or(i64::MAX)
        })
        .fold(0_i64, i64::saturating_add)
        .min(segment.length)
}

fn summarise(segments: &[Segment], sequence: Rational) -> PlanSummary {
    let mut video = Totals::default();
    let mut audio = Totals::default();
    for segment in segments {
        let totals = match segment.media {
            Media::Video => &mut video,
            Media::Audio => &mut audio,
        };
        let re_encoded = match segment.tier {
            ExportTier::StreamCopy => 0,
            ExportTier::SmartCut { .. } => window_frames(segment, sequence),
            ExportTier::FullReEncode { .. } => segment.length,
        };
        totals.re_encoded += re_encoded;
        totals.copied += segment.length - re_encoded;
    }
    PlanSummary {
        video,
        audio,
        lossless: segments.iter().all(|segment| segment.tier.is_lossless()),
        exportable: segments.iter().all(|segment| segment.decline.is_none()),
    }
}
