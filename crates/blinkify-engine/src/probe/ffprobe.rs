//! `ffprobe -print_format json`, read defensively.
//!
//! Every field is optional here because every field is optional in practice:
//! `ffprobe` omits what the demuxer does not know, writes `N/A` for what it
//! knows it does not know, and prints numbers as strings in some places and as
//! numbers in others. The file being described is hostile input, and so is this
//! description of it.

use serde::Deserialize;
use serde_json::Value;

use super::{
    AudioInfo, Chapter, ChromaSubsampling, Color, ContainerInfo, ContentLight, EditList, FrameRate,
    FrameRateMode, Hdr, HdrTransfer, MasteringDisplay, Rational, StreamInfo, StreamKind, VideoInfo,
};

#[derive(Debug, Default, Deserialize)]
pub(super) struct Output {
    #[serde(default)]
    pub streams: Vec<Stream>,
    #[serde(default)]
    pub chapters: Vec<RawChapter>,
    #[serde(default)]
    pub format: Option<Format>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Format {
    #[serde(rename = "format_name")]
    pub name: Option<String>,
    #[serde(rename = "format_long_name")]
    pub long_name: Option<String>,
    pub duration: Option<Value>,
    pub bit_rate: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct RawChapter {
    pub start_time: Option<Value>,
    pub end_time: Option<Value>,
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Stream {
    pub index: u32,
    pub codec_name: Option<String>,
    pub codec_type: Option<String>,
    pub profile: Option<String>,
    pub level: Option<i32>,
    pub bit_rate: Option<Value>,
    pub duration: Option<Value>,
    pub start_time: Option<Value>,
    pub time_base: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pix_fmt: Option<String>,
    pub bits_per_raw_sample: Option<Value>,
    pub sample_aspect_ratio: Option<String>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub field_order: Option<String>,
    pub has_b_frames: Option<u32>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub sample_rate: Option<Value>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub sample_fmt: Option<String>,
    pub bits_per_sample: Option<u32>,
    #[serde(default)]
    pub disposition: std::collections::BTreeMap<String, i64>,
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub side_data_list: Vec<Value>,
}

/// `-show_entries frame=:side_data` asks for side data on packets and frames
/// alike, and `ffprobe` then prints one interleaved `packets_and_frames` list
/// rather than `frames`. Both shapes are read; only frames count.
#[derive(Debug, Default, Deserialize)]
pub(super) struct Frames {
    #[serde(default)]
    pub frames: Vec<Frame>,
    #[serde(default)]
    pub packets_and_frames: Vec<Frame>,
}

impl Frames {
    fn side_data(&self) -> impl Iterator<Item = &Value> {
        self.frames
            .iter()
            .chain(
                self.packets_and_frames
                    .iter()
                    .filter(|entry| entry.kind.as_deref() == Some("frame")),
            )
            .flat_map(|frame| frame.side_data_list.iter())
    }
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Frame {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub side_data_list: Vec<Value>,
}

/// A number that may be a JSON number, a numeric string, or `"N/A"`.
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// `30000/1001` or `16:9`. `0/0` and `N/A` are "unknown".
pub(super) fn rational(text: Option<&str>) -> Option<Rational> {
    let text = text?;
    let (num, den) = text.split_once(['/', ':'])?;
    let rational = Rational {
        num: num.trim().parse().ok()?,
        den: den.trim().parse().ok()?,
    };
    (rational.den != 0 && rational.num != 0).then_some(rational)
}

fn non_empty(value: Option<&String>) -> Option<String> {
    value
        .filter(|s| !s.is_empty() && s.as_str() != "unknown" && s.as_str() != "N/A")
        .cloned()
}

pub(super) fn container_info(
    output: &Output,
    size_bytes: u64,
    edit_lists: Vec<EditList>,
) -> ContainerInfo {
    let format = output.format.as_ref();
    ContainerInfo {
        format_name: format.and_then(|f| f.name.clone()).unwrap_or_default(),
        format_long_name: format.and_then(|f| f.long_name.clone()),
        duration_seconds: number(format.and_then(|f| f.duration.as_ref())),
        bit_rate: number(format.and_then(|f| f.bit_rate.as_ref())),
        size_bytes,
        has_edit_list: edit_lists.iter().any(EditList::shifts_presentation),
        edit_lists,
        chapters: output
            .chapters
            .iter()
            .map(|chapter| Chapter {
                start_seconds: number(chapter.start_time.as_ref()).unwrap_or(0.0),
                end_seconds: number(chapter.end_time.as_ref()).unwrap_or(0.0),
                title: chapter.tags.get("title").cloned(),
            })
            .collect(),
    }
}

pub(super) fn stream_info(stream: &Stream) -> StreamInfo {
    let kind = match stream.codec_type.as_deref() {
        Some("video") => StreamKind::Video(Box::new(video_info(stream))),
        Some("audio") => StreamKind::Audio(AudioInfo {
            sample_rate: number(stream.sample_rate.as_ref()).and_then(to_u32),
            channels: stream.channels,
            channel_layout: non_empty(stream.channel_layout.as_ref()),
            sample_format: non_empty(stream.sample_fmt.as_ref()),
            bits_per_sample: stream.bits_per_sample.filter(|&bits| bits > 0),
        }),
        Some("subtitle") => StreamKind::Subtitle,
        Some("attachment") => StreamKind::Attachment {
            filename: stream.tags.get("filename").cloned(),
            mimetype: stream.tags.get("mimetype").cloned(),
        },
        Some("data") => StreamKind::Data,
        _ => StreamKind::Other,
    };
    StreamInfo {
        index: stream.index,
        codec: non_empty(stream.codec_name.as_ref()),
        profile: non_empty(stream.profile.as_ref()),
        // FFmpeg reports "no level" as -99.
        level: stream.level.filter(|&level| level > 0),
        bit_rate: number(stream.bit_rate.as_ref()),
        duration_seconds: number(stream.duration.as_ref()),
        start_seconds: number(stream.start_time.as_ref()),
        time_base: rational(stream.time_base.as_deref()),
        is_default: stream.disposition.get("default").is_some_and(|&d| d != 0),
        kind,
    }
}

fn to_u32(value: f64) -> Option<u32> {
    (value >= 0.0 && value <= f64::from(u32::MAX) && value.fract() == 0.0).then(|| {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = value as u32;
        whole
    })
}

fn video_info(stream: &Stream) -> VideoInfo {
    let width = stream.width.unwrap_or(0);
    let height = stream.height.unwrap_or(0);
    let rotation = rotation(&stream.side_data_list);
    let (display_width, display_height) = if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    };
    let pixel_format = non_empty(stream.pix_fmt.as_ref());
    VideoInfo {
        width,
        height,
        display_width,
        display_height,
        rotation,
        bit_depth: pixel_format.as_deref().and_then(bit_depth).or_else(|| {
            number(stream.bits_per_raw_sample.as_ref())
                .and_then(to_u32)
                .and_then(|b| u8::try_from(b).ok())
        }),
        chroma_subsampling: pixel_format.as_deref().and_then(chroma),
        pixel_format,
        sample_aspect_ratio: rational(stream.sample_aspect_ratio.as_deref()),
        frame_rate: FrameRate {
            average: rational(stream.avg_frame_rate.as_deref()),
            real_base: rational(stream.r_frame_rate.as_deref()),
            mode: FrameRateMode::Unknown,
        },
        field_order: non_empty(stream.field_order.as_ref()),
        has_b_frames: stream.has_b_frames.is_some_and(|b| b > 0),
        color: Color {
            range: non_empty(stream.color_range.as_ref()),
            primaries: non_empty(stream.color_primaries.as_ref()),
            transfer: non_empty(stream.color_transfer.as_ref()),
            matrix: non_empty(stream.color_space.as_ref()),
        },
        hdr: None,
        is_attached_picture: stream
            .disposition
            .get("attached_pic")
            .is_some_and(|&d| d != 0),
    }
}

/// The display matrix rotation, normalised to 0, 90, 180 or 270.
fn rotation(side_data: &[Value]) -> u32 {
    side_data
        .iter()
        .filter(|entry| {
            entry.get("side_data_type").and_then(Value::as_str) == Some("Display Matrix")
        })
        .find_map(|entry| number(entry.get("rotation")))
        .map_or(0, |degrees| {
            // Snap to the nearest quarter turn: a display matrix can carry a
            // non-right angle, which no player honours.
            let quarter_turns = (degrees / 90.0).round();
            #[allow(clippy::cast_possible_truncation)]
            let quarter_turns = quarter_turns as i64;
            u32::try_from(quarter_turns.rem_euclid(4) * 90).unwrap_or(0)
        })
}

/// Bits per component from the pixel format name.
fn bit_depth(pix_fmt: &str) -> Option<u8> {
    const DEPTHS: [(&str, u8); 5] = [("16", 16), ("14", 14), ("12", 12), ("10", 10), ("9", 9)];
    if pix_fmt.starts_with("p010") {
        return Some(10);
    }
    if pix_fmt.starts_with("p016") {
        return Some(16);
    }
    DEPTHS
        .iter()
        .find(|(digits, _)| {
            pix_fmt.contains(&format!("p{digits}")) || pix_fmt.contains(&format!("{digits}le"))
        })
        .map(|&(_, depth)| depth)
        .or(Some(8))
}

fn chroma(pix_fmt: &str) -> Option<ChromaSubsampling> {
    let format = pix_fmt.trim_start_matches("yuvj");
    Some(
        if pix_fmt.starts_with("nv12")
            || pix_fmt.starts_with("p01")
            || pix_fmt.contains("420")
            || format.starts_with("420")
        {
            ChromaSubsampling::Yuv420
        } else if pix_fmt.contains("422")
            || pix_fmt.starts_with("nv16")
            || pix_fmt.starts_with("p21")
        {
            ChromaSubsampling::Yuv422
        } else if pix_fmt.contains("444")
            || pix_fmt.starts_with("nv24")
            || pix_fmt.starts_with("p41")
        {
            ChromaSubsampling::Yuv444
        } else if pix_fmt.starts_with("gray") {
            ChromaSubsampling::Gray
        } else if pix_fmt.contains("rgb") || pix_fmt.contains("bgr") || pix_fmt.starts_with("gbr") {
            ChromaSubsampling::Rgb
        } else {
            return None;
        },
    )
}

/// Constant or variable, from the presentation timestamps of the first
/// thirty seconds of packets.
///
/// Timestamps are sorted first, because packets arrive in decode order and
/// B-frames make that differ from presentation order. The rate is constant
/// when every gap is the same to within one tick — a millisecond timebase
/// writes 29.97 fps as alternating 33 and 34 ms, which is constant, not
/// variable.
pub(super) fn frame_rate_mode(csv: &[u8]) -> FrameRateMode {
    let mut pts: Vec<i64> = String::from_utf8_lossy(csv)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse::<i64>().ok())
        .collect();
    pts.sort_unstable();
    pts.dedup();
    let gaps: Vec<i64> = pts
        .windows(2)
        .filter_map(|pair| match pair {
            [a, b] => b.checked_sub(*a),
            _ => None,
        })
        .collect();
    if gaps.len() < 2 {
        return FrameRateMode::Unknown;
    }
    let (Some(&min), Some(&max)) = (gaps.iter().min(), gaps.iter().max()) else {
        return FrameRateMode::Unknown;
    };
    if max.saturating_sub(min) <= 1 {
        FrameRateMode::Constant
    } else {
        FrameRateMode::Variable
    }
}

/// HDR, from whichever place the file keeps it: stream side data (MP4 `mdcv`
/// and `clli` boxes, Matroska colour elements) or the first frame (HEVC SEI).
pub(super) fn hdr(stream: &Stream, first_frame: &Frames, color: &Color) -> Option<Hdr> {
    let side_data = stream.side_data_list.iter().chain(first_frame.side_data());
    let mut mastering_display = None;
    let mut content_light = None;
    for entry in side_data {
        match entry.get("side_data_type").and_then(Value::as_str) {
            Some("Mastering display metadata") if mastering_display.is_none() => {
                mastering_display = mastering(entry);
            }
            Some("Content light level metadata") if content_light.is_none() => {
                content_light = Some(ContentLight {
                    max_content: number(entry.get("max_content"))
                        .and_then(to_u32)
                        .unwrap_or(0),
                    max_frame_average: number(entry.get("max_average"))
                        .and_then(to_u32)
                        .unwrap_or(0),
                });
            }
            _ => {}
        }
    }
    let transfer = match color.transfer.as_deref() {
        Some("smpte2084") => Some(HdrTransfer::Pq),
        Some("arib-std-b67") => Some(HdrTransfer::Hlg),
        _ if mastering_display.is_some() || content_light.is_some() => Some(HdrTransfer::Other),
        _ => None,
    }?;
    Some(Hdr {
        transfer,
        mastering_display,
        content_light,
    })
}

fn mastering(entry: &Value) -> Option<MasteringDisplay> {
    let ratio = |key: &str| rational(entry.get(key)?.as_str()).and_then(Rational::value);
    let pair = |x: &str, y: &str| Some([ratio(x)?, ratio(y)?]);
    Some(MasteringDisplay {
        red: pair("red_x", "red_y")?,
        green: pair("green_x", "green_y")?,
        blue: pair("blue_x", "blue_y")?,
        white_point: pair("white_point_x", "white_point_y")?,
        min_luminance: ratio("min_luminance").unwrap_or(0.0),
        max_luminance: ratio("max_luminance")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_mode_tolerates_one_tick_of_rounding() {
        // 29.97 fps in a millisecond timebase.
        assert_eq!(
            frame_rate_mode(b"0\n33\n67\n100\n133\n167\n200\n"),
            FrameRateMode::Constant
        );
    }

    #[test]
    fn frame_rate_mode_sees_through_b_frame_reordering() {
        // Decode order of an IBBP stream at 25 fps, 1/1000 timebase.
        assert_eq!(
            frame_rate_mode(b"0\n120\n40\n80\n240\n160\n200\n"),
            FrameRateMode::Constant
        );
    }

    #[test]
    fn frame_rate_mode_catches_a_rate_change_and_a_dropped_frame() {
        assert_eq!(
            frame_rate_mode(b"0\n3000\n6000\n9000\n18000\n27000\n"),
            FrameRateMode::Variable
        );
        assert_eq!(
            frame_rate_mode(b"0\n3000\n6000\n12000\n15000\n"),
            FrameRateMode::Variable
        );
    }

    #[test]
    fn frame_rate_mode_needs_enough_frames() {
        assert_eq!(frame_rate_mode(b"0\n"), FrameRateMode::Unknown);
        assert_eq!(frame_rate_mode(b"N/A\nN/A\n"), FrameRateMode::Unknown);
    }

    #[test]
    fn rotation_is_normalised_to_a_quarter_turn() {
        let matrix = |degrees: i64| {
            vec![serde_json::json!({"side_data_type": "Display Matrix", "rotation": degrees})]
        };
        assert_eq!(rotation(&matrix(-90)), 270);
        assert_eq!(rotation(&matrix(90)), 90);
        assert_eq!(rotation(&matrix(180)), 180);
        assert_eq!(rotation(&matrix(-180)), 180);
        assert_eq!(rotation(&[]), 0);
    }

    #[test]
    fn bit_depth_and_chroma_come_from_the_pixel_format() {
        assert_eq!(bit_depth("yuv420p"), Some(8));
        assert_eq!(bit_depth("yuv420p10le"), Some(10));
        assert_eq!(bit_depth("p010le"), Some(10));
        assert_eq!(bit_depth("yuv422p12le"), Some(12));
        assert_eq!(chroma("yuv420p10le"), Some(ChromaSubsampling::Yuv420));
        assert_eq!(chroma("yuvj422p"), Some(ChromaSubsampling::Yuv422));
        assert_eq!(chroma("nv12"), Some(ChromaSubsampling::Yuv420));
        assert_eq!(chroma("gbrp10le"), Some(ChromaSubsampling::Rgb));
    }

    #[test]
    fn unknown_rationals_are_none() {
        assert_eq!(rational(Some("0/0")), None);
        assert_eq!(rational(Some("N/A")), None);
        assert_eq!(rational(Some("16:9")), Some(Rational { num: 16, den: 9 }));
    }
}
