//! What the export does to particular clips, read off the plan (#130, #132):
//! the cost stated at the point of use — the inspector's crop section, the
//! reframe dialog — rather than only in the export dialog afterwards.
//!
//! Everything here is the plan's (#39) own decision, summed per clip; nothing
//! is decided twice, and the renderer only phrases it (`CLAUDE.md` section 2).
//! The inspector asks again after every edit, as the application bar's
//! lossless indicator does, so the statement follows the crop and returns to
//! "copied bit for bit" when it is reset.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use ts_rs::TS;

use super::plan::{Cause, ExportPlan, Media, Segment};
use crate::project::ClipId;
use crate::project::reframe::Reframe;
use crate::tier::{ExportTier, ReEncodeReason};

/// Sequence frames of `plan`, in seconds.
#[allow(clippy::cast_precision_loss)]
fn seconds(plan: &ExportPlan, frames: i64) -> f64 {
    let base = plan.time_base;
    if base.den == 0 {
        return 0.0;
    }
    frames as f64 * base.num as f64 / base.den as f64
}

/// Whether `cause` is a crop: the one re-encode the crop controls answer for.
fn is_crop(cause: &Cause) -> bool {
    matches!(
        cause,
        Cause::Operation {
            reason: ReEncodeReason::FilterChangesPixels
        }
    )
}

fn of_clip(segment: &Segment, clip: ClipId) -> bool {
    segment.sources.iter().any(|source| source.clip == clip)
}

/// What the export does to the sound of some clips.
#[derive(Debug, Clone, PartialEq, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SoundCost {
    pub copied_seconds: f64,
    pub re_encoded_seconds: f64,
    /// Why it is re-encoded, in the engine's words; never the crop, which
    /// does not reach the sound.
    pub reasons: Vec<String>,
}

/// What the export does to the pictures and sound of some clips.
#[derive(Debug, Clone, PartialEq, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipsCost {
    /// The clips asked about whose pictures the export takes.
    pub clips: u32,
    /// Of them, the clips whose pictures are re-encoded because they are
    /// cropped, and for how long.
    pub cropped_clips: u32,
    pub cropped_seconds: f64,
    /// Pictures re-encoded whole for another reason (a reverse, a hold, a
    /// sequence that does not match) and those reasons, in the engine's
    /// words.
    pub other_seconds: f64,
    pub other_reasons: Vec<String>,
    /// Pictures copied, but for a seam re-encoded at a cut between
    /// keyframes.
    pub seamed_seconds: f64,
    /// Pictures copied packet for packet.
    pub copied_seconds: f64,
    /// Part of them cannot be exported as the graph stands.
    pub declined: bool,
    /// Their sound: `None` when none of them has any in the export.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sound: Option<SoundCost>,
}

/// What exporting `plan` does to `clips`, pictures and sound (#130).
#[must_use]
pub fn clips_cost(plan: &ExportPlan, clips: &[ClipId]) -> ClipsCost {
    let wanted: BTreeSet<ClipId> = clips.iter().copied().collect();
    let mut cost = ClipsCost::default();
    let mut pictured = BTreeSet::new();
    let mut cropped = BTreeSet::new();
    let mut sound: Option<SoundCost> = None;
    for segment in &plan.segments {
        let Some(&clip) = wanted.iter().find(|&&clip| of_clip(segment, clip)) else {
            continue;
        };
        let length = seconds(plan, segment.length);
        cost.declined |= segment.decline.is_some();
        match segment.media {
            Media::Video => {
                pictured.insert(clip);
                match segment.tier {
                    ExportTier::StreamCopy => cost.copied_seconds += length,
                    ExportTier::SmartCut { .. } => cost.seamed_seconds += length,
                    ExportTier::FullReEncode { .. } => {
                        if segment.causes.iter().any(is_crop) {
                            cropped.insert(clip);
                            cost.cropped_seconds += length;
                        } else {
                            cost.other_seconds += length;
                        }
                        for cause in segment.causes.iter().filter(|cause| !is_crop(cause)) {
                            let sentence = cause.sentence();
                            if !cost.other_reasons.contains(&sentence) {
                                cost.other_reasons.push(sentence);
                            }
                        }
                    }
                }
            }
            Media::Audio => {
                let sound = sound.get_or_insert_with(SoundCost::default);
                if matches!(segment.tier, ExportTier::FullReEncode { .. }) {
                    sound.re_encoded_seconds += length;
                    for cause in &segment.causes {
                        let sentence = cause.sentence();
                        if !sound.reasons.contains(&sentence) {
                            sound.reasons.push(sentence);
                        }
                    }
                } else {
                    sound.copied_seconds += length;
                }
            }
        }
    }
    cost.clips = u32::try_from(pictured.len()).unwrap_or(u32::MAX);
    cost.cropped_clips = u32::try_from(cropped.len()).unwrap_or(u32::MAX);
    cost.sound = sound;
    cost
}

/// A clip's pictures in a plan: for how long they play, and whether any of
/// them is re-encoded whole.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pictures {
    seconds: f64,
    re_encoded: bool,
}

fn pictures(plan: &ExportPlan) -> BTreeMap<ClipId, Pictures> {
    let mut clips: BTreeMap<ClipId, Pictures> = BTreeMap::new();
    for segment in plan.segments.iter().filter(|s| s.media == Media::Video) {
        for source in &segment.sources {
            let entry = clips.entry(source.clip).or_insert(Pictures {
                seconds: 0.0,
                re_encoded: false,
            });
            entry.seconds += seconds(plan, segment.length);
            entry.re_encoded |= matches!(segment.tier, ExportTier::FullReEncode { .. });
        }
    }
    clips
}

/// What a reframe would cost, stated before it is applied (#132): the plan
/// of the project as it is against the plan of the project it would leave.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReframeImpact {
    pub reframe: Reframe,
    /// Video clips whose pictures are copied now and would be re-encoded:
    /// the reframe's cost.
    pub re_encoded_clips: Vec<ClipId>,
    pub re_encoded_seconds: f64,
    /// Video clips re-encoded now that still would be.
    pub still_re_encoded_clips: Vec<ClipId>,
    /// Video clips whose pictures would be copies afterwards.
    pub copied_clips: Vec<ClipId>,
    pub copied_seconds: f64,
    /// Part of the reframed sequence could not be exported as it stands.
    pub declined: bool,
}

/// What `reframe` costs, from the plans `before` and `after` it.
#[must_use]
pub fn reframe_impact(reframe: Reframe, before: &ExportPlan, after: &ExportPlan) -> ReframeImpact {
    let was = pictures(before);
    let mut impact = ReframeImpact {
        reframe,
        re_encoded_clips: Vec::new(),
        re_encoded_seconds: 0.0,
        still_re_encoded_clips: Vec::new(),
        copied_clips: Vec::new(),
        copied_seconds: 0.0,
        declined: after.segments.iter().any(|s| s.decline.is_some()),
    };
    for (clip, now) in pictures(after) {
        if !now.re_encoded {
            impact.copied_clips.push(clip);
            impact.copied_seconds += now.seconds;
        } else if was.get(&clip).is_some_and(|was| was.re_encoded) {
            impact.still_re_encoded_clips.push(clip);
        } else {
            impact.re_encoded_clips.push(clip);
            impact.re_encoded_seconds += now.seconds;
        }
    }
    impact
}
