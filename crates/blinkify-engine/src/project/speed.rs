//! What a constant speed change does to a clip's pictures (#56, #42): the
//! frame rate it produces, whether that rate can be carried by a copy, and
//! the tier that follows.
//!
//! A constant factor is applied by rescaling packet timestamps, so the
//! pictures are untouched and the video stays a stream copy. What can break
//! that is the rate the rescale produces: 4× of 120 fps is 480 fps, which no
//! container Blinkify writes accepts, and the clip then has to be re-encoded
//! at the sequence's rate — with that reason recorded.
//!
//! [`verdict`] is the **one** place that is decided. The inspector states its
//! answer while the user is choosing a speed, and the export planner (#39)
//! reads the same answer for the same graph; neither works it out again.
//! ADR-0009 records the limits and why they are where they are.

use serde::Serialize;
use ts_rs::TS;

use super::evaluate::Placement;
use super::settings::{FRAME_RATE_RANGE, StreamGeometry, reduced};
use crate::probe::Rational;
use crate::tier::{ExportTier, ReEncodeReason};

/// The slowest and fastest constant speed a clip may have: a tenth of normal
/// and a hundred times normal: the range users arriving from `CapCut` know.
pub const SPEED_RANGE: (Rational, Rational) =
    (Rational { num: 1, den: 10 }, Rational { num: 100, den: 1 });

/// Below this many frames a second a slowed clip reads as a slideshow.
/// Blinkify does not interpolate frames, so it says so rather than calling
/// the result smooth; the copy itself is still lossless.
pub const SMOOTH_MOTION_FLOOR: i64 = 12;

/// Something about a speed the user should know before exporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "problem",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum SpeedProblem {
    /// The rescaled rate is outside what a video file can carry
    /// ([`FRAME_RATE_RANGE`]): the clip is re-encoded at the sequence rate.
    FrameRateOutsideContainer { rate: Rational },
    /// The rescaled rate is below [`SMOOTH_MOTION_FLOOR`]: copied losslessly,
    /// but every frame is held longer and motion stutters.
    BelowSmoothMotion { rate: Rational },
}

/// A clip's speed, and what exporting it at that speed does to its pictures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpeedVerdict {
    /// Reduced: `3/2`, never `6/4`.
    pub speed: Rational,
    /// The clip's frame rate after the change: the source's nominal rate
    /// times the speed, exactly.
    pub output_frame_rate: Rational,
    /// The source's rate varies, so `output_frame_rate` is nominal and every
    /// packet is rescaled on its own timestamp, never snapped to a grid.
    pub variable_frame_rate: bool,
    /// What the speed does to the pictures: a stream copy, or a re-encode
    /// with its reason. Nothing else about the clip is decided here — a
    /// reverse, a hold or a geometry mismatch is its own reason.
    pub tier: ExportTier,
    pub problems: Vec<SpeedProblem>,
}

/// Whether `speed` is one a clip may be given.
#[must_use]
pub fn in_range(speed: Rational) -> bool {
    let (low, high) = SPEED_RANGE;
    speed.num > 0 && speed.den > 0 && at_least(speed, low) && at_least(high, speed)
}

/// `a >= b`, exactly.
fn at_least(a: Rational, b: Rational) -> bool {
    i128::from(a.num) * i128::from(b.den) >= i128::from(b.num) * i128::from(a.den)
}

/// What exporting `placement` at its speed does to its pictures, given what
/// its source's pictures are.
#[must_use]
pub fn verdict(placement: &Placement, source: &StreamGeometry) -> SpeedVerdict {
    let speed = reduced(placement.speed);
    let rate = source.frame_rate;
    let output_frame_rate = reduced(Rational {
        num: rate.num.saturating_mul(speed.num),
        den: rate.den.saturating_mul(speed.den),
    });
    let (low, high) = FRAME_RATE_RANGE;
    let mut problems = Vec::new();
    let carried = at_least(output_frame_rate, Rational { num: low, den: 1 })
        && at_least(Rational { num: high, den: 1 }, output_frame_rate);
    if !carried {
        problems.push(SpeedProblem::FrameRateOutsideContainer {
            rate: output_frame_rate,
        });
    } else if !at_least(
        output_frame_rate,
        Rational {
            num: SMOOTH_MOTION_FLOOR,
            den: 1,
        },
    ) {
        problems.push(SpeedProblem::BelowSmoothMotion {
            rate: output_frame_rate,
        });
    }
    SpeedVerdict {
        speed,
        output_frame_rate,
        variable_frame_rate: source.variable_frame_rate,
        tier: if carried {
            ExportTier::StreamCopy
        } else {
            ExportTier::FullReEncode {
                reason: ReEncodeReason::SpeedFrameRateOutsideContainer,
            }
        },
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TrackKind;

    fn rational(num: i64, den: i64) -> Rational {
        Rational { num, den }
    }

    fn source(rate: Rational, variable: bool) -> StreamGeometry {
        StreamGeometry {
            width: 1920,
            height: 1080,
            frame_rate: rate,
            pixel_aspect: rational(1, 1),
            variable_frame_rate: variable,
            hdr: false,
        }
    }

    fn at(speed: Rational) -> Placement {
        Placement {
            track: 1,
            kind: TrackKind::Video,
            clip: 1,
            source: 1,
            stream: 0,
            time_base: rational(1, 30_000),
            source_in: 0,
            source_out: 300_000,
            start: 0,
            length: 300,
            speed,
            audio: Vec::new(),
            sequence_time_base: rational(1, 30),
            motion: None,
            silent: false,
            forced: None,
        }
    }

    #[test]
    fn normal_speed_is_a_copy_at_the_source_rate() {
        let verdict = verdict(&at(rational(1, 1)), &source(rational(30, 1), false));
        assert_eq!(verdict.tier, ExportTier::StreamCopy);
        assert_eq!(verdict.output_frame_rate, rational(30, 1));
        assert!(verdict.problems.is_empty());
    }

    #[test]
    fn double_speed_rescales_the_rate_exactly() {
        let ntsc = source(rational(30_000, 1001), false);
        let verdict = verdict(&at(rational(4, 2)), &ntsc);
        assert_eq!(verdict.speed, rational(2, 1));
        assert_eq!(verdict.output_frame_rate, rational(60_000, 1001));
        assert_eq!(verdict.tier, ExportTier::StreamCopy);
    }

    #[test]
    fn the_container_ceiling_is_inclusive() {
        // 4× of 60 fps is 240 fps: the highest rate a file can carry.
        let verdict = verdict(&at(rational(4, 1)), &source(rational(60, 1), false));
        assert_eq!(verdict.tier, ExportTier::StreamCopy);
    }

    #[test]
    fn a_rate_no_container_accepts_is_re_encoded_with_its_reason() {
        let verdict = verdict(&at(rational(4, 1)), &source(rational(120, 1), false));
        assert_eq!(
            verdict.tier,
            ExportTier::FullReEncode {
                reason: ReEncodeReason::SpeedFrameRateOutsideContainer
            }
        );
        assert_eq!(
            verdict.problems,
            vec![SpeedProblem::FrameRateOutsideContainer {
                rate: rational(480, 1)
            }]
        );
    }

    #[test]
    fn extreme_speed_up_and_slowdown_are_bounded() {
        // A hundredfold speed-up of 30 fps: 3000 fps, re-encoded.
        let fast = verdict(&at(rational(100, 1)), &source(rational(30, 1), false));
        assert!(!fast.tier.is_lossless());
        // A tenth of 24 fps is 2.4 fps: still carried, still a copy, and
        // said to stutter rather than called smooth.
        let slow = verdict(&at(rational(1, 10)), &source(rational(24, 1), false));
        assert_eq!(slow.tier, ExportTier::StreamCopy);
        assert_eq!(
            slow.problems,
            vec![SpeedProblem::BelowSmoothMotion {
                rate: rational(12, 5)
            }]
        );
        // A tenth of 5 fps is half a frame a second: no file carries it.
        let slower = verdict(&at(rational(1, 10)), &source(rational(5, 1), false));
        assert!(!slower.tier.is_lossless());
    }

    #[test]
    fn slow_motion_from_a_high_rate_source_is_smooth_and_lossless() {
        // The feature in one line: 240 fps phone footage at an eighth of
        // normal speed plays at 30 fps with every recorded packet intact.
        let verdict = verdict(&at(rational(1, 8)), &source(rational(240, 1), false));
        assert_eq!(verdict.output_frame_rate, rational(30, 1));
        assert_eq!(verdict.tier, ExportTier::StreamCopy);
        assert!(verdict.problems.is_empty());
    }

    #[test]
    fn a_variable_rate_source_is_rescaled_per_packet() {
        let verdict = verdict(&at(rational(2, 1)), &source(rational(30, 1), true));
        assert!(verdict.variable_frame_rate);
        assert_eq!(verdict.tier, ExportTier::StreamCopy);
    }

    #[test]
    fn the_speed_range_is_a_tenth_to_a_hundredfold() {
        assert!(in_range(rational(1, 10)));
        assert!(in_range(rational(100, 1)));
        assert!(in_range(rational(3, 2)));
        assert!(!in_range(rational(1, 11)));
        assert!(!in_range(rational(101, 1)));
        assert!(!in_range(rational(0, 1)));
        assert!(!in_range(rational(-1, 1)));
    }
}
