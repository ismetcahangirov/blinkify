//! Re-encode profile matching (#44): a re-encoded stretch that joins copied
//! packets in one stream must be the same kind of stream — same codec,
//! profile, level ceiling, pixel format, bit depth, chroma and colour — or
//! the file plays for a second and then breaks up, and passes a naive test.
//!
//! This module decides, before anything is encoded:
//!
//! - **What the source is** ([`SourceProfile`]): read from its probe.
//! - **Which encoder can make it** ([`select`]): from the machine's
//!   capability profile (#21, ADR-0003), the first encoder — hardware first —
//!   that produced this profile at this bit depth and at least this level. An
//!   encoder that cannot is passed over; where none can, the answer is a
//!   refusal naming the parameter that could not be matched ([`Unmatched`]),
//!   never a substitute. There is no software H.264 or HEVC encoder to fall
//!   back to (ADR-0003 part 3).
//! - **How to ask it** ([`EncoderChoice::arguments`]): profile, level, pixel
//!   format, colour metadata stated explicitly, and a quality far above the
//!   source's — a seam is a fraction of a second, so bits spent there are
//!   free.
//! - **Whether what it made can join** ([`validate_join`]): the encoder's
//!   codec configuration is compared with the source's field by field before
//!   a single packet of it is written.
//!
//! HDR is refused here as everywhere a picture would be rendered (ADR-0008),
//! and so is interlaced material: the encoders are probed progressive only.

use serde::Serialize;
use thiserror::Error;
use ts_rs::TS;

use super::sps::{Sequence, av1_sequence, h264_sequence, hevc_sequence};
use crate::capability::{EncoderCapabilities, EncoderSource, ProfileCapability, VideoCodec};
use crate::probe::{ChromaSubsampling, Rational, StreamInfo, StreamKind};

/// What a source's video stream is, as far as an encoder must match it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SourceProfile {
    pub codec: VideoCodec,
    /// In the capability profile's terms: `high`, `main10`, `0`.
    pub profile: String,
    /// As the codec numbers it: H.264 `level_idc` (41), HEVC
    /// `general_level_idc` (153). `None` where levels are not matched.
    pub level: Option<i32>,
    pub pixel_format: String,
    pub bit_depth: u8,
    pub width: u32,
    pub height: u32,
    /// `[primaries, transfer, matrix, range]`, where the stream states them.
    pub colour: [Option<String>; 4],
    pub time_base: Rational,
}

/// Which parameter no encoder on this machine could match.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, TS)]
#[serde(
    tag = "unmatched",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Unmatched {
    #[error("{codec} is not a codec Blinkify can encode")]
    Codec { codec: String },
    #[error("this computer has no encoder for {codec:?}")]
    NoEncoder { codec: VideoCodec },
    #[error("no encoder on this computer makes the {profile} profile of {codec:?}")]
    Profile { codec: VideoCodec, profile: String },
    #[error("no encoder on this computer makes {bit_depth}-bit {codec:?}")]
    BitDepth { codec: VideoCodec, bit_depth: u8 },
    #[error("no encoder on this computer reaches level {level} of {codec:?}")]
    Level { codec: VideoCodec, level: i32 },
    #[error("only 4:2:0 pictures can be re-encoded to match")]
    Chroma,
    #[error("HDR pictures are not re-encoded: they are copied as recorded or declined")]
    Hdr,
    #[error("interlaced pictures are not re-encoded to match")]
    Interlaced,
    #[error("this computer's encoders are still being checked")]
    EncodersUnknown,
}

/// The encoder chosen for a source, and how it was asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EncoderChoice {
    pub codec: VideoCodec,
    pub encoder: String,
    pub source: EncoderSource,
    pub profile: String,
    pub pixel_format: String,
    pub bit_depth: u8,
    /// The level the encoder is asked for: the source's.
    pub level: Option<i32>,
    pub colour: [Option<String>; 4],
}

fn codec_of(name: &str) -> Option<VideoCodec> {
    Some(match name {
        "h264" => VideoCodec::H264,
        "hevc" => VideoCodec::Hevc,
        "vp9" => VideoCodec::Vp9,
        "av1" => VideoCodec::Av1,
        _ => return None,
    })
}

/// The probe's profile name in the capability profile's terms.
fn profile_name(codec: VideoCodec, probed: &str) -> String {
    let probed = probed.to_ascii_lowercase();
    match (codec, probed.as_str()) {
        (VideoCodec::H264, "high 10") => "high10".to_owned(),
        (VideoCodec::Hevc, "main 10") => "main10".to_owned(),
        (VideoCodec::Vp9, profile) => profile.trim_start_matches("profile ").to_owned(),
        (_, profile) => profile.replace(' ', "-"),
    }
}

/// A colour field as the probe reports it, where it says something.
fn stated(value: Option<&String>) -> Option<String> {
    value
        .filter(|v| !v.is_empty() && v.as_str() != "unknown" && v.as_str() != "unspecified")
        .cloned()
}

/// What `stream` is, or why no encoder could ever match it.
///
/// # Errors
///
/// The stream is not video, not a codec Blinkify encodes, not 4:2:0, HDR or
/// interlaced.
pub fn source_profile(stream: &StreamInfo) -> Result<SourceProfile, Unmatched> {
    let StreamKind::Video(video) = &stream.kind else {
        return Err(Unmatched::Codec {
            codec: stream.codec.clone().unwrap_or_default(),
        });
    };
    let name = stream.codec.clone().unwrap_or_default();
    let codec = codec_of(&name).ok_or(Unmatched::Codec { codec: name })?;
    if video.hdr.is_some() {
        return Err(Unmatched::Hdr);
    }
    if video
        .field_order
        .as_deref()
        .is_some_and(|order| !matches!(order, "progressive" | "unknown"))
    {
        return Err(Unmatched::Interlaced);
    }
    if video
        .chroma_subsampling
        .is_some_and(|chroma| chroma != ChromaSubsampling::Yuv420)
    {
        return Err(Unmatched::Chroma);
    }
    let pixel_format = video.pixel_format.clone().unwrap_or_default();
    let bit_depth = video
        .bit_depth
        .unwrap_or(if pixel_format.contains("10") { 10 } else { 8 });
    Ok(SourceProfile {
        codec,
        profile: profile_name(codec, stream.profile.as_deref().unwrap_or("")),
        level: match codec {
            VideoCodec::H264 | VideoCodec::Hevc => stream.level,
            VideoCodec::Vp9 | VideoCodec::Av1 => None,
        },
        pixel_format,
        bit_depth,
        width: video.width,
        height: video.height,
        colour: [
            stated(video.color.primaries.as_ref()),
            stated(video.color.transfer.as_ref()),
            stated(video.color.matrix.as_ref()),
            stated(video.color.range.as_ref()),
        ],
        time_base: stream.time_base.unwrap_or(Rational { num: 1, den: 1 }),
    })
}

/// A capability's level label (`5.1`) as the codec numbers it.
fn level_number(codec: VideoCodec, label: &str) -> Option<i32> {
    let (major, minor) = label.split_once('.')?;
    let tenths = major.parse::<i32>().ok()? * 10 + minor.parse::<i32>().ok()?;
    match codec {
        VideoCodec::H264 => Some(tenths),
        // general_level_idc is the level times thirty: 5.1 is 153.
        VideoCodec::Hevc => Some(tenths * 3),
        VideoCodec::Vp9 | VideoCodec::Av1 => None,
    }
}

/// Whether `capability` makes `source`'s profile at its bit depth and level.
fn fits(source: &SourceProfile, capability: &ProfileCapability) -> Result<(), Unmatched> {
    if capability.profile != source.profile {
        return Err(Unmatched::Profile {
            codec: source.codec,
            profile: source.profile.clone(),
        });
    }
    if capability.bit_depth != source.bit_depth {
        return Err(Unmatched::BitDepth {
            codec: source.codec,
            bit_depth: source.bit_depth,
        });
    }
    if let Some(level) = source.level {
        let ceiling = capability
            .max_level
            .as_deref()
            .and_then(|label| level_number(source.codec, label));
        if ceiling.is_none_or(|ceiling| ceiling < level) {
            return Err(Unmatched::Level {
                codec: source.codec,
                level,
            });
        }
    }
    Ok(())
}

/// The first encoder on this machine — in `ADR-0003`'s order, hardware first
/// — that makes `source` exactly.
///
/// # Errors
///
/// The most specific thing no encoder could match: no encoder for the codec
/// at all, else the profile, else the bit depth, else the level.
pub fn select(
    source: &SourceProfile,
    capabilities: &EncoderCapabilities,
) -> Result<EncoderChoice, Unmatched> {
    let encoders = capabilities.encoders_for(source.codec);
    if encoders.is_empty() {
        return Err(Unmatched::NoEncoder {
            codec: source.codec,
        });
    }
    let mut closest: Option<Unmatched> = None;
    for encoder in encoders {
        for capability in &encoder.profiles {
            match fits(source, capability) {
                Ok(()) => {
                    return Ok(EncoderChoice {
                        codec: source.codec,
                        encoder: encoder.encoder.clone(),
                        source: encoder.source,
                        profile: source.profile.clone(),
                        pixel_format: capability.pixel_format.clone(),
                        bit_depth: source.bit_depth,
                        level: source.level,
                        colour: source.colour.clone(),
                    });
                }
                Err(unmatched) => {
                    // Keep the refusal that got furthest: a level is closer
                    // than a bit depth, which is closer than a profile.
                    let rank = |u: &Unmatched| match u {
                        Unmatched::Level { .. } => 3,
                        Unmatched::BitDepth { .. } => 2,
                        _ => 1,
                    };
                    if closest.as_ref().is_none_or(|c| rank(&unmatched) > rank(c)) {
                        closest = Some(unmatched);
                    }
                }
            }
        }
    }
    Err(closest.unwrap_or(Unmatched::NoEncoder {
        codec: source.codec,
    }))
}

impl EncoderChoice {
    /// The level as this encoder's `-level` option takes it. NVENC and AMF
    /// take the codec's own number; Quick Sync's HEVC takes the level times
    /// ten, like H.264 (see the capability probe).
    fn level_argument(&self, codec: VideoCodec) -> Option<String> {
        let level = self.level?;
        match (codec, self.source) {
            (VideoCodec::Hevc, EncoderSource::Intel) => {
                Some((level * 10).div_euclid(30).to_string())
            }
            _ => Some(level.to_string()),
        }
    }

    /// The options that make this encoder produce the source's kind of
    /// stream, at a quality well above the source's, as `(name, value)`.
    #[must_use]
    pub fn arguments(&self, codec: VideoCodec) -> Vec<(&'static str, String)> {
        let mut arguments: Vec<(&'static str, String)> = vec![
            ("-c:v", self.encoder.clone()),
            ("-pix_fmt", self.pixel_format.clone()),
        ];
        let profile = match (self.encoder.as_str(), self.profile.as_str()) {
            // SVT-AV1 and libaom name profiles by number; AV1 Main is 0.
            ("libsvtav1" | "libaom-av1" | "av1_nvenc" | "av1_qsv" | "av1_amf", _) => None,
            (_, profile) => Some(profile.to_owned()),
        };
        if let Some(profile) = profile {
            arguments.push(("-profile:v", profile));
        }
        if let Some(level) = self.level_argument(codec) {
            arguments.push(("-level:v", level));
        }
        // A seam is a fraction of a second, so its quality is set far above
        // the source's — but never past the bitrate its level allows, or the
        // encoder refuses the level and the seam would claim a higher one.
        let ceiling = self
            .level
            .and_then(|level| max_kilobits(codec, &self.profile, level));
        let quality: Vec<(&'static str, String)> = match self.source {
            EncoderSource::Nvidia => vec![
                ("-preset", "p7".to_owned()),
                ("-rc", "vbr".to_owned()),
                ("-cq", "16".to_owned()),
                ("-b:v", "0".to_owned()),
            ],
            EncoderSource::Intel => vec![
                ("-preset", "veryslow".to_owned()),
                ("-global_quality", "16".to_owned()),
            ],
            EncoderSource::Amd => vec![
                ("-quality", "quality".to_owned()),
                ("-rc", "qvbr".to_owned()),
                ("-qvbr_quality_level", "44".to_owned()),
            ],
            EncoderSource::Software => match self.encoder.as_str() {
                "libvpx-vp9" => vec![
                    ("-crf", "12".to_owned()),
                    ("-b:v", "0".to_owned()),
                    ("-row-mt", "1".to_owned()),
                ],
                "libsvtav1" => vec![("-crf", "14".to_owned()), ("-preset", "8".to_owned())],
                _ => vec![
                    ("-crf", "14".to_owned()),
                    ("-b:v", "0".to_owned()),
                    ("-cpu-used", "6".to_owned()),
                ],
            },
        };
        arguments.extend(quality);
        if let Some(kilobits) = ceiling {
            arguments.push(("-maxrate", format!("{kilobits}k")));
            arguments.push(("-bufsize", format!("{kilobits}k")));
        }
        let names = [
            "-color_primaries",
            "-color_trc",
            "-colorspace",
            "-color_range",
        ];
        for (name, value) in names.into_iter().zip(&self.colour) {
            if let Some(value) = value {
                arguments.push((name, value.clone()));
            }
        }
        arguments
    }
}

/// The most a stream at `level` may carry, in kilobits a second, with a
/// tenth held back: H.264 Table A-1 (High profiles × 1.25, High 10 × 3),
/// HEVC Table A.8 main tier. `None` where the level is not in the tables.
fn max_kilobits(codec: VideoCodec, profile: &str, level: i32) -> Option<u32> {
    let base: u32 = match codec {
        VideoCodec::H264 => match level {
            10 => 64,
            11 => 192,
            12 => 384,
            13 | 20 => 2_000,
            21 | 22 => 4_000,
            30 => 10_000,
            31 => 14_000,
            32 | 40 => 20_000,
            41 | 42 => 50_000,
            50 => 135_000,
            51 | 52 | 60..=62 => 240_000,
            _ => return None,
        },
        VideoCodec::Hevc => match level {
            30 => 128,
            60 => 1_500,
            63 => 3_000,
            90 => 6_000,
            93 => 10_000,
            120 => 12_000,
            123 => 20_000,
            150 => 25_000,
            153 => 40_000,
            156 | 180 => 60_000,
            183 => 120_000,
            186 => 240_000,
            _ => return None,
        },
        VideoCodec::Vp9 | VideoCodec::Av1 => return None,
    };
    let factor = match (codec, profile) {
        (VideoCodec::H264, "high") => 5,
        (VideoCodec::H264, "high10") => 12,
        _ => 4,
    };
    // base × factor ÷ 4, less a tenth: 90% of the level's ceiling.
    Some(base.saturating_mul(factor).saturating_mul(9).div_euclid(40))
}

/// A field of a codec configuration that differs between a re-encoded
/// stretch and the stream it joins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum JoinMismatch {
    #[error("the codec configuration could not be read")]
    Unreadable,
    #[error("the profile differs")]
    Profile,
    #[error("the level is above the stream's")]
    Level,
    #[error("the chroma format differs")]
    Chroma,
    #[error("the bit depth differs")]
    BitDepth,
}

fn sequence(codec: VideoCodec, record: &[u8]) -> Option<Sequence> {
    match codec {
        VideoCodec::H264 => h264_sequence(record),
        VideoCodec::Hevc => hevc_sequence(record),
        VideoCodec::Av1 => av1_sequence(record),
        // VP9 carries its profile, bit depth and chroma in every keyframe's
        // header, and has no configuration record to compare.
        VideoCodec::Vp9 => None,
    }
}

/// Whether a stretch encoded with `encoded`'s codec configuration can join a
/// stream whose configuration is `source`: same profile, chroma and bit
/// depth, and a level no higher. Checked before a packet of it is written.
///
/// # Errors
///
/// The first field that differs, or [`JoinMismatch::Unreadable`] where a
/// record that should be there cannot be read.
pub fn validate_join(codec: VideoCodec, source: &[u8], encoded: &[u8]) -> Result<(), JoinMismatch> {
    if codec == VideoCodec::Vp9 {
        return Ok(());
    }
    let (Some(source), Some(encoded)) = (sequence(codec, source), sequence(codec, encoded)) else {
        return Err(JoinMismatch::Unreadable);
    };
    let differs = |a: Option<u8>, b: Option<u8>| a.zip(b).is_some_and(|(a, b)| a != b);
    if source.profile != encoded.profile {
        return Err(JoinMismatch::Profile);
    }
    if encoded.level > source.level {
        return Err(JoinMismatch::Level);
    }
    if differs(source.chroma, encoded.chroma) {
        return Err(JoinMismatch::Chroma);
    }
    if differs(source.bit_depth, encoded.bit_depth) {
        return Err(JoinMismatch::BitDepth);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::capability::{Chroma, CodecCapability, EncoderCapability};

    fn source(
        codec: VideoCodec,
        profile: &str,
        bit_depth: u8,
        level: Option<i32>,
    ) -> SourceProfile {
        SourceProfile {
            codec,
            profile: profile.to_owned(),
            level,
            pixel_format: if bit_depth == 10 {
                "yuv420p10le".to_owned()
            } else {
                "yuv420p".to_owned()
            },
            bit_depth,
            width: 1920,
            height: 1080,
            colour: [Some("bt709".to_owned()), None, None, Some("tv".to_owned())],
            time_base: Rational {
                num: 1,
                den: 90_000,
            },
        }
    }

    fn capability(
        profile: &str,
        pixel_format: &str,
        bit_depth: u8,
        level: &str,
    ) -> ProfileCapability {
        ProfileCapability {
            profile: profile.to_owned(),
            pixel_format: pixel_format.to_owned(),
            bit_depth,
            chroma: Chroma::Yuv420,
            max_level: Some(level.to_owned()),
        }
    }

    fn machine(encoders: &[(VideoCodec, EncoderCapability)]) -> EncoderCapabilities {
        let codecs = [
            VideoCodec::H264,
            VideoCodec::Hevc,
            VideoCodec::Vp9,
            VideoCodec::Av1,
        ]
        .into_iter()
        .map(|codec| CodecCapability {
            codec,
            encoders: encoders
                .iter()
                .filter(|(c, _)| *c == codec)
                .map(|(_, e)| e.clone())
                .collect(),
        })
        .collect();
        EncoderCapabilities { codecs }
    }

    fn nvidia() -> EncoderCapabilities {
        machine(&[
            (
                VideoCodec::H264,
                EncoderCapability {
                    encoder: "h264_nvenc".to_owned(),
                    source: EncoderSource::Nvidia,
                    profiles: vec![
                        capability("main", "yuv420p", 8, "6.2"),
                        capability("high", "yuv420p", 8, "6.2"),
                    ],
                },
            ),
            (
                VideoCodec::Hevc,
                EncoderCapability {
                    encoder: "hevc_nvenc".to_owned(),
                    source: EncoderSource::Nvidia,
                    profiles: vec![capability("main", "yuv420p", 8, "5.1")],
                },
            ),
        ])
    }

    #[test]
    fn a_high_profile_source_gets_the_hardware_encoder_that_makes_high() {
        let choice =
            select(&source(VideoCodec::H264, "high", 8, Some(41)), &nvidia()).expect("an encoder");
        assert_eq!(choice.encoder, "h264_nvenc");
        let arguments = choice.arguments(VideoCodec::H264);
        assert!(arguments.contains(&("-profile:v", "high".to_owned())));
        assert!(arguments.contains(&("-level:v", "41".to_owned())));
        assert!(arguments.contains(&("-color_primaries", "bt709".to_owned())));
        assert!(arguments.contains(&("-color_range", "tv".to_owned())));
    }

    #[test]
    fn with_no_hardware_hevc_encoder_hevc_is_refused_not_substituted() {
        let software_only = machine(&[]);
        assert_eq!(
            select(
                &source(VideoCodec::Hevc, "main", 8, Some(120)),
                &software_only
            ),
            Err(Unmatched::NoEncoder {
                codec: VideoCodec::Hevc
            })
        );
    }

    #[test]
    fn a_ten_bit_source_is_never_given_an_eight_bit_encoder() {
        assert_eq!(
            select(
                &source(VideoCodec::Hevc, "main10", 10, Some(120)),
                &nvidia()
            ),
            Err(Unmatched::Profile {
                codec: VideoCodec::Hevc,
                profile: "main10".to_owned()
            })
        );
        // The same profile name at the wrong depth is a bit-depth refusal.
        let mut odd = nvidia();
        odd.codecs[1].encoders[0].profiles[0].profile = "main10".to_owned();
        assert_eq!(
            select(&source(VideoCodec::Hevc, "main10", 10, Some(120)), &odd),
            Err(Unmatched::BitDepth {
                codec: VideoCodec::Hevc,
                bit_depth: 10
            })
        );
    }

    #[test]
    fn a_level_above_every_encoders_ceiling_is_refused() {
        // HEVC 6.1 is 183; the encoder reaches 5.1 (153).
        assert_eq!(
            select(&source(VideoCodec::Hevc, "main", 8, Some(183)), &nvidia()),
            Err(Unmatched::Level {
                codec: VideoCodec::Hevc,
                level: 183
            })
        );
        assert!(select(&source(VideoCodec::Hevc, "main", 8, Some(153)), &nvidia()).is_ok());
    }

    #[test]
    fn a_baseline_source_has_no_match_among_main_and_high() {
        assert_eq!(
            select(
                &source(VideoCodec::H264, "constrained-baseline", 8, Some(30)),
                &nvidia()
            ),
            Err(Unmatched::Profile {
                codec: VideoCodec::H264,
                profile: "constrained-baseline".to_owned()
            })
        );
    }

    #[test]
    fn the_next_qualifying_encoder_is_used_when_the_first_cannot() {
        let mut two = nvidia();
        two.codecs[0].encoders.push(EncoderCapability {
            encoder: "h264_qsv".to_owned(),
            source: EncoderSource::Intel,
            profiles: vec![capability("high10", "yuv420p10le", 10, "5.1")],
        });
        let choice = select(&source(VideoCodec::H264, "high10", 10, Some(41)), &two).expect("qsv");
        assert_eq!(choice.encoder, "h264_qsv");
        assert_eq!(choice.pixel_format, "yuv420p10le");
    }

    /// The corpus's real High 3.0 SPS as an `avcC`, with its profile and
    /// level bytes replaced.
    fn record(profile: u8, level: u8) -> Vec<u8> {
        let mut record = vec![
            0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x1A, 0x67, 0x64, 0x00, 0x1E, 0xAC, 0xD9,
            0x40, 0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00, 0x03,
            0x00, 0x3C, 0x0F, 0x16, 0x2D, 0x96, 0x01, 0x00, 0x06, 0x68, 0xEB, 0xE3, 0xCB, 0x22,
            0xC0, 0xFD, 0xF8, 0xF8, 0x00,
        ];
        record[9] = profile;
        record[11] = level;
        record
    }

    #[test]
    fn a_join_is_refused_on_the_first_field_that_differs() {
        let source = record(100, 30);
        assert_eq!(
            validate_join(VideoCodec::H264, &source, &record(100, 30)),
            Ok(())
        );
        assert_eq!(
            validate_join(VideoCodec::H264, &source, &record(100, 21)),
            Ok(())
        );
        assert_eq!(
            validate_join(VideoCodec::H264, &source, &record(100, 31)),
            Err(JoinMismatch::Level)
        );
        // Main profile: a different profile, and no chroma or depth fields.
        assert_eq!(
            validate_join(VideoCodec::H264, &source, &record(77, 30)),
            Err(JoinMismatch::Profile)
        );
        assert_eq!(
            validate_join(VideoCodec::H264, &source, &[1, 2]),
            Err(JoinMismatch::Unreadable)
        );
        // The same parameter sets in Annex B form, as an encoder writes them.
        let mut annex_b = vec![0, 0, 0, 1];
        annex_b.extend_from_slice(&source[8..34]);
        assert_eq!(validate_join(VideoCodec::H264, &source, &annex_b), Ok(()));
    }
}
