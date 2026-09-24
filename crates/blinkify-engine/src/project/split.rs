//! Cutting a clip in two, and where a cut is lossless (#35).
//!
//! A split must give two clips whose extents on the timeline add up to the
//! original exactly, and whose source ranges meet without a gap or an
//! overlap — no frame dropped, none duplicated at the cut.
//!
//! Whether a cut is lossless is a fact about the source: the frame on screen
//! at the cut must be a keyframe a stream copy can start from. That fact comes
//! from the keyframe index (#24); [`cut_point`] states it in timeline frames,
//! for the indicator the user sees while editing.

use serde::Serialize;
use ts_rs::TS;

use super::ClipId;
use super::evaluate::{Motion, Placement};

use crate::keyframes::{GopKind, Keyframe};
use crate::time::{Rounding, rescale};

/// One half of a split: its source range, start and length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Half {
    pub from: i64,
    pub to: i64,
    pub start: i64,
    pub length: i64,
}

/// The two halves of `placement` cut at sequence frame `at`, which must be
/// strictly inside it. `None` at an edge, outside it, or on overflow.
pub(super) fn halves(placement: &Placement, at: i64) -> Option<(Half, Half)> {
    let k = at - placement.start;
    if k <= 0 || k >= placement.length {
        return None;
    }
    let n = placement.length;
    if placement.motion == Some(Motion::Hold) {
        let (from, to) = (placement.source_in, placement.source_out);
        return Some((
            Half {
                from,
                to,
                start: placement.start,
                length: k,
            },
            Half {
                from,
                to,
                start: at,
                length: n - k,
            },
        ));
    }
    // The cut in the source is the tick on screen at the cut, as the
    // evaluator reads it: `k` frames of the clip, rounded down. Both halves
    // meet there, so every source tick is in exactly one of them — nothing
    // dropped, nothing shown twice. If the far half's partial last frame
    // would make it a frame too long, that sub-frame tail is left off, so
    // the halves together end exactly where the clip did.
    let played = placement.played_time_base();
    let sequence = placement.sequence_time_base;
    let span = |frames: i64| rescale(frames, sequence, played, Rounding::Down);
    let frames = |ticks: i64| rescale(ticks, played, sequence, Rounding::Up);
    let (left, right) = if placement.motion == Some(Motion::Reverse) {
        // Backwards, the left half plays the end of the source.
        let cut = placement.source_out.checked_sub(span(k)?)?;
        let from = placement.source_in.max(cut.checked_sub(span(n - k)?)?);
        (
            Half {
                from: cut,
                to: placement.source_out,
                start: placement.start,
                length: frames(placement.source_out - cut)?,
            },
            Half {
                from,
                to: cut,
                start: at,
                length: frames(cut - from)?,
            },
        )
    } else {
        let cut = placement.source_in.checked_add(span(k)?)?;
        let to = placement.source_out.min(cut.checked_add(span(n - k)?)?);
        (
            Half {
                from: placement.source_in,
                to: cut,
                start: placement.start,
                length: frames(cut - placement.source_in)?,
            },
            Half {
                from: cut,
                to,
                start: at,
                length: frames(to - cut)?,
            },
        )
    };
    (left.length == k && right.length == n - k && left.from < left.to && right.from < right.to)
        .then_some((left, right))
}

/// Whether a cut at a sequence frame is lossless, and where the keyframes
/// around it are — what the timeline's keyframe indicator shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CutPoint {
    pub clip: ClipId,
    /// Sequence frames.
    #[ts(type = "number")]
    pub position: i64,
    /// The frame on screen here is a keyframe a copy can start from: a split
    /// here costs nothing at export.
    pub lossless: bool,
    /// The keyframe here is an open-GOP one: pictures after it depend on the
    /// previous group, so a copy starting here is not clean.
    pub open_gop: bool,
    /// The nearest keyframe at or before, in sequence frames, inside the clip.
    #[ts(type = "number | null")]
    pub previous: Option<i64>,
    /// The nearest keyframe after, in sequence frames, inside the clip.
    #[ts(type = "number | null")]
    pub next: Option<i64>,
}

/// The sequence frame at which source tick `pts` first shows in a forward
/// placement, if it shows at all.
fn frame_of(placement: &Placement, pts: i64) -> Option<i64> {
    if pts < placement.source_in || pts >= placement.source_out {
        return None;
    }
    let offset = rescale(
        pts - placement.source_in,
        placement.played_time_base(),
        placement.sequence_time_base,
        Rounding::Up,
    )?;
    let frame = placement.start.checked_add(offset)?;
    placement.covers(frame).then_some(frame)
}

/// State the cut at `position` in a forward `placement`, given the frame on
/// screen there (`shown`, a source tick) and the keyframes at or before it
/// and after it.
#[must_use]
pub fn cut_point(
    placement: &Placement,
    position: i64,
    shown: i64,
    before: Option<Keyframe>,
    after: Option<Keyframe>,
) -> CutPoint {
    let at_keyframe = before.filter(|keyframe| keyframe.pts == shown);
    let open_gop = at_keyframe
        .is_some_and(|keyframe| keyframe.gop == GopKind::Open || keyframe.has_leading_pictures);
    CutPoint {
        clip: placement.clip,
        position,
        lossless: at_keyframe.is_some() && !open_gop,
        open_gop,
        previous: before.and_then(|keyframe| frame_of(placement, keyframe.pts)),
        next: after
            .filter(|keyframe| keyframe.pts > shown)
            .and_then(|keyframe| frame_of(placement, keyframe.pts)),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::probe::Rational;
    use crate::project::TrackKind;

    fn placement(source_in: i64, source_out: i64, motion: Option<Motion>) -> Placement {
        let mut placement = Placement {
            track: 1,
            kind: TrackKind::Video,
            clip: 1,
            source: 1,
            stream: 0,
            time_base: Rational { num: 1, den: 1000 },
            source_in,
            source_out,
            start: 20,
            length: 0,
            speed: Rational { num: 1, den: 1 },
            audio: Vec::new(),
            sequence_time_base: Rational { num: 1, den: 30 },
            motion,
            forced: None,
            silent: false,
        };
        placement.length = rescale(
            source_out - source_in,
            placement.played_time_base(),
            placement.sequence_time_base,
            Rounding::Up,
        )
        .expect("length");
        if motion == Some(Motion::Hold) {
            placement.length = 90;
        }
        placement
    }

    #[test]
    fn a_split_neither_drops_nor_repeats_a_frame() {
        for source_in in [0, 17, 33, 500] {
            for source_out in [1000, 1017, 1033, 2011] {
                for motion in [None, Some(Motion::Reverse), Some(Motion::Hold)] {
                    let clip = placement(source_in, source_out, motion);
                    for at in (clip.start + 1)..clip.end() {
                        let (left, right) = halves(&clip, at).unwrap_or_else(|| {
                            panic!("{source_in}..{source_out} {motion:?} at {at}")
                        });
                        assert_eq!(left.start, clip.start);
                        assert_eq!(left.start + left.length, at);
                        assert_eq!(right.start, at);
                        assert_eq!(right.start + right.length, clip.end());
                        // Less than a frame of source may be left off the far
                        // end; nothing else.
                        let frame = 34;
                        match motion {
                            None => {
                                assert_eq!(left.from, clip.source_in);
                                assert_eq!(left.to, right.from, "contiguous");
                                assert!(clip.source_out - right.to < frame);
                            }
                            Some(Motion::Reverse) => {
                                assert_eq!(left.to, clip.source_out);
                                assert_eq!(right.to, left.from, "contiguous");
                                assert!(right.from - clip.source_in < frame);
                            }
                            Some(Motion::Hold) => assert_eq!(left.from, right.from),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_splits_at_an_edge_or_outside() {
        let clip = placement(0, 1000, None);
        assert!(halves(&clip, clip.start).is_none());
        assert!(halves(&clip, clip.end()).is_none());
        assert!(halves(&clip, clip.end() + 5).is_none());
    }

    fn keyframe(pts: i64, gop: GopKind) -> Keyframe {
        Keyframe {
            pts,
            dts: Some(pts),
            pos: None,
            gop,
            picture: None,
            has_leading_pictures: false,
        }
    }

    #[test]
    fn a_cut_on_a_closed_keyframe_is_lossless_and_between_them_is_not() {
        // 90 kHz source at 30 fps: 3000 ticks a frame, keyframes every 30.
        let mut clip = placement(0, 900_000, None);
        clip.time_base = Rational {
            num: 1,
            den: 90_000,
        };
        clip.length = 300;
        let on = cut_point(
            &clip,
            50,
            90_000,
            Some(keyframe(90_000, GopKind::Closed)),
            Some(keyframe(90_000, GopKind::Closed)),
        );
        assert!(on.lossless);
        assert_eq!(on.previous, Some(50));
        let between = cut_point(
            &clip,
            55,
            105_000,
            Some(keyframe(90_000, GopKind::Closed)),
            Some(keyframe(180_000, GopKind::Closed)),
        );
        assert!(!between.lossless);
        assert_eq!((between.previous, between.next), (Some(50), Some(80)));
        let open = cut_point(
            &clip,
            50,
            90_000,
            Some(keyframe(90_000, GopKind::Open)),
            None,
        );
        assert!(!open.lossless && open.open_gop);
    }
}
