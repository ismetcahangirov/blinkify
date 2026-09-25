//! Re-encode profile matching (#44) on this machine's real encoders: the
//! encoder chosen for a source produces a stream that can join it — same
//! profile, level ceiling, chroma, bit depth and colour — and where no
//! encoder can, the answer is a refusal naming the parameter, never a
//! substitute.
//!
//! Which encoders exist differs between machines: a developer's laptop has
//! NVENC, a CI runner has only the software AV1 and VP9 encoders. Each test
//! asserts whichever answer this machine must give.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use blinkify_engine::capability::{self, EncoderCapabilities, VideoCodec};
use blinkify_engine::export::nut;
use blinkify_engine::export::profile::{
    EncoderChoice, SourceProfile, Unmatched, select, source_profile, validate_join,
};
use blinkify_engine::orchestrator::{Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::probe::{Prober, StreamInfo, StreamKind};

fn capabilities() -> &'static EncoderCapabilities {
    static PROFILE: OnceLock<EncoderCapabilities> = OnceLock::new();
    PROFILE.get_or_init(|| capability::probe(&common::orchestrator()))
}

fn video_stream(path: &Path) -> StreamInfo {
    Prober::new(common::orchestrator())
        .probe(path)
        .expect("probe")
        .streams
        .iter()
        .find(|s| matches!(s.kind, StreamKind::Video(_)))
        .cloned()
        .expect("video")
}

/// The NUT stream header `command` writes.
fn nut_header(command: SidecarCommand) -> nut::StreamHeader {
    let (sender, chunks) = std::sync::mpsc::channel::<Vec<u8>>();
    let job = common::orchestrator().run(
        command.option("-f", "nut").output_stdout(),
        Priority::Foreground,
        JobOptions::default().on_chunk(move |chunk| {
            let _ = sender.send(chunk.to_vec());
            Flow::Continue
        }),
    );
    job.wait().expect("runs");
    let bytes: Vec<u8> = chunks.try_iter().flatten().collect();
    let reader = nut::Reader::open(bytes.as_slice()).expect("nut");
    reader.header().streams[0].clone()
}

/// The source's codec configuration, as a stream copy carries it.
fn source_configuration(path: &Path) -> Vec<u8> {
    nut_header(
        SidecarCommand::ffmpeg()
            .option("-v", "error")
            .input(path)
            .option("-map", "0:v:0")
            .option("-c", "copy")
            .option("-frames:v", "1"),
    )
    .extradata
}

/// Encode the first frames of `path` with `choice`, as a seam would be.
fn encode(path: &Path, choice: &EncoderChoice, codec: VideoCodec, out: &Path) -> Vec<u8> {
    let arguments = choice.arguments(codec);
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .input(path)
        .option("-map", "0:v:0")
        .option("-frames:v", "12");
    for (name, value) in &arguments {
        command = command.option(name, value.clone());
    }
    // Once to NUT, to read the configuration as the router would; once to a
    // file, to probe what was made.
    let header = nut_header(command.clone());
    common::orchestrator()
        .run_to_end(command.output_file(out), Priority::Foreground)
        .expect("encodes");
    header.extradata
}

/// Whichever answer this machine must give for `path`: an encoder whose
/// output joins the source, or a refusal naming what could not be matched.
fn matches_or_refuses(path: &Path, name: &str) -> Result<EncoderChoice, Unmatched> {
    let stream = video_stream(path);
    let source: SourceProfile = source_profile(&stream).expect("a matchable source");
    let result = select(&source, capabilities());
    match &result {
        Ok(choice) => {
            let out = common::scratch(&format!("export-profile-{name}")).join("seam.mkv");
            let encoded = encode(path, choice, source.codec, &out);
            assert_eq!(
                validate_join(source.codec, &source_configuration(path), &encoded),
                Ok(()),
                "{} for {name}",
                choice.encoder
            );
            let made = video_stream(&out);
            let StreamKind::Video(made_video) = &made.kind else {
                panic!("video")
            };
            let StreamKind::Video(was) = &stream.kind else {
                panic!("video")
            };
            assert_eq!(made_video.pixel_format, was.pixel_format, "{name}");
            assert_eq!(made_video.bit_depth, was.bit_depth, "{name}");
            assert_eq!(made_video.chroma_subsampling, was.chroma_subsampling);
            assert_eq!(made.profile, stream.profile, "{name}");
            if let Some(range) = was.color.range.as_deref().filter(|r| *r != "unknown") {
                assert_eq!(made_video.color.range.as_deref(), Some(range), "{name}");
            }
        }
        Err(unmatched) => {
            // Only because this machine has no encoder that can: never
            // because one was passed over that could.
            assert!(
                capabilities().encoders_for(source.codec).iter().all(|e| e
                    .profiles
                    .iter()
                    .all(|p| p.profile != source.profile || p.bit_depth != source.bit_depth)),
                "{name}: refused ({unmatched}) though an encoder makes it"
            );
        }
    }
    result
}

#[test]
fn h264_high_gets_an_encoder_that_joins_it_or_a_named_refusal() {
    let result = matches_or_refuses(&common::corpus("h264-high-closed-gop.mp4"), "h264");
    if capabilities().encoders_for(VideoCodec::H264).is_empty() {
        // ADR-0003: no software H.264 encoder ships. This is the CI answer.
        assert_eq!(
            result,
            Err(Unmatched::NoEncoder {
                codec: VideoCodec::H264
            })
        );
    }
}

#[test]
fn hevc_without_a_hardware_encoder_is_refused_not_substituted() {
    let result = matches_or_refuses(&common::corpus("hevc-closed-gop-radl.mp4"), "hevc");
    if capabilities().encoders_for(VideoCodec::Hevc).is_empty() {
        assert_eq!(
            result,
            Err(Unmatched::NoEncoder {
                codec: VideoCodec::Hevc
            })
        );
    } else {
        assert!(
            result
                .expect("an HEVC encoder")
                .encoder
                .starts_with("hevc_")
        );
    }
}

#[test]
fn hdr_is_refused_before_any_encoder_is_considered() {
    let stream = video_stream(&common::corpus("hevc-hdr10.mp4"));
    assert_eq!(source_profile(&stream), Err(Unmatched::Hdr));
}

/// A ten-bit AV1 stream, made here: the corpus has no ten-bit SDR source,
/// and the software AV1 encoder exists on every machine.
fn ten_bit_av1() -> PathBuf {
    let path = common::scratch("export-profile-source-av1-10").join("source.mkv");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .lavfi_input("testsrc2=size=640x360:rate=30:duration=1")
                .option("-pix_fmt", "yuv420p10le")
                .option("-c:v", "libsvtav1")
                .option("-preset", "12")
                .option("-color_range", "tv")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("encodes");
    path
}

#[test]
fn a_ten_bit_source_is_matched_at_ten_bits() {
    let path = ten_bit_av1();
    let choice = matches_or_refuses(&path, "av1-10").expect("AV1 always has an encoder");
    assert_eq!(choice.bit_depth, 10);
    assert!(
        choice.pixel_format.contains("10"),
        "{}",
        choice.pixel_format
    );
}

#[test]
fn vp9_is_matched_by_the_software_encoder_everywhere() {
    let choice = matches_or_refuses(&common::corpus("vp9.webm"), "vp9").expect("libvpx");
    assert!(choice.encoder.contains("vp9"));
}
