//! Exact time arithmetic.
//!
//! Every timestamp in a media file is an integer count of ticks of a rational
//! time base, and every conversion between two time bases is a rounding. Done
//! in floating-point seconds, the roundings drift, and a cut or a frame step
//! eventually lands one frame from where it was put. So conversions are done
//! here, in 128-bit integers, with the rounding direction stated at every call
//! site — because which way to round is a decision, not a detail: a frame's
//! place on the timeline rounds *up* and a timeline position's frame rounds
//! *down*, so that the two round-trip onto the same frame.

use crate::probe::Rational;

/// Which way a conversion rounds when it is not exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    /// Towards negative infinity.
    Down,
    /// Towards positive infinity.
    Up,
    /// To the nearest tick, halves away from zero.
    Nearest,
}

/// Microseconds: the time base of the playback timeline.
pub const MICROSECONDS: Rational = Rational {
    num: 1,
    den: 1_000_000,
};

/// `value` ticks of `from`, in ticks of `to`. `None` for a zero or negative
/// time base, or a result outside `i64`.
#[must_use]
pub fn rescale(value: i64, from: Rational, to: Rational, rounding: Rounding) -> Option<i64> {
    if from.num <= 0 || from.den <= 0 || to.num <= 0 || to.den <= 0 {
        return None;
    }
    if from == to {
        return Some(value);
    }
    // value * from.num / from.den * to.den / to.num
    let numerator = i128::from(value) * i128::from(from.num) * i128::from(to.den);
    let denominator = i128::from(from.den) * i128::from(to.num);
    let floor = numerator.div_euclid(denominator);
    let remainder = numerator.rem_euclid(denominator);
    let rounded = match rounding {
        Rounding::Down => floor,
        Rounding::Up if remainder == 0 => floor,
        Rounding::Up => floor + 1,
        Rounding::Nearest => {
            // Halves away from zero: for a positive value a remainder of
            // exactly half rounds up; for a negative one it rounds down.
            let twice = remainder * 2;
            if twice > denominator || (twice == denominator && numerator > 0) {
                floor + 1
            } else {
                floor
            }
        }
    };
    i64::try_from(rounded).ok()
}

/// `ticks` of `time_base` as seconds, for display and for FFmpeg options that
/// take seconds. Never feed the result back into timestamp arithmetic.
#[must_use]
pub fn seconds(ticks: i64, time_base: Rational) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let ticks = ticks as f64;
    ticks * time_base.value().unwrap_or(0.0)
}

/// `seconds` in ticks of `time_base`, rounded as asked. For values that arrive
/// as seconds — a probe's start time, a duration — never for a timestamp
/// that exists as ticks.
#[must_use]
pub fn from_seconds(seconds: f64, time_base: Rational, rounding: Rounding) -> i64 {
    let per_tick = time_base.value().filter(|v| *v > 0.0).unwrap_or(1.0);
    let ticks = seconds / per_tick;
    let rounded = match rounding {
        Rounding::Down => ticks.floor(),
        Rounding::Up => ticks.ceil(),
        Rounding::Nearest => ticks.round(),
    };
    // `as` saturates, and NaN becomes zero: both are the right answer for a
    // value that did not fit or did not exist.
    #[allow(clippy::cast_possible_truncation)]
    let rounded = rounded as i64;
    rounded
}

#[cfg(test)]
mod tests {
    use super::*;

    const NTSC: Rational = Rational { num: 1, den: 30000 };
    const MP4: Rational = Rational { num: 1, den: 15360 };
    const MS: Rational = Rational { num: 1, den: 1000 };

    #[test]
    fn equal_time_bases_are_exact() {
        assert_eq!(rescale(-7, MP4, MP4, Rounding::Up), Some(-7));
    }

    #[test]
    fn each_rounding_goes_the_way_it_says() {
        // 1001 / 30000 s = 33366.66… µs
        assert_eq!(
            rescale(1001, NTSC, MICROSECONDS, Rounding::Down),
            Some(33366)
        );
        assert_eq!(rescale(1001, NTSC, MICROSECONDS, Rounding::Up), Some(33367));
        assert_eq!(
            rescale(1001, NTSC, MICROSECONDS, Rounding::Nearest),
            Some(33367)
        );
        assert_eq!(
            rescale(-1001, NTSC, MICROSECONDS, Rounding::Down),
            Some(-33367)
        );
        assert_eq!(
            rescale(-1001, NTSC, MICROSECONDS, Rounding::Up),
            Some(-33366)
        );
    }

    #[test]
    fn nearest_rounds_halves_away_from_zero() {
        let half = Rational { num: 1, den: 2 };
        let one = Rational { num: 1, den: 1 };
        assert_eq!(rescale(1, half, one, Rounding::Nearest), Some(1));
        assert_eq!(rescale(-1, half, one, Rounding::Nearest), Some(-1));
        assert_eq!(
            rescale(3003, Rational { num: 1, den: 90000 }, MS, Rounding::Nearest),
            Some(33)
        );
    }

    #[test]
    fn a_frame_placed_up_is_found_again_rounding_down() {
        // The rule the player depends on: a frame's timeline position rounds
        // up, a timeline position's source tick rounds down, and the frame
        // survives the round trip — for every tick of a second of each base.
        for base in [NTSC, MP4, Rational { num: 1, den: 90000 }, MS] {
            for pts in 0..30_000 {
                let micros = rescale(pts, base, MICROSECONDS, Rounding::Up).unwrap_or(0);
                let back = rescale(micros, MICROSECONDS, base, Rounding::Down).unwrap_or(-1);
                assert_eq!(back, pts, "{pts} in {base:?}");
            }
        }
    }

    #[test]
    fn an_invalid_time_base_is_refused() {
        assert_eq!(
            rescale(1, Rational { num: 0, den: 1 }, MS, Rounding::Down),
            None
        );
        assert_eq!(
            rescale(1, MS, Rational { num: 1, den: 0 }, Rounding::Down),
            None
        );
    }

    #[test]
    fn seconds_convert_with_the_rounding_asked_for() {
        assert_eq!(from_seconds(0.7, MP4, Rounding::Nearest), 10752);
        assert_eq!(from_seconds(1.0 / 3.0, MS, Rounding::Down), 333);
        assert_eq!(from_seconds(1.0 / 3.0, MS, Rounding::Up), 334);
        assert!((seconds(10752, MP4) - 0.7).abs() < 1e-12);
    }
}
