//! Reading frame timestamps from the `showinfo` filter.
//!
//! Raw video on a pipe carries pixels and nothing else, and a variable frame
//! rate source makes "frame number times frame duration" wrong. So the decode
//! graph ends in `showinfo`, which logs every frame it passes — in the same
//! order the frames are written to standard output — with its presentation
//! timestamp in the filter link's time base. The nth line is the nth frame.

use crate::probe::Rational;

/// One line of `showinfo` output that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Line {
    /// The time base every following `pts` is in.
    TimeBase(Rational),
    /// A frame passed the filter. `pts` is `None` for a frame without one.
    Frame { n: u64, pts: Option<i64> },
}

/// Parse one stderr line, or `None` if it is not from `showinfo`.
pub(crate) fn parse(line: &str) -> Option<Line> {
    let body = line
        .split_once("Parsed_showinfo")
        .and_then(|(_, rest)| rest.split_once("] "))
        .map(|(_, body)| body.trim_start())?;
    if let Some(rest) = body.strip_prefix("config in time_base:") {
        let ratio = rest.trim_start().split([',', ' ']).next()?;
        let (num, den) = ratio.split_once('/')?;
        return Some(Line::TimeBase(Rational {
            num: num.parse().ok()?,
            den: den.parse().ok()?,
        }));
    }
    let rest = body.strip_prefix("n:")?;
    let mut tokens = rest.split_whitespace();
    let n = tokens.next()?.parse().ok()?;
    if tokens.next()? != "pts:" {
        return None;
    }
    let pts = tokens.next()?.parse().ok();
    Some(Line::Frame { n, pts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_line_yields_its_index_and_timestamp() {
        let line = "[Parsed_showinfo_3 @ 000001d2c4a0f880] n:  12 pts:  10752 pts_time:0.7     duration:    512 duration_time:0.0333333 fmt:rgba";
        assert_eq!(
            parse(line),
            Some(Line::Frame {
                n: 12,
                pts: Some(10752)
            })
        );
    }

    #[test]
    fn a_negative_or_missing_timestamp_is_read_as_such() {
        assert_eq!(
            parse("[Parsed_showinfo_0 @ 0] n:   0 pts:  -1024 pts_time:-0.066"),
            Some(Line::Frame {
                n: 0,
                pts: Some(-1024)
            })
        );
        assert_eq!(
            parse("[Parsed_showinfo_0 @ 0] n:   3 pts:  NOPTS pts_time:NOPTS"),
            Some(Line::Frame { n: 3, pts: None })
        );
    }

    #[test]
    fn the_link_time_base_is_read() {
        assert_eq!(
            parse("[Parsed_showinfo_2 @ 0] config in time_base: 1/15360, frame_rate: 30/1"),
            Some(Line::TimeBase(Rational { num: 1, den: 15360 }))
        );
    }

    #[test]
    fn other_lines_and_side_data_are_ignored() {
        assert_eq!(parse("Stream mapping:"), None);
        assert_eq!(
            parse(
                "[Parsed_showinfo_3 @ 0]   side data - display matrix: rotation of -90.00 degrees"
            ),
            None
        );
        assert_eq!(parse("[Parsed_showinfo_3 @ 0] color_range:tv"), None);
    }
}
