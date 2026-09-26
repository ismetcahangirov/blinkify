//! What an export will do, told before it is asked for (#50): the plan's
//! decisions as the export dialog states them.
//!
//! Everything here is read off the plan (#39) and the executor's own checks
//! (#40); nothing is decided twice. The dialog shows the result; it computes
//! none of it (`CLAUDE.md` section 2).
//!
//! - **Pictures and sound are claimed separately.** A gain change re-encodes
//!   the sound and copies the pictures; one badge for both would either hide
//!   the first or deny the second.
//! - **Every re-encode names its cause and its time.** "The cut at 00:01:23 is
//!   0.40 s from the nearest keyframe" is something to act on; "quality may be
//!   affected" is not.
//! - **The keyframe snap is offered with its cost.** Each cut that would be
//!   smart-cut can move to the nearest place a copy starts and ends; the offer
//!   says by how much each cut moves, and it is an ordinary undoable edit
//!   ([`Edit::SnapToKeyframes`]), so what is previewed is what is exported.
//! - **The size says whether it is an estimate.** A copy is its source's
//!   packets, and its size follows from the streams' bitrates; an encoder's
//!   output size is not known until it has run.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use super::audio::{AudioEncoding, AudioTarget};
use super::execute::{ExportInput, check};
use super::plan::{Cause, Decline, ExportPlan, Media, Segment, SegmentSource};
use crate::audio::denoise::Models;
use crate::probe::Rational;
use crate::project::edit::{Edit, KeyframeSnap};
use crate::project::evaluate::Timeline;
use crate::project::{ClipId, SourceId};
use crate::tier::ExportTier;
use crate::time::{Rounding, rescale};

/// Bytes of 24-bit PCM a second, per channel and hertz; FLAC is taken at
/// about 60 % of it.
const PCM_BYTES_PER_SAMPLE: f64 = 3.0;
const FLAC_RATIO: f64 = 0.6;

/// What happens to one of the output's streams.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StreamClaim {
    /// Nothing of the stream is re-encoded.
    pub lossless: bool,
    pub copied_seconds: f64,
    pub re_encoded_seconds: f64,
}

/// One reason part of the output is not copied, with where it is.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reason {
    pub media: Media,
    /// Where on the sequence it applies: a cut's own time, or the start of
    /// the stretch.
    pub at_seconds: f64,
    pub tier: ExportTier,
    pub sentence: String,
    /// The part cannot be exported as it stands.
    pub declined: bool,
}

/// One cut the snap moves, and by how much.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnappedCut {
    pub clip: ClipId,
    /// Where the clip starts on the sequence.
    pub at_seconds: f64,
    /// How far its in-point moves in the source: positive is later, which
    /// drops pictures from the start.
    pub in_shift_seconds: f64,
    /// How far its out-point moves: positive is later, which adds pictures
    /// at the end.
    pub out_shift_seconds: f64,
}

/// Moving every smart-cut to the nearest keyframes, so nothing of the
/// pictures is re-encoded for a cut.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapOffer {
    /// The edit that makes it: applied like any other, and undoable.
    pub edit: Edit,
    pub cuts: Vec<SnappedCut>,
    /// How much longer the sequence becomes; negative is shorter.
    pub length_change_seconds: f64,
    /// Smart-cuts it cannot move: a clip partly covered by another above it
    /// is cut there by the cover, not by its own trim.
    pub unsnappable: u32,
}

/// How large the output will be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SizeEstimate {
    #[ts(type = "number")]
    pub bytes: u64,
    /// Everything is copied, and every copied stream's rate is one its file
    /// records: the size follows from the sources' packets, to within 10 %
    /// (tested). Otherwise an encoder's output, or a split of a file's rate
    /// between streams it does not record, is part of it: an estimate.
    pub precise: bool,
}

/// The target's volume, against the size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiskSpace {
    #[ts(type = "number")]
    pub available: u64,
    pub enough: bool,
}

/// What exporting the plan to a target will do, before it is asked for.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportOverview {
    /// `None` for a stream the output does not have.
    pub video: Option<StreamClaim>,
    pub audio: Option<StreamClaim>,
    /// Nothing at all is re-encoded.
    pub lossless: bool,
    pub duration_seconds: f64,
    /// In output order: the pictures' reasons, then the sound's.
    pub reasons: Vec<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub snap: Option<SnapOffer>,
    pub size: SizeEstimate,
    /// What re-encoded sound is made as, where any is.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub audio_encoding: Option<AudioEncoding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub space: Option<DiskSpace>,
    /// A file is at the target already: replacing it must be confirmed.
    pub target_exists: bool,
    /// Why the export would be refused as it stands; empty when it can run.
    pub problems: Vec<String>,
}

/// Everything an overview is made from.
#[derive(Debug, Clone, Copy)]
pub struct OverviewRequest<'a> {
    pub plan: &'a ExportPlan,
    /// The graph the plan was made from: where each clip is.
    pub timeline: &'a Timeline,
    pub inputs: &'a BTreeMap<SourceId, ExportInput>,
    pub target: &'a Path,
    pub audio: AudioTarget,
    pub models: Option<&'a Models>,
    /// Free bytes on the target's volume, where known.
    pub available: Option<u64>,
}

/// What exporting `request.plan` to `request.target` will do.
#[must_use]
pub fn overview(request: &OverviewRequest<'_>) -> ExportOverview {
    let plan = request.plan;
    let seconds = |frames: i64| seconds(frames, plan.time_base);
    let mut problems = Vec::new();
    // The executor's own checks; replacing an existing file is the save
    // dialog's question, not a problem.
    let audio_encoding = match check(
        plan,
        request.inputs,
        request.target,
        true,
        request.audio,
        request.models,
    ) {
        Ok(encoding) => encoding,
        Err(error) => {
            problems.push(error.to_string());
            None
        }
    };
    let size = estimate(plan, request.inputs, audio_encoding.as_ref());
    let space = request.available.map(|available| DiskSpace {
        available,
        enough: enough_space(size.bytes, available),
    });
    if let Some(space) = space
        && !space.enough
    {
        problems.push(format!(
            "the target's drive has {} free, and the export needs about {}",
            bytes_text(space.available),
            bytes_text(size.bytes)
        ));
    }
    let reasons = plan
        .segments
        .iter()
        .flat_map(|segment| reasons_of(segment, plan.time_base))
        .collect();
    ExportOverview {
        video: claim(plan, Media::Video),
        audio: claim(plan, Media::Audio),
        lossless: plan.summary.lossless,
        duration_seconds: seconds(plan.length),
        reasons,
        snap: snap_offer(plan, request.timeline),
        size,
        audio_encoding,
        space,
        target_exists: request.target.exists(),
        problems,
    }
}

/// Whether `available` bytes hold an output of about `needed`: with a
/// margin, since the size is an estimate and a volume at zero bytes free
/// fails in ways nobody wants to find out about at 90 percent.
#[must_use]
pub fn enough_space(needed: u64, available: u64) -> bool {
    let margin = needed.div_ceil(20) + 64 * 1024 * 1024;
    available >= needed.saturating_add(margin)
}

fn seconds(ticks: i64, time_base: Rational) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = ticks as f64 * time_base.value().unwrap_or(0.0);
    seconds
}

fn claim(plan: &ExportPlan, media: Media) -> Option<StreamClaim> {
    plan.segments.iter().any(|s| s.media == media).then(|| {
        let totals = match media {
            Media::Video => plan.summary.video,
            Media::Audio => plan.summary.audio,
        };
        StreamClaim {
            lossless: totals.re_encoded == 0,
            copied_seconds: seconds(totals.copied, plan.time_base),
            re_encoded_seconds: seconds(totals.re_encoded, plan.time_base),
        }
    })
}

/// Each reason `segment` is not copied, where it applies.
fn reasons_of(segment: &Segment, time_base: Rational) -> Vec<Reason> {
    let start = seconds(segment.start, time_base);
    let end = seconds(segment.end(), time_base);
    let mut reasons: Vec<Reason> = segment
        .causes
        .iter()
        .map(|cause| {
            let at = match cause {
                Cause::OutPointNotKeyframe { .. } | Cause::OpenGopAtOutPoint { .. } => end,
                _ => start,
            };
            Reason {
                media: segment.media,
                at_seconds: at,
                tier: segment.tier,
                sentence: explain(cause, segment.sources.first()),
                declined: false,
            }
        })
        .collect();
    if let Some(decline) = &segment.decline {
        reasons.push(Reason {
            media: segment.media,
            at_seconds: start,
            tier: segment.tier,
            sentence: explain_decline(decline, segment.alternative.is_some()),
            declined: true,
        });
    }
    reasons
}

/// A cause in a sentence, with the distance to the keyframe where the cause
/// is a cut: the export dialog and the report (#52) say the same thing.
#[must_use]
pub fn explain(cause: &Cause, source: Option<&SegmentSource>) -> String {
    let distance = |cut: i64, before: Option<i64>, after: Option<i64>| {
        let source = source?;
        let nearest = [before, after]
            .into_iter()
            .flatten()
            .map(|keyframe| (cut - keyframe).unsigned_abs())
            .min()?;
        let nearest = i64::try_from(nearest).ok()?;
        Some(seconds(nearest, source.time_base))
    };
    match cause {
        Cause::InPointNotKeyframe {
            keyframe_before,
            keyframe_after,
        } => match source.and_then(|s| distance(s.source_in, *keyframe_before, *keyframe_after)) {
            Some(gap) => format!(
                "The clip starts {gap:.2} s from the nearest keyframe, so the pictures up to \
                 the next keyframe are re-encoded and the rest is copied."
            ),
            None => cause.sentence(),
        },
        Cause::OutPointNotKeyframe {
            keyframe_before,
            keyframe_after,
        } => match source.and_then(|s| distance(s.source_out, *keyframe_before, *keyframe_after)) {
            Some(gap) => format!(
                "The clip ends {gap:.2} s from the nearest keyframe, so the pictures after \
                 the last keyframe are re-encoded and the rest is copied."
            ),
            None => cause.sentence(),
        },
        _ => cause.sentence(),
    }
}

/// Why a part cannot be exported as it stands, and the way out.
#[must_use]
pub fn explain_decline(decline: &Decline, snappable: bool) -> String {
    let remedy = if snappable {
        " Snapping the cut to the nearest keyframes exports it with nothing re-encoded."
    } else {
        ""
    };
    match decline {
        Decline::HdrWouldBeRendered => format!(
            "Part of this HDR clip would have to be re-encoded, and Blinkify never re-encodes \
             HDR pictures, so it cannot be exported as it stands.{remedy}"
        ),
        Decline::NoEncoder { unmatched } => format!(
            "The cut cannot be smart-cut: {unmatched}, so no re-encoded picture could join \
             the copied ones.{remedy}"
        ),
    }
}

/// The snap of every smart-cut whose clip's own trim makes the cut.
fn snap_offer(plan: &ExportPlan, timeline: &Timeline) -> Option<SnapOffer> {
    let mut snaps: Vec<KeyframeSnap> = Vec::new();
    let mut cuts = Vec::new();
    let mut unsnappable = 0_u32;
    let mut change = 0.0;
    for segment in &plan.segments {
        let (Media::Video, ExportTier::SmartCut { .. }, Some(alternative)) =
            (segment.media, segment.tier, segment.alternative)
        else {
            continue;
        };
        let snapped = match segment.sources.as_slice() {
            [source] => timeline
                .placements()
                .find(|placement| placement.clip == source.clip)
                .filter(|placement| placement.motion.is_none())
                .and_then(|placement| {
                    let to_clip = |ticks: i64, rounding| {
                        rescale(ticks, source.time_base, placement.time_base, rounding)
                    };
                    let (from, to) = (
                        to_clip(alternative.source_in, Rounding::Nearest)?,
                        to_clip(alternative.source_out, Rounding::Nearest)?,
                    );
                    let whole = to_clip(source.source_in, Rounding::Down)? == placement.source_in
                        && to_clip(source.source_out, Rounding::Up)? == placement.source_out;
                    whole.then_some((placement, from, to))
                }),
            _ => None,
        };
        let Some((placement, from, to)) = snapped else {
            unsnappable += 1;
            continue;
        };
        if snaps.iter().any(|snap| snap.clip == placement.clip) {
            continue;
        }
        let in_shift = seconds(from - placement.source_in, placement.time_base);
        let out_shift = seconds(to - placement.source_out, placement.time_base);
        let speed = placement.speed.value().unwrap_or(1.0);
        change += (out_shift - in_shift) / speed;
        snaps.push(KeyframeSnap {
            clip: placement.clip,
            from,
            to,
        });
        cuts.push(SnappedCut {
            clip: placement.clip,
            at_seconds: seconds(placement.start, plan.time_base),
            in_shift_seconds: in_shift,
            out_shift_seconds: out_shift,
        });
    }
    (!snaps.is_empty() || unsnappable > 0).then_some(SnapOffer {
        edit: Edit::SnapToKeyframes { snaps },
        cuts,
        length_change_seconds: change,
        unsnappable,
    })
}

/// The output's size: each copied stretch at its source stream's bitrate,
/// each encoded one at what its encoder is asked for — or, for pictures,
/// which an encoder is asked to match in quality rather than in rate, at the
/// source's.
fn estimate(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    encoding: Option<&AudioEncoding>,
) -> SizeEstimate {
    let bitrate = |source: &SegmentSource| -> Rate {
        inputs
            .get(&source.source)
            .and_then(|input| stream_rate(&input.info, source.stream))
            .unwrap_or(Rate {
                bits: 0.0,
                recorded: false,
            })
    };
    let reference_rate = plan
        .reference
        .and_then(|reference| {
            plan.segments
                .iter()
                .filter(|s| s.media == Media::Video)
                .flat_map(|s| &s.sources)
                .find(|source| source.source == reference)
        })
        .map_or(0.0, |source| bitrate(source).bits);
    let mut bits = 0.0;
    let mut recorded = true;
    for segment in &plan.segments {
        let length = seconds(segment.length, plan.time_base);
        bits += match (segment.media, segment.tier) {
            (_, ExportTier::StreamCopy | ExportTier::SmartCut { .. }) => {
                segment.sources.first().map_or(0.0, |source| {
                    let rate = bitrate(source);
                    recorded &= rate.recorded;
                    rate.bits * length * speed_of(source)
                })
            }
            (Media::Video, ExportTier::FullReEncode { .. }) => reference_rate * length,
            (Media::Audio, ExportTier::FullReEncode { .. }) => {
                encoding.map_or(0.0, |encoding| audio_bits_per_second(encoding) * length)
            }
        };
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bytes = (bits / 8.0).max(0.0).round() as u64;
    SizeEstimate {
        bytes,
        precise: plan.summary.lossless && recorded,
    }
}

/// Kilobits a second assumed per channel of a sound whose file records no
/// rate for it: what Opus and AAC are commonly given (128 kb/s for stereo).
const ASSUMED_KILOBITS_PER_CHANNEL: f64 = 64.0;

/// A stream's rate, and whether its file records it.
#[derive(Debug, Clone, Copy)]
struct Rate {
    bits: f64,
    recorded: bool,
}

/// A stream's bitrate. Matroska records none per stream. There, a stream
/// that is the only one without a rate takes what the file's rate leaves
/// after the others, which is still the file's own figure; where several
/// have none, a sound is taken at a common rate for its channels and the
/// pictures share the rest, and the figure is an assumption.
fn stream_rate(info: &crate::probe::MediaInfo, index: u32) -> Option<Rate> {
    use crate::probe::StreamKind;
    let stream = info.streams.iter().find(|s| s.index == index)?;
    if let Some(bits) = stream.bit_rate {
        return Some(Rate {
            bits,
            recorded: true,
        });
    }
    let total = info.container.bit_rate;
    let unrecorded = info.streams.iter().filter(|s| s.bit_rate.is_none()).count();
    let recorded: f64 = info.streams.iter().filter_map(|s| s.bit_rate).sum();
    if unrecorded == 1 {
        return total.map(|total| Rate {
            bits: (total - recorded).max(0.0),
            recorded: true,
        });
    }
    let assumed = |s: &crate::probe::StreamInfo| match &s.kind {
        StreamKind::Audio(audio) => audio
            .channels
            .map(|channels| f64::from(channels) * ASSUMED_KILOBITS_PER_CHANNEL * 1000.0),
        _ => None,
    };
    if let StreamKind::Audio(_) = stream.kind {
        return assumed(stream).map(|bits| Rate {
            bits,
            recorded: false,
        });
    }
    let sounds: f64 = info
        .streams
        .iter()
        .filter(|s| s.bit_rate.is_none())
        .filter_map(assumed)
        .sum();
    let pictures = info
        .streams
        .iter()
        .filter(|s| s.bit_rate.is_none() && matches!(s.kind, StreamKind::Video(_)))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let share = pictures.max(1) as f64;
    total.map(|total| Rate {
        bits: ((total - recorded - sounds) / share).max(0.0),
        recorded: false,
    })
}

fn speed_of(source: &SegmentSource) -> f64 {
    source.speed.value().filter(|s| *s > 0.0).unwrap_or(1.0)
}

fn audio_bits_per_second(encoding: &AudioEncoding) -> f64 {
    let pcm =
        f64::from(encoding.sample_rate) * f64::from(encoding.channels) * PCM_BYTES_PER_SAMPLE * 8.0;
    match encoding.kilobits {
        Some(kilobits) => f64::from(kilobits) * 1000.0,
        None if encoding.codec == "flac" => pcm * FLAC_RATIO,
        None => pcm,
    }
}

fn bytes_text(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64;
    if value >= 1e9 {
        format!("{:.1} GB", value / 1e9)
    } else {
        format!("{:.0} MB", value / 1e6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_needs_a_margin_over_the_estimate() {
        let gigabyte = 1_000_000_000;
        assert!(enough_space(gigabyte, 2 * gigabyte));
        assert!(!enough_space(gigabyte, gigabyte));
        // Five percent and 64 MiB over.
        let twentieth = gigabyte.div_ceil(20);
        assert!(!enough_space(gigabyte, gigabyte + twentieth));
        assert!(enough_space(
            gigabyte,
            gigabyte + twentieth + 64 * 1024 * 1024
        ));
        assert!(!enough_space(0, 1024));
    }
}
