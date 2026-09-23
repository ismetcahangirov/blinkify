//! Timecode: a timeline position as hours, minutes, seconds and frames, in
//! the timeline's frame rate.
//!
//! Non-drop-frame, counted at the nominal rate — 30 for 30000/1001 — which is
//! what every editor shows by default. The frame number itself is exact: it is
//! the timeline position times the real rate, rounded down, and a frame's
//! position is its start rounded *up* to the microsecond — so a frame's start
//! is never counted as the frame before it, and the timecode shown is the
//! timecode of the frame shown.

use crate::probe::Rational;

use super::plan::ProgramTime;

/// The frame of the timeline, at `frame_rate`, that `position` falls in.
#[must_use]
pub fn frame_number(position: ProgramTime, frame_rate: Rational) -> i64 {
    let numerator = i128::from(position) * i128::from(frame_rate.num);
    let denominator = i128::from(frame_rate.den) * 1_000_000;
    if denominator <= 0 {
        return 0;
    }
    i64::try_from(numerator.div_euclid(denominator)).unwrap_or(i64::MAX)
}

/// The nominal whole-number rate timecode counts frames in.
#[must_use]
pub fn nominal_rate(frame_rate: Rational) -> i64 {
    let rate = frame_rate.value().unwrap_or(30.0).round();
    // A frame rate is a small positive number.
    #[allow(clippy::cast_possible_truncation)]
    let rate = rate as i64;
    rate.max(1)
}

/// `HH:MM:SS:FF` for frame `frame` at `frame_rate`.
#[must_use]
pub fn format(frame: i64, frame_rate: Rational) -> String {
    let rate = nominal_rate(frame_rate);
    let frame = frame.max(0);
    let frames = frame.rem_euclid(rate);
    let seconds = frame.div_euclid(rate);
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds.div_euclid(3600),
        seconds.div_euclid(60).rem_euclid(60),
        seconds.rem_euclid(60),
        frames
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIRTY: Rational = Rational { num: 30, den: 1 };
    const NTSC: Rational = Rational {
        num: 30000,
        den: 1001,
    };

    #[test]
    fn a_frame_starts_count_as_that_frame() {
        assert_eq!(frame_number(0, THIRTY), 0);
        assert_eq!(
            frame_number(33_333, THIRTY),
            0,
            "a microsecond before frame 1"
        );
        assert_eq!(frame_number(50_000, THIRTY), 1, "the middle of frame 1");
        assert_eq!(frame_number(33_334, THIRTY), 1);
        assert_eq!(frame_number(3_966_667, THIRTY), 119);
        // 1001/30000 s per frame: frame 1 starts at 33366.67 µs.
        assert_eq!(frame_number(33_367, NTSC), 1);
        assert_eq!(frame_number(1_001_000, NTSC), 30);
    }

    #[test]
    fn timecode_is_hours_minutes_seconds_frames() {
        assert_eq!(format(0, THIRTY), "00:00:00:00");
        assert_eq!(format(119, THIRTY), "00:00:03:29");
        assert_eq!(format(120, THIRTY), "00:00:04:00");
        assert_eq!(format(30 * 3661 + 7, THIRTY), "01:01:01:07");
        assert_eq!(format(30, NTSC), "00:00:01:00");
        assert_eq!(format(-5, THIRTY), "00:00:00:00");
    }
}
