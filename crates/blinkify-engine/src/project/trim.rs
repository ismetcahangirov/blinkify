//! Clip timing in sequence frames, done exactly (#34).
//!
//! The timeline speaks in sequence frames — a trim handle dragged three
//! frames, a clip moved to frame 120 — and the graph stores source ticks.
//! Converting between them is where a cut drifts a frame, so it happens here,
//! once, in integer arithmetic through [`rescale`], with the rounding chosen
//! so the edge the user did not touch stays exactly where it was:
//!
//! - trimming the **start** moves the source in-point by the ticks of the
//!   frames, rounded up, then nudges it by a tick if a sub-frame remainder
//!   would move the far edge;
//! - trimming the **end** sets the out-point to the whole number of frames
//!   the clip should now last, rounded down, so the clip lasts exactly that.
//!
//! Every trim is bounded by the source's extent and by one frame of length,
//! and — unless it ripples — by the neighbouring clips. A bound that stops
//! the trim is reported as `clamped`, so the timeline can show the user they
//! hit it rather than silently doing less.

use serde::Serialize;
use ts_rs::TS;

use super::evaluate::Placement;
use super::{Clip, Operation, SourceId};
use crate::probe::Rational;
use crate::time::{Rounding, rescale};

/// The ticks of a source stream that exist: `start..end` in its time base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StreamExtent {
    pub source: SourceId,
    pub stream: u32,
    pub time_base: Rational,
    #[ts(type = "number")]
    pub start: i64,
    /// The first tick past the end.
    #[ts(type = "number")]
    pub end: i64,
}

/// A source extent for a stream whose extent is not known: far enough that
/// no trim reaches it, near enough that arithmetic on it cannot overflow.
const UNBOUNDED: i64 = 1 << 60;

/// Which edge of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Edge {
    Start,
    End,
}

/// A clip's new source range and place, and whether a bound stopped it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Trimmed {
    pub from: i64,
    pub to: i64,
    pub start: i64,
    /// Sequence frames.
    pub length: i64,
    pub clamped: bool,
}

/// How many sequence frames `ticks` of the placement's source last: a
/// partial last frame counts, as the evaluator counts it.
fn frames(placement: &Placement, ticks: i64) -> Option<i64> {
    rescale(
        ticks,
        placement.played_time_base(),
        placement.sequence_time_base,
        Rounding::Up,
    )
}

fn ticks(placement: &Placement, frames: i64, rounding: Rounding) -> Option<i64> {
    rescale(
        frames,
        placement.sequence_time_base,
        placement.played_time_base(),
        rounding,
    )
}

/// Trim `edge` of `placement` by `delta` sequence frames — positive is
/// later — within the source `extent` (source ticks, when known) and the
/// timeline `room`: the frames the clip may start at or after, and end at or
/// before. `None` when the arithmetic does not fit in 64 bits.
pub(super) fn trim(
    placement: &Placement,
    edge: Edge,
    delta: i64,
    extent: Option<(i64, i64)>,
    room: (Option<i64>, Option<i64>),
) -> Option<Trimmed> {
    let (lowest, highest) = extent.unwrap_or((i64::MIN, i64::MAX));
    let end = placement.end();
    match edge {
        Edge::Start => {
            // The start may move no earlier than the room and the source
            // allow, and no later than one frame before the end.
            let mut wanted = delta.min(placement.length - 1);
            if let Some(earliest) = room.0 {
                wanted = wanted.max(earliest - placement.start);
            }
            let mut from =
                placement
                    .source_in
                    .checked_add(ticks(placement, wanted, Rounding::Up)?)?;
            let mut clamped = wanted != delta;
            if from < lowest {
                from = lowest;
                clamped = true;
            }
            // A sub-frame remainder at the out-point can leave the clip a
            // frame off the length intended. Move the in-point a tick at a
            // time until it is exact; the end is kept where it was either
            // way, because the start is worked out from it.
            let mut length = frames(placement, placement.source_out - from)?;
            let target = end - (placement.start + wanted);
            let mut guard = 0;
            while !clamped && length != target && guard < 4 {
                if length > target {
                    from += 1;
                } else if from > lowest {
                    from -= 1;
                } else {
                    // The source has no whole frame left before the in-point.
                    clamped = true;
                    break;
                }
                length = frames(placement, placement.source_out - from)?;
                guard += 1;
            }
            Some(Trimmed {
                from,
                to: placement.source_out,
                start: end - length,
                length,
                clamped,
            })
        }
        Edge::End => {
            let mut wanted = delta.max(1 - placement.length);
            if let Some(latest) = room.1 {
                wanted = wanted.min(latest - end);
            }
            let mut clamped = wanted != delta;
            let length = placement.length + wanted;
            let mut to =
                placement
                    .source_in
                    .checked_add(ticks(placement, length, Rounding::Down)?)?;
            if to > highest {
                to = highest;
                clamped = true;
            }
            if to <= placement.source_in {
                to = placement.source_in + 1;
                clamped = true;
            }
            Some(Trimmed {
                from: placement.source_in,
                to,
                start: placement.start,
                length: frames(placement, to - placement.source_in)?,
                clamped,
            })
        }
    }
}

/// How far, in sequence frames, `edge` of `placement` can move before the
/// source runs out — `(earliest, latest)` deltas, each clamped to the
/// one-frame minimum length.
pub(super) fn reach(placement: &Placement, edge: Edge, extent: Option<(i64, i64)>) -> (i64, i64) {
    let (lowest, highest) = extent.unwrap_or((-UNBOUNDED, UNBOUNDED));
    match edge {
        Edge::Start => {
            let before = frames(placement, placement.source_in - lowest).unwrap_or(0);
            // Whole frames only: a partial one is not a frame to move to.
            let before = before
                - i64::from(
                    ticks(placement, before, Rounding::Up).unwrap_or(0)
                        > placement.source_in - lowest,
                );
            (-before.max(0), placement.length - 1)
        }
        Edge::End => {
            let after = rescale(
                highest - placement.source_in,
                placement.played_time_base(),
                placement.sequence_time_base,
                Rounding::Down,
            )
            .unwrap_or(placement.length);
            (1 - placement.length, (after - placement.length).max(0))
        }
    }
}

/// `clip` playing `from..to` at `start`: every trim replaced by one, where
/// the first one was, or first.
pub(super) fn retimed(clip: &Clip, from: i64, to: i64, start: i64) -> Clip {
    let mut operations = Vec::with_capacity(clip.operations.len() + 1);
    let mut placed = false;
    for operation in &clip.operations {
        if matches!(operation, Operation::Trim { .. }) {
            if !placed {
                operations.push(Operation::Trim { from, to });
                placed = true;
            }
        } else {
            operations.push(*operation);
        }
    }
    if !placed {
        operations.insert(0, Operation::Trim { from, to });
    }
    Clip {
        start,
        operations,
        ..clip.clone()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::project::TrackKind;

    /// A clip of a 1/1000 source at 30 fps: 33⅓ ticks a frame, so every
    /// conversion has a remainder.
    fn placement(source_in: i64, source_out: i64, start: i64) -> Placement {
        let sequence = Rational { num: 1, den: 30 };
        let mut placement = Placement {
            track: 1,
            kind: TrackKind::Video,
            clip: 1,
            source: 1,
            stream: 0,
            time_base: Rational { num: 1, den: 1000 },
            source_in,
            source_out,
            start,
            length: 0,
            speed: Rational { num: 1, den: 1 },
            audio: Vec::new(),
            sequence_time_base: sequence,
        };
        placement.length = frames(&placement, source_out - source_in).expect("frames");
        placement
    }

    #[test]
    fn a_start_trim_moves_the_start_and_leaves_the_end_exactly_where_it_was() {
        for source_in in [0, 1, 17, 33, 34, 500] {
            for source_out in [1000, 1001, 1017, 1033, 2000] {
                let clip = placement(source_in, source_out, 60);
                for delta in -10..clip.length {
                    let trimmed = trim(&clip, Edge::Start, delta, Some((0, 5000)), (None, None))
                        .expect("fits");
                    assert_eq!(
                        trimmed.start + trimmed.length,
                        clip.end(),
                        "in {source_in} out {source_out} delta {delta}: {trimmed:?}"
                    );
                    if !trimmed.clamped {
                        assert_eq!(trimmed.start, clip.start + delta, "delta {delta}");
                    }
                    assert!(trimmed.length >= 1);
                }
            }
        }
    }

    #[test]
    fn an_end_trim_makes_the_clip_last_exactly_the_frames_asked() {
        let clip = placement(17, 1017, 0);
        for delta in (1 - clip.length)..40 {
            let trimmed =
                trim(&clip, Edge::End, delta, Some((0, 5000)), (None, None)).expect("fits");
            assert!(!trimmed.clamped, "delta {delta}");
            assert_eq!(trimmed.length, clip.length + delta, "delta {delta}");
            assert_eq!(trimmed.start, 0);
        }
    }

    #[test]
    fn a_trim_past_the_source_stops_at_it_and_says_so() {
        let clip = placement(100, 1000, 30);
        let early = trim(&clip, Edge::Start, -20, Some((0, 1100)), (None, None)).expect("fits");
        assert!(early.clamped);
        assert_eq!(early.from, 0);
        assert_eq!(early.start + early.length, clip.end());
        let late = trim(&clip, Edge::End, 20, Some((0, 1100)), (None, None)).expect("fits");
        assert!(late.clamped);
        assert_eq!(late.to, 1100);
        // Never shorter than a frame.
        let short = trim(&clip, Edge::End, -1000, Some((0, 1100)), (None, None)).expect("fits");
        assert!(short.clamped);
        assert_eq!(short.length, 1);
    }

    #[test]
    fn a_trim_stops_at_its_neighbour_unless_it_ripples() {
        let clip = placement(1000, 1900, 30);
        let blocked = trim(
            &clip,
            Edge::End,
            10,
            Some((0, 5000)),
            (None, Some(clip.end() + 4)),
        )
        .expect("fits");
        assert!(blocked.clamped);
        assert_eq!(blocked.start + blocked.length, clip.end() + 4);
        let blocked =
            trim(&clip, Edge::Start, -10, Some((0, 5000)), (Some(25), None)).expect("fits");
        assert!(blocked.clamped);
        assert_eq!(blocked.start, 25);
    }

    #[test]
    fn the_reach_of_an_edge_is_what_the_source_has_left() {
        let clip = placement(100, 1000, 30);
        // 100 ticks before: three whole frames of 33⅓.
        assert_eq!(reach(&clip, Edge::Start, Some((0, 1100))).0, -3);
        // 100 ticks after: three whole frames.
        assert_eq!(reach(&clip, Edge::End, Some((0, 1100))).1, 3);
        assert_eq!(reach(&clip, Edge::End, Some((0, 1000))).1, 0);
    }
}
