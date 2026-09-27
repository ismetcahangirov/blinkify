//! The cropping-window prototype of #126 on the corpus, end to end: every
//! file is copied with its SPS window rewritten by the same tool the ADR's
//! measurements were made with (`examples/crop_window.rs`), and the output is
//! checked on bytes — every picture packet identical to the source's by the
//! hash boundary of #45, FFmpeg decoding exactly the source's pixels inside
//! the window. What other players make of such a file is not testable here;
//! ADR-0020 records it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

#[allow(dead_code, unreachable_pub)]
#[path = "../examples/crop_window.rs"]
mod tool;

use std::path::{Path, PathBuf};

use blinkify_engine::VideoCodec;
use blinkify_engine::export::crop_window::{Rect, WindowError};
use blinkify_engine::export::nut;
use blinkify_engine::export::verify::{decode_errors, payload_hashes};
use blinkify_engine::orchestrator::{Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::probe::{Prober, StreamKind, VideoInfo};

use tool::{RemuxError, remux};

fn video(path: &Path) -> VideoInfo {
    Prober::new(common::orchestrator())
        .probe(path)
        .expect("probe")
        .streams
        .iter()
        .find_map(|stream| match &stream.kind {
            StreamKind::Video(video) => Some(video.as_ref().clone()),
            _ => None,
        })
        .expect("a video stream")
}

/// A 16-pixel border off every edge of the stored picture.
fn border(info: &VideoInfo) -> Rect {
    Rect {
        x: 16,
        y: 16,
        width: info.width - 32,
        height: info.height - 32,
    }
}

/// The MD5 of every decoded frame of `path`, in the stored orientation,
/// after `filter` if one is given.
fn frames(path: &Path, before_input: &[&'static str], filter: Option<String>) -> Vec<String> {
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&collected);
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-noautorotate");
    for flag in before_input {
        command = command.option(flag, "1");
    }
    let mut command = command.input(path).option("-map", "0:v:0");
    if let Some(filter) = filter {
        command = command.option("-vf", filter);
    }
    common::orchestrator()
        .run(
            command.option("-f", "framemd5").output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decodes");
    let text = String::from_utf8_lossy(&collected.lock().expect("lock")).into_owned();
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.rsplit(',').next().map(|md5| md5.trim().to_owned()))
        .collect()
}

fn hashes(path: &Path) -> Vec<[u8; 32]> {
    payload_hashes(&common::orchestrator(), path, "v:0")
        .expect("hashes")
        .into_iter()
        .map(|packet| packet.sha256)
        .collect()
}

/// `source` stream-copied into `output` by the sidecar alone, the video
/// converted to Annex B: what MPEG-TS makes of a copy with no window, access
/// unit delimiters and in-band parameter sets included.
fn plain_copy(source: &Path, output: &Path, codec: VideoCodec) {
    let filter = match codec {
        VideoCodec::Hevc => "hevc_mp4toannexb",
        _ => "h264_mp4toannexb",
    };
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(source)
                .option("-map", "0:v:0")
                .option("-c", "copy")
                .option("-bsf:v", filter)
                .output_file(output),
            Priority::Foreground,
        )
        .expect("copies");
}

/// Copy `name` with a 16-pixel border cropped by the window into `output`,
/// and check everything that can be checked on bytes.
fn crop_and_check(name: &str, output: &Path, codec: VideoCodec, in_band: bool) {
    let source = common::corpus(name);
    let before = video(&source);
    let rect = border(&before);
    let done = remux(&common::orchestrator(), &source, output, rect).expect("remux");
    assert_eq!(done.codec, codec, "{name}");
    assert_eq!(done.window.size(), (rect.width, rect.height), "{name}");
    assert_eq!(
        done.in_band > 0,
        in_band,
        "{name}: packets with an SPS in band"
    );

    // The container and the decoder both say the window's size; a display
    // rotation is kept, applied after the window.
    let after = video(output);
    assert_eq!(
        (after.width, after.height),
        (rect.width, rect.height),
        "{name}"
    );
    assert_eq!(after.rotation, before.rotation, "{name}");

    // Every picture packet is the source's, byte for byte. MPEG-TS adds an
    // access unit delimiter to every packet and the record's SEI to every
    // keyframe, whatever the window, so an in-band output is compared with
    // the sidecar's own plain copy of the source into the same container.
    let reference = if in_band {
        let plain = output.with_extension("plain.ts");
        plain_copy(&source, &plain, codec);
        hashes(&plain)
    } else {
        hashes(&source)
    };
    assert_eq!(hashes(output), reference, "{name}: picture packets");

    // It decodes cleanly, to exactly the source's pixels inside the window.
    // The tool copies every packet, so an edit list's pre-roll is shown: the
    // source is decoded with its edit list ignored to match.
    assert_eq!(
        decode_errors(&common::orchestrator(), output).expect("decode"),
        Vec::<String>::new(),
        "{name}"
    );
    let crop = format!("crop={}:{}:{}:{}", rect.width, rect.height, rect.x, rect.y);
    let expected = frames(&source, &["-ignore_editlist"], Some(crop));
    assert!(!expected.is_empty(), "{name}");
    assert_eq!(
        frames(output, &[], None),
        expected,
        "{name}: decoded pixels"
    );
}

#[test]
fn the_window_crops_every_corpus_stream_without_touching_a_picture_byte() {
    let dir = common::scratch("crop-window-corpus");
    for (name, codec) in [
        ("h264-high-closed-gop.mp4", VideoCodec::H264),
        ("h264-open-gop.mp4", VideoCodec::H264),
        ("h264-high10.mp4", VideoCodec::H264),
        ("hevc-open-gop.mp4", VideoCodec::Hevc),
        ("hevc-main10.mp4", VideoCodec::Hevc),
        ("hevc-hdr10.mp4", VideoCodec::Hevc),
        ("portrait-phone.mp4", VideoCodec::H264),
        ("edit-list.mp4", VideoCodec::H264),
    ] {
        crop_and_check(name, &dir.join(name), codec, false);
    }
}

#[test]
fn an_sps_repeated_in_band_is_rewritten_in_every_packet() {
    // MPEG-TS carries parameter sets only in band, before every keyframe.
    let dir = common::scratch("crop-window-in-band");
    for (name, codec) in [
        ("h264-high-closed-gop.mp4", VideoCodec::H264),
        ("hevc-main10.mp4", VideoCodec::Hevc),
    ] {
        let output: PathBuf = dir.join(name).with_extension("ts");
        crop_and_check(name, &output, codec, true);
    }
}

#[test]
fn a_window_the_codec_cannot_express_is_refused_and_nothing_is_written() {
    let dir = common::scratch("crop-window-refused");
    let source = common::corpus("h264-high-closed-gop.mp4");
    let refused = |rect: Rect, output: &str| {
        let output = dir.join(output);
        let error = remux(&common::orchestrator(), &source, &output, rect).expect_err("refused");
        assert!(!output.exists(), "{rect:?} wrote a file");
        match error {
            RemuxError::Crop(error) => error,
            other => panic!("{rect:?}: {other}"),
        }
    };
    // An odd offset or size: 4:2:0 counts the window in chroma samples.
    for rect in [
        Rect {
            x: 15,
            y: 16,
            width: 608,
            height: 328,
        },
        Rect {
            x: 16,
            y: 16,
            width: 607,
            height: 328,
        },
    ] {
        assert_eq!(
            refused(rect, "odd.mp4"),
            WindowError::Misaligned {
                unit_x: 2,
                unit_y: 2
            }
        );
    }
    // Larger than the picture: 640x368 is the coded picture, whose bottom 8
    // rows were never meant to be shown.
    for rect in [
        Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 368,
        },
        Rect {
            x: 64,
            y: 0,
            width: 640,
            height: 360,
        },
    ] {
        assert_eq!(refused(rect, "large.mp4"), WindowError::OutsidePicture);
    }
}

#[test]
fn parameter_sets_that_change_mid_stream_are_detected() {
    // Two corpus streams of different sizes: the first's configuration, then
    // a keyframe of the second carrying its own SPS in band — what a stream
    // whose parameter sets change part-way looks like to the rewrite.
    let first = read_first_key(&common::corpus("h264-high-closed-gop.mp4"));
    let second = read_first_key(&common::corpus("portrait-phone.mp4"));
    let window = blinkify_engine::export::crop_window::CropWindow::new(
        VideoCodec::H264,
        &first.0,
        Rect {
            x: 16,
            y: 16,
            width: 608,
            height: 328,
        },
    )
    .expect("window");
    // The first stream's own keyframe, with its SPS in band, is rewritten.
    window.packet(&first.1).expect("same picture");
    assert_eq!(
        window.packet(&second.1),
        Err(WindowError::ParameterSetsChange)
    );
}

/// The `avcC` record of `path`'s video and its first keyframe with the
/// record's SPS and PPS put in band before it, as a stream that repeats them
/// carries it.
fn read_first_key(path: &Path) -> (Vec<u8>, Vec<u8>) {
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&collected);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:v:0")
                .option("-frames:v", "1")
                .option("-c", "copy")
                .option("-f", "nut")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("reads");
    let bytes = collected.lock().expect("lock").clone();
    let mut reader = nut::Reader::open(bytes.as_slice()).expect("nut");
    let record = reader.header().streams[0].extradata.clone();
    let key = reader.next_packet().expect("packet").expect("a keyframe");
    // avcC: SPS then PPS, each after a 16-bit length; framed here with the
    // stream's 4-byte lengths.
    let sps_length = usize::from(u16::from_be_bytes([record[6], record[7]]));
    let sps = &record[8..8 + sps_length];
    let at = 8 + sps_length + 1;
    let pps_length = usize::from(u16::from_be_bytes([record[at], record[at + 1]]));
    let pps = &record[at + 2..at + 2 + pps_length];
    let mut packet = Vec::new();
    for unit in [sps, pps] {
        packet.extend_from_slice(&u32::try_from(unit.len()).expect("short").to_be_bytes());
        packet.extend_from_slice(unit);
    }
    packet.extend_from_slice(&key.data);
    (record, packet)
}
