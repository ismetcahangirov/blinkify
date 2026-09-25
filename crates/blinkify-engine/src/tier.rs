//! The three export tiers, and the rule that orders them.
//!
//! This module is small on purpose. It is the type-level statement of the
//! product's binding constraint, and Epic #6 builds the planner on top of it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Why a segment could not be stream-copied whole and needed a seam encode.
///
/// A smart-cut always carries one. `CLAUDE.md` forbidden behaviour 2: no
/// re-encode without a recorded reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum SeamReason {
    /// The in-point falls inside a GOP rather than on its keyframe.
    InPointNotKeyframeAligned,
    /// The out-point falls inside a GOP rather than on its keyframe.
    OutPointNotKeyframeAligned,
    /// The source uses open GOPs, so the boundary frame depends on a picture
    /// outside the segment.
    OpenGopBoundary,
}

/// Why a segment had to be decoded, filtered and re-encoded in full.
///
/// Tier 3 always carries one, it is stored with the plan, and it reaches the
/// user in the export report. A re-encode nobody can explain is a bug by
/// definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum ReEncodeReason {
    /// A filter changes the pixels — crop, scale, a burned overlay.
    FilterChangesPixels,
    /// Segments from sources with incompatible parameters are concatenated.
    IncompatibleSourceParameters,
    /// The user asked for an output codec the source is not already in.
    OutputCodecDiffersFromSource,
    /// No encoder on this machine can produce a seam that matches the source,
    /// and the user chose a full re-encode over moving the cut. See `ADR-0003`.
    NoEncoderMatchesSourceProfile,
    /// A held frame (#35): a picture shown for longer than the source ever
    /// showed it has to be encoded.
    FreezeFrame,
    /// A reversed clip (#35): its pictures are decoded and encoded again in
    /// the opposite order.
    Reverse,
    /// A constant speed (#42, #56) whose rescaled frame rate no container
    /// accepts: the pictures are re-timed to the sequence rate instead.
    SpeedFrameRateOutsideContainer,
}

/// What happens to a segment's pixels on the way to the output file.
///
/// The variants are ordered by fidelity: [`ExportTier::StreamCopy`] is lossless,
/// [`ExportTier::SmartCut`] loses a fraction of a second at the seam, and
/// [`ExportTier::FullReEncode`] re-encodes everything.
///
/// Note what is not representable: a lossy tier without a reason. That is
/// deliberate — forbidden behaviour 2 is enforced by the type, not by a review
/// checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case", tag = "tier")]
#[ts(export)]
pub enum ExportTier {
    /// Packets are copied byte-for-byte. Zero generation loss.
    StreamCopy,
    /// Only the partial GOP at the boundary is re-encoded; the interior is
    /// copied.
    SmartCut { reason: SeamReason },
    /// The segment is decoded, filtered and re-encoded.
    FullReEncode { reason: ReEncodeReason },
}

impl ExportTier {
    /// The tier's rank: 1, 2 or 3, matching the numbering in `CLAUDE.md`.
    ///
    /// Lower is better. This is the ordering the planner minimises over.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::StreamCopy => 1,
            Self::SmartCut { .. } => 2,
            Self::FullReEncode { .. } => 3,
        }
    }

    /// Whether the pixels reach the output file unchanged.
    #[must_use]
    pub const fn is_lossless(self) -> bool {
        matches!(self, Self::StreamCopy)
    }

    /// The lowest tier among `candidates` — the one the planner must pick.
    ///
    /// Returns `None` for an empty set, which is a caller bug rather than a
    /// planning outcome: a segment with no viable tier cannot be exported at
    /// all and must be declined before it reaches here.
    #[must_use]
    pub fn lowest(candidates: impl IntoIterator<Item = Self>) -> Option<Self> {
        candidates.into_iter().min_by_key(|tier| tier.rank())
    }
}

#[cfg(test)]
// `expect` is denied in engine code because a panic on hostile input is a
// crash on a user's project. In a test it is the assertion.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const SMART_CUT: ExportTier = ExportTier::SmartCut {
        reason: SeamReason::InPointNotKeyframeAligned,
    };
    const RE_ENCODE: ExportTier = ExportTier::FullReEncode {
        reason: ReEncodeReason::FilterChangesPixels,
    };

    #[test]
    fn tiers_rank_by_fidelity() {
        assert_eq!(ExportTier::StreamCopy.rank(), 1);
        assert_eq!(SMART_CUT.rank(), 2);
        assert_eq!(RE_ENCODE.rank(), 3);
    }

    #[test]
    fn only_stream_copy_is_lossless() {
        assert!(ExportTier::StreamCopy.is_lossless());
        assert!(!SMART_CUT.is_lossless());
        assert!(!RE_ENCODE.is_lossless());
    }

    #[test]
    fn the_planner_picks_the_lowest_viable_tier() {
        // The whole product in one assertion: given a choice, copy.
        assert_eq!(
            ExportTier::lowest([RE_ENCODE, ExportTier::StreamCopy, SMART_CUT]),
            Some(ExportTier::StreamCopy),
        );
        assert_eq!(ExportTier::lowest([RE_ENCODE, SMART_CUT]), Some(SMART_CUT));
    }

    #[test]
    fn no_viable_tier_is_not_a_silent_re_encode() {
        // Falling back to tier 3 when nothing is viable is exactly the silent
        // degradation forbidden behaviour 3 prohibits. The caller must decline.
        assert_eq!(ExportTier::lowest([]), None);
    }

    #[test]
    fn a_lossy_tier_serialises_with_its_reason() {
        let json = serde_json::to_string(&SMART_CUT).expect("serialises");
        assert!(json.contains("smart-cut"), "{json}");
        assert!(json.contains("in-point-not-keyframe-aligned"), "{json}");
    }
}
