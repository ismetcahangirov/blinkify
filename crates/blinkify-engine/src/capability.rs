//! What this machine can actually encode.
//!
//! `ADR-0003` part 1: advertisement is not capability. The bundled sidecar is
//! built with every hardware encoder FFmpeg knows (`h264_nvenc`, `hevc_qsv`,
//! `av1_amf` …), so `ffmpeg -encoders` lists them on every machine — including
//! the ones with no NVIDIA card, a Quick Sync driver too old for HEVC, or an
//! AMD part with no AV1 block. An encoder that is listed but fails to open is
//! worse than one that is not listed, because it fails at export time on the
//! user's real work.
//!
//! So every candidate is **opened and made to encode a few synthetic frames**
//! at each profile and bit depth Blinkify cares about, and only what succeeds is
//! reported. Epics #6 and #7 read this profile and nothing may assume an
//! encoder exists.
//!
//! The candidate table is deliberately explicit. A software H.264 or HEVC
//! encoder does not appear in it and cannot: ADR-0003 ships none.

use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::thread;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::sidecar::Sidecar;

/// A video codec Blinkify may have to encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum VideoCodec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

/// Where an encoder's implementation comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum EncoderSource {
    /// NVIDIA NVENC, through the GPU driver.
    Nvidia,
    /// Intel Quick Sync, through the GPU driver.
    Intel,
    /// AMD AMF, through the GPU driver.
    Amd,
    /// A licence-clean software encoder inside the bundled sidecar.
    Software,
}

/// Chroma subsampling of an encoder's input and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Chroma {
    #[serde(rename = "4:2:0")]
    #[ts(rename = "4:2:0")]
    Yuv420,
}

/// One profile at one bit depth that an encoder produced frames for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProfileCapability {
    /// The profile as the codec names it: `main`, `high`, `main10`, `0`, `2`.
    pub profile: String,
    /// The FFmpeg pixel format the encoder accepted for it.
    pub pixel_format: String,
    pub bit_depth: u8,
    pub chroma: Chroma,
    /// The highest level that opened and encoded, as the codec writes it
    /// (`5.1`), or `None` where levels were not probed for this codec.
    pub max_level: Option<String>,
}

/// An encoder that exists on this machine, and what it can produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EncoderCapability {
    /// The FFmpeg encoder name, e.g. `hevc_nvenc`.
    pub encoder: String,
    pub source: EncoderSource,
    /// Never empty: an encoder that produced nothing is not reported at all.
    pub profiles: Vec<ProfileCapability>,
}

/// Every encoder that works, per codec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodecCapability {
    pub codec: VideoCodec,
    /// Hardware first, in `ADR-0003`'s order of preference, then software. An
    /// empty list is a real answer: this machine cannot encode the codec.
    pub encoders: Vec<EncoderCapability>,
}

/// The machine's encoder capability profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EncoderCapabilities {
    /// One entry per [`VideoCodec`], always all four, in a fixed order.
    pub codecs: Vec<CodecCapability>,
}

impl EncoderCapabilities {
    /// The encoders for `codec`, best first.
    #[must_use]
    pub fn encoders_for(&self, codec: VideoCodec) -> &[EncoderCapability] {
        self.codecs
            .iter()
            .find(|entry| entry.codec == codec)
            .map_or(&[], |entry| entry.encoders.as_slice())
    }
}

/// A profile worth testing, and how to ask the encoder for it.
#[derive(Debug, Clone, Copy)]
struct ProfileCandidate {
    profile: &'static str,
    /// What `-profile:v` takes for this encoder, which is not always the
    /// codec's own name for it.
    profile_arg: Option<&'static str>,
    pixel_format: &'static str,
    bit_depth: u8,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    codec: VideoCodec,
    encoder: &'static str,
    source: EncoderSource,
    profiles: &'static [ProfileCandidate],
    /// Levels to try, highest first, as `(label, value passed to -level)`.
    /// Empty where the encoder's level option is not comparable across vendors.
    levels: &'static [(&'static str, &'static str)],
}

const fn p(
    profile: &'static str,
    profile_arg: Option<&'static str>,
    pixel_format: &'static str,
    bit_depth: u8,
) -> ProfileCandidate {
    ProfileCandidate {
        profile,
        profile_arg,
        pixel_format,
        bit_depth,
    }
}

// H.264 `level_idc` is the level times ten; HEVC `general_level_idc` is the
// level times thirty. NVENC and AMF take those numbers for their own codec.
// Quick Sync does not: its HEVC levels are the level times ten, like H.264's
// (`MFX_LEVEL_HEVC_51` is 51), so HEVC on QSV gets its own table. Found by
// running the probe on an Intel iGPU, where every HEVC level "failed".
const H264_LEVELS: &[(&str, &str)] = &[
    ("6.2", "62"),
    ("6.1", "61"),
    ("6.0", "60"),
    ("5.2", "52"),
    ("5.1", "51"),
    ("5.0", "50"),
    ("4.2", "42"),
    ("4.1", "41"),
];
const HEVC_LEVELS: &[(&str, &str)] = &[
    ("6.2", "186"),
    ("6.1", "183"),
    ("6.0", "180"),
    ("5.2", "156"),
    ("5.1", "153"),
    ("5.0", "150"),
    ("4.1", "123"),
];

const HEVC_QSV_LEVELS: &[(&str, &str)] = &[
    ("6.2", "62"),
    ("6.1", "61"),
    ("6.0", "60"),
    ("5.2", "52"),
    ("5.1", "51"),
    ("5.0", "50"),
    ("4.1", "41"),
];

const H264_PROFILES: &[ProfileCandidate] = &[
    p("main", Some("main"), "yuv420p", 8),
    p("high", Some("high"), "yuv420p", 8),
];
const H264_NVENC_PROFILES: &[ProfileCandidate] = &[
    p("main", Some("main"), "yuv420p", 8),
    p("high", Some("high"), "yuv420p", 8),
    p("high10", Some("high10"), "yuv420p10le", 10),
];
const HEVC_PROFILES: &[ProfileCandidate] = &[
    p("main", Some("main"), "yuv420p", 8),
    p("main10", Some("main10"), "p010le", 10),
];
const AV1_HARDWARE_PROFILES: &[ProfileCandidate] =
    &[p("main", None, "yuv420p", 8), p("main", None, "p010le", 10)];
const AV1_SOFTWARE_PROFILES: &[ProfileCandidate] = &[
    p("main", None, "yuv420p", 8),
    p("main", None, "yuv420p10le", 10),
];
const VP9_PROFILES: &[ProfileCandidate] = &[
    p("0", Some("0"), "yuv420p", 8),
    p("2", Some("2"), "yuv420p10le", 10),
];

/// Every encoder Blinkify would use, in `ADR-0003`'s order of preference.
const CANDIDATES: &[Candidate] = &[
    Candidate {
        codec: VideoCodec::H264,
        encoder: "h264_nvenc",
        source: EncoderSource::Nvidia,
        profiles: H264_NVENC_PROFILES,
        levels: H264_LEVELS,
    },
    Candidate {
        codec: VideoCodec::H264,
        encoder: "h264_qsv",
        source: EncoderSource::Intel,
        profiles: H264_PROFILES,
        levels: H264_LEVELS,
    },
    Candidate {
        codec: VideoCodec::H264,
        encoder: "h264_amf",
        source: EncoderSource::Amd,
        profiles: H264_PROFILES,
        levels: H264_LEVELS,
    },
    Candidate {
        codec: VideoCodec::Hevc,
        encoder: "hevc_nvenc",
        source: EncoderSource::Nvidia,
        profiles: HEVC_PROFILES,
        levels: HEVC_LEVELS,
    },
    Candidate {
        codec: VideoCodec::Hevc,
        encoder: "hevc_qsv",
        source: EncoderSource::Intel,
        profiles: HEVC_PROFILES,
        levels: HEVC_QSV_LEVELS,
    },
    Candidate {
        codec: VideoCodec::Hevc,
        encoder: "hevc_amf",
        source: EncoderSource::Amd,
        profiles: HEVC_PROFILES,
        levels: HEVC_LEVELS,
    },
    Candidate {
        codec: VideoCodec::Vp9,
        encoder: "vp9_qsv",
        source: EncoderSource::Intel,
        profiles: VP9_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Vp9,
        encoder: "libvpx-vp9",
        source: EncoderSource::Software,
        profiles: VP9_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Av1,
        encoder: "av1_nvenc",
        source: EncoderSource::Nvidia,
        profiles: AV1_HARDWARE_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Av1,
        encoder: "av1_qsv",
        source: EncoderSource::Intel,
        profiles: AV1_HARDWARE_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Av1,
        encoder: "av1_amf",
        source: EncoderSource::Amd,
        profiles: AV1_HARDWARE_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Av1,
        encoder: "libsvtav1",
        source: EncoderSource::Software,
        profiles: AV1_SOFTWARE_PROFILES,
        levels: &[],
    },
    Candidate {
        codec: VideoCodec::Av1,
        encoder: "libaom-av1",
        source: EncoderSource::Software,
        profiles: AV1_SOFTWARE_PROFILES,
        levels: &[],
    },
];

/// Something that can run the sidecar and say whether it succeeded.
///
/// A seam rather than a direct `Command` so the probe's logic — what is tried,
/// in what order, and what counts as a capability — is testable without a GPU,
/// and so the same probe runs through the orchestrator once it exists.
pub trait EncodeTrial: Sync {
    /// The encoder names the sidecar advertises.
    fn advertised(&self) -> BTreeSet<String>;
    /// Whether `ffmpeg` exits successfully with these arguments.
    fn succeeds(&self, args: &[String]) -> bool;
}

/// Runs trials against the real sidecar.
#[derive(Debug, Clone)]
pub struct SidecarTrial {
    sidecar: Sidecar,
}

impl SidecarTrial {
    #[must_use]
    pub fn new(sidecar: Sidecar) -> Self {
        Self { sidecar }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(self.sidecar.ffmpeg());
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_window(&mut command);
        command
    }
}

impl EncodeTrial for SidecarTrial {
    fn advertised(&self) -> BTreeSet<String> {
        let output = self.command().args(["-hide_banner", "-encoders"]).output();
        match output {
            Ok(output) => parse_encoder_listing(&String::from_utf8_lossy(&output.stdout)),
            Err(_) => BTreeSet::new(),
        }
    }

    fn succeeds(&self, args: &[String]) -> bool {
        self.command()
            .args(args)
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// Names from `ffmpeg -encoders`: everything after the `------` rule, second
/// column.
#[must_use]
pub fn parse_encoder_listing(listing: &str) -> BTreeSet<String> {
    listing
        .lines()
        .skip_while(|line| !line.trim_start().starts_with("---"))
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
        .collect()
}

/// The arguments for one trial encode: a few frames of synthetic video, into
/// the null muxer. Nothing is written anywhere.
fn trial_args(encoder: &str, profile: &ProfileCandidate, level: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        // 1280x720 rather than a thumbnail: several hardware encoders refuse
        // sizes below their minimum, which would read as "no encoder".
        "testsrc2=size=1280x720:rate=30",
        "-frames:v",
        "5",
        "-pix_fmt",
        profile.pixel_format,
        "-c:v",
        encoder,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(profile_arg) = profile.profile_arg {
        args.extend(["-profile:v".to_owned(), profile_arg.to_owned()]);
    }
    if let Some(level) = level {
        args.extend(["-level:v".to_owned(), level.to_owned()]);
    }
    args.extend(["-f".to_owned(), "null".to_owned(), "-".to_owned()]);
    args
}

fn probe_candidate(trial: &dyn EncodeTrial, candidate: &Candidate) -> Option<EncoderCapability> {
    let profiles: Vec<ProfileCapability> = candidate
        .profiles
        .iter()
        .filter(|profile| trial.succeeds(&trial_args(candidate.encoder, profile, None)))
        .map(|profile| ProfileCapability {
            profile: profile.profile.to_owned(),
            pixel_format: profile.pixel_format.to_owned(),
            bit_depth: profile.bit_depth,
            chroma: Chroma::Yuv420,
            max_level: candidate
                .levels
                .iter()
                .find(|(_, value)| {
                    trial.succeeds(&trial_args(candidate.encoder, profile, Some(value)))
                })
                .map(|(label, _)| (*label).to_owned()),
        })
        .collect();
    (!profiles.is_empty()).then(|| EncoderCapability {
        encoder: candidate.encoder.to_owned(),
        source: candidate.source,
        profiles,
    })
}

/// Build the capability profile by trying every candidate encoder.
///
/// Encoders that the sidecar does not advertise are skipped without a trial.
/// The rest are tried concurrently — one thread per encoder, a handful of
/// short-lived processes each — because a hardware encoder that fails usually
/// fails in initialisation, and waiting for them one at a time is several
/// seconds of nothing.
#[must_use]
pub fn probe(trial: &dyn EncodeTrial) -> EncoderCapabilities {
    let advertised = trial.advertised();
    let found: Vec<(VideoCodec, EncoderCapability)> = thread::scope(|scope| {
        let handles: Vec<_> = CANDIDATES
            .iter()
            .filter(|candidate| advertised.contains(candidate.encoder))
            .map(|candidate| {
                scope.spawn(move || {
                    probe_candidate(trial, candidate).map(|found| (candidate.codec, found))
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok().flatten())
            .collect()
    });

    let codecs = [
        VideoCodec::H264,
        VideoCodec::Hevc,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ]
    .into_iter()
    .map(|codec| CodecCapability {
        codec,
        encoders: found
            .iter()
            .filter(|(found_codec, _)| *found_codec == codec)
            .map(|(_, encoder)| encoder.clone())
            .collect(),
    })
    .collect();
    EncoderCapabilities { codecs }
}

/// A GUI application that spawns a console program gets a console window
/// flashed at the user unless it asks for none.
fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[cfg(test)]
// Indexing and `expect` are denied in engine code because a panic on hostile
// input is a crash on a user's project. In a test they are the assertion.
#[allow(clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    /// A machine described by which `(encoder, pixel format, level)` trials
    /// succeed.
    struct FakeMachine {
        advertised: BTreeSet<String>,
        works: fn(encoder: &str, pix_fmt: &str, level: Option<&str>) -> bool,
    }

    impl EncodeTrial for FakeMachine {
        fn advertised(&self) -> BTreeSet<String> {
            self.advertised.clone()
        }
        fn succeeds(&self, args: &[String]) -> bool {
            let after = |flag: &str| {
                args.iter()
                    .position(|arg| arg == flag)
                    .and_then(|index| args.get(index + 1))
                    .map(String::as_str)
            };
            (self.works)(
                after("-c:v").unwrap_or_default(),
                after("-pix_fmt").unwrap_or_default(),
                after("-level:v"),
            )
        }
    }

    fn everything_advertised() -> BTreeSet<String> {
        CANDIDATES
            .iter()
            .map(|candidate| candidate.encoder.to_owned())
            .collect()
    }

    #[test]
    fn an_advertised_encoder_that_fails_to_open_is_not_reported() {
        // The ADR-0003 case: every hardware encoder is listed, none works.
        let machine = FakeMachine {
            advertised: everything_advertised(),
            works: |encoder, _, _| encoder.starts_with("lib"),
        };
        let capabilities = probe(&machine);
        assert!(capabilities.encoders_for(VideoCodec::H264).is_empty());
        assert!(capabilities.encoders_for(VideoCodec::Hevc).is_empty());
        let av1: Vec<_> = capabilities
            .encoders_for(VideoCodec::Av1)
            .iter()
            .map(|e| e.encoder.as_str())
            .collect();
        assert_eq!(av1, ["libsvtav1", "libaom-av1"]);
    }

    #[test]
    fn a_hardware_encoder_reports_only_the_bit_depths_it_produced() {
        // An NVENC generation with no 10-bit HEVC.
        let machine = FakeMachine {
            advertised: everything_advertised(),
            works: |encoder, pix_fmt, _| encoder == "hevc_nvenc" && pix_fmt == "yuv420p",
        };
        let capabilities = probe(&machine);
        let hevc = capabilities.encoders_for(VideoCodec::Hevc);
        assert_eq!(hevc.len(), 1);
        let nvenc = &hevc[0];
        assert_eq!(nvenc.source, EncoderSource::Nvidia);
        assert_eq!(nvenc.profiles.len(), 1);
        assert_eq!(nvenc.profiles[0].profile, "main");
        assert_eq!(nvenc.profiles[0].bit_depth, 8);
    }

    #[test]
    fn the_level_ceiling_is_the_highest_level_that_worked() {
        let machine = FakeMachine {
            advertised: everything_advertised(),
            works: |encoder, _, level| {
                encoder == "h264_qsv" && level.is_none_or(|level| level <= "51")
            },
        };
        let capabilities = probe(&machine);
        let qsv = &capabilities.encoders_for(VideoCodec::H264)[0];
        assert_eq!(qsv.encoder, "h264_qsv");
        assert!(
            qsv.profiles
                .iter()
                .all(|profile| profile.max_level.as_deref() == Some("5.1"))
        );
    }

    #[test]
    fn an_encoder_the_sidecar_does_not_list_is_never_tried() {
        let machine = FakeMachine {
            advertised: BTreeSet::new(),
            works: |_, _, _| true,
        };
        let capabilities = probe(&machine);
        assert_eq!(capabilities.codecs.len(), 4);
        assert!(
            capabilities
                .codecs
                .iter()
                .all(|codec| codec.encoders.is_empty())
        );
    }

    #[test]
    fn hardware_is_preferred_over_software_in_the_order_reported() {
        let machine = FakeMachine {
            advertised: everything_advertised(),
            works: |_, _, _| true,
        };
        let capabilities = probe(&machine);
        let sources: Vec<_> = capabilities
            .encoders_for(VideoCodec::Av1)
            .iter()
            .map(|encoder| encoder.source)
            .collect();
        assert_eq!(
            sources,
            [
                EncoderSource::Nvidia,
                EncoderSource::Intel,
                EncoderSource::Amd,
                EncoderSource::Software,
                EncoderSource::Software,
            ]
        );
    }

    #[test]
    fn no_software_h264_or_hevc_encoder_is_ever_a_candidate() {
        // ADR-0003 part 3, as a property of the table rather than a comment.
        assert!(!CANDIDATES.iter().any(|candidate| {
            matches!(candidate.codec, VideoCodec::H264 | VideoCodec::Hevc)
                && candidate.source == EncoderSource::Software
        }));
    }

    #[test]
    fn the_encoder_listing_parser_reads_names_after_the_legend() {
        let listing =
            "Encoders:\n V..... = Video\n ------\n V....D libsvtav1   SVT-AV1\n A....D aac   AAC\n";
        let names = parse_encoder_listing(listing);
        assert_eq!(names.into_iter().collect::<Vec<_>>(), ["aac", "libsvtav1"]);
    }
}
