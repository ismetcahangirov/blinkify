//! Smart-cut (#41), end to end on this machine's encoders: a cut between
//! keyframes re-encodes only the window its frames depend on, every packet
//! outside the window is the source's, and the output decodes cleanly across
//! both seams, frame-accurately.
//!
//! H.264 and HEVC seams need a hardware encoder (ADR-0003); where this
//! machine has none, those tests assert the decline instead. VP9 and AV1 have
//! software encoders and run everywhere, CI included.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::integer_division
)]

mod common;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex};

use blinkify_engine::capability::{CodecCapability, EncoderCapabilities, VideoCodec};
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::ExportError;
use blinkify_engine::export::nut;
use blinkify_engine::export::plan::{Cause, Decline, ExportPlan, Media, Segment};
use blinkify_engine::export::profile::Unmatched;
use blinkify_engine::export::seam::is_parameter_set;
use blinkify_engine::orchestrator::{Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::tier::ExportTier;
use common::fixture::{Source, encoders};

fn video(plan: &ExportPlan) -> &Segment {
    plan.segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video")
}

/// Every video packet of `path` as a stream copy reads it: its presentation
/// timestamp in the NUT time base, keyframe flag, and a hash of its payload
/// **without in-band parameter sets** — the hash boundary a smart-cut is held
/// to: a seam re-sends the source's SPS and PPS before the next copied
/// keyframe, and those are the stream's configuration, not its pictures.
fn packets(path: &Path, codec: VideoCodec) -> Vec<(f64, bool, u64)> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .flag("-copyts")
                .input(path)
                .option("-map", "0:v:0")
                .option("-c", "copy")
                .option("-output_ts_offset", "100")
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
    let length_size = match codec {
        VideoCodec::H264 => Some(usize::from(reader.header().streams[0].extradata[4] & 3) + 1),
        VideoCodec::Hevc => Some(usize::from(reader.header().streams[0].extradata[21] & 3) + 1),
        _ => None,
    };
    let base = reader.header().streams[0].time_base;
    // Seconds of the source's own timeline: the reader's offset taken off.
    let seconds = |pts: i64| pts as f64 * base.num as f64 / base.den as f64 - 100.0;
    let mut found = Vec::new();
    while let Some(packet) = reader.next_packet().expect("packet") {
        let mut hasher = DefaultHasher::new();
        match length_size {
            Some(size) => {
                let mut at = 0;
                while at + size <= packet.data.len() {
                    let length = packet.data[at..at + size]
                        .iter()
                        .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
                    let unit = &packet.data[at + size..(at + size + length).min(packet.data.len())];
                    if !is_parameter_set(codec, unit) {
                        unit.hash(&mut hasher);
                    }
                    at += size + length;
                }
            }
            None => packet.data.hash(&mut hasher),
        }
        found.push((seconds(packet.pts), packet.key, hasher.finish()));
    }
    found
}

/// The frames of `path` shown, decoded: how many, and the PSNR summary of
/// the output's frames against the source's frames of `from..to`.
fn frame_count(path: &Path) -> usize {
    common::packet_times(path, "v:0").len()
}

fn min_psnr(output: &Path, source: &Path, from: i64, to: i64) -> f64 {
    let result = common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "info")
                .input(output)
                .flag("-copyts")
                .input(source)
                .option(
                    "-filter_complex",
                    format!(
                        "[1:v]trim=start_pts={from}:end_pts={to},setpts=PTS-STARTPTS[r];[0:v]setpts=PTS-STARTPTS[o];[o][r]psnr=stats_file=-"
                    ),
                )
                .output_null(),
            Priority::Foreground,
        )
        .expect("compares");
    // Per-frame lines on stdout are not collected; the summary on stderr has
    // the minimum: `PSNR y:… average:… min:… max:…`.
    let summary = result
        .stderr_tail
        .iter()
        .find(|line| line.contains("PSNR") && line.contains("min:"))
        .cloned()
        .unwrap_or_default();
    summary
        .split("min:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map_or(0.0, |value| {
            if value == "inf" {
                f64::INFINITY
            } else {
                value.parse().unwrap_or(0.0)
            }
        })
}

/// The frames the source shows in `from..to`.
fn shown(source: &Source, from: i64, to: i64) -> usize {
    common::packet_times(&source.path, "v:0")
        .iter()
        // `pts_time` is printed to microseconds: half a frame of margin.
        .filter(|t| **t >= source.seconds(from) - 1e-3 && **t < source.seconds(to) - 1e-3)
        .count()
}

/// Cut `name` from a frame after its second keyframe to a few frames after
/// its fourth, and check everything a smart-cut promises.
fn smart_cut(name: &str, codec: VideoCodec, frame: i64) {
    smart_cut_of(&Source::with_encoders(name), name, codec, frame);
}

fn smart_cut_of(source: &Source, name: &str, codec: VideoCodec, frame: i64) {
    let from = source.keyframe(1) + 3 * frame;
    let to = source.keyframe(3) + 4 * frame;
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[])]);
    let segment = video(&plan);
    assert!(
        matches!(segment.tier, ExportTier::SmartCut { .. }),
        "{name}: {:?}",
        segment.tier
    );
    if let Some(decline) = &segment.decline {
        // No encoder here can make this stream: declined, with the
        // keyframe-aligned cut offered, never a mismatched seam.
        assert!(
            encoders().encoders_for(codec).is_empty(),
            "{name}: {decline:?}"
        );
        assert!(segment.alternative.is_some());
        let target = common::scratch(&format!("smartcut-{name}-declined")).join("out.mkv");
        assert!(matches!(
            source.export(&plan, &target, AudioTarget::default()),
            Err(ExportError::Declined(_))
        ));
        assert!(!target.exists());
        return;
    }
    let container = if matches!(codec, VideoCodec::Vp9 | VideoCodec::Av1) {
        "mkv"
    } else {
        "mp4"
    };
    let target = common::scratch(&format!("smartcut-{name}")).join(format!("cut.{container}"));
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");

    // Frame-accurate: exactly the frames the source shows in the range.
    assert_eq!(frame_count(&target), shown(source, from, to), "{name}");
    // Decodes cleanly across both seams.
    assert_eq!(
        common::decode_errors(&target),
        Vec::<String>::new(),
        "{name}"
    );
    // The seams look like the source.
    let psnr = min_psnr(&target, &source.path, from, to);
    assert!(psnr > 30.0, "{name}: minimum PSNR {psnr}");
    // Every packet outside the windows is the source's own.
    // The copied pieces: from the head window's end (or the in-point) to the
    // tail window's start (or the out-point), in decode order from the
    // copied keyframe until the keyframe the copy stops on.
    let head = segment
        .causes
        .iter()
        .any(|c| matches!(c, Cause::InPointNotKeyframe { .. }));
    let tail = segment.causes.iter().any(|c| {
        matches!(
            c,
            Cause::OutPointNotKeyframe { .. } | Cause::OpenGopAtOutPoint { .. }
        )
    });
    let copy_from = if head { segment.windows[0].to } else { from };
    let copy_to = if tail {
        segment.windows.last().expect("tail window").from
    } else {
        to
    };
    if copy_from >= copy_to {
        return;
    }
    let (low, high) = (source.seconds(copy_from), source.seconds(copy_to));
    let source_packets = packets(&source.path, codec);
    let start = source_packets
        .iter()
        .position(|p| p.1 && (p.0 - low).abs() < 1e-6)
        .expect("the copied GOP's keyframe");
    let copied: Vec<u64> = source_packets[start..]
        .iter()
        .skip(1)
        .take_while(|p| !(p.1 && p.0 >= high - 1e-6))
        .chain(std::iter::once(&source_packets[start]))
        .filter(|p| p.0 >= low - 1e-6 && p.0 < high - 1e-6)
        .map(|p| p.2)
        .collect();
    // The keyframe first, as the stream has it.
    let copied: Vec<u64> = std::iter::once(source_packets[start].2)
        .chain(copied.into_iter().filter(|h| *h != source_packets[start].2))
        .collect();
    let output: Vec<u64> = packets(&target, codec).iter().map(|p| p.2).collect();
    let at = output
        .iter()
        .position(|h| *h == copied[0])
        .expect("the first copied packet is in the output");
    assert_eq!(output[at..at + copied.len()].to_vec(), copied, "{name}");
    assert!(
        outcome
            .commands
            .iter()
            .any(|c| c.contains("trim=start_pts"))
    );
}

#[test]
fn h264_high_is_cut_frame_accurately_or_declined() {
    // A frame is 512 ticks of 1/15360.
    smart_cut("h264-high-closed-gop.mp4", VideoCodec::H264, 512);
}

#[test]
fn an_h264_open_gop_is_recoded_to_the_next_idr_never_copied_from_a_recovery_point() {
    smart_cut("h264-open-gop.mp4", VideoCodec::H264, 512);
}

#[test]
fn hevc_is_cut_where_a_hardware_encoder_exists_and_declined_where_not() {
    smart_cut("hevc-closed-gop-radl.mp4", VideoCodec::Hevc, 512);
}

/// A four-second file with a keyframe every second, made here with the
/// sidecar's own software encoder: the corpus's VP9 and AV1 files have one
/// keyframe each, and a smart-cut needs a GOP to copy between two seams.
fn keyframed(name: &str, arguments: &[(&'static str, &str)]) -> Source {
    let path = common::scratch(&format!("smartcut-source-{name}")).join("source.mkv");
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .lavfi_input("testsrc2=size=640x360:rate=30:duration=4")
        .option("-pix_fmt", "yuv420p")
        .option("-g", "30")
        .option("-keyint_min", "30");
    for (name, value) in arguments {
        command = command.option(name, (*value).to_owned());
    }
    common::orchestrator()
        .run_to_end(command.output_file(&path), Priority::Foreground)
        .expect("encodes");
    Source::at(path, Some(encoders()))
}

#[test]
fn vp9_is_cut_with_the_software_encoder_on_every_machine() {
    let source = keyframed(
        "vp9",
        &[
            ("-c:v", "libvpx-vp9"),
            ("-deadline", "realtime"),
            ("-b:v", "0"),
            ("-crf", "30"),
        ],
    );
    // Matroska ticks are milliseconds: a frame is about 33.
    smart_cut_of(&source, "vp9", VideoCodec::Vp9, 33);
}

#[test]
fn av1_is_cut_with_the_software_encoder_on_every_machine() {
    let source = keyframed("av1", &[("-c:v", "libsvtav1"), ("-preset", "12")]);
    smart_cut_of(&source, "av1", VideoCodec::Av1, 33);
}

#[test]
fn with_no_encoder_here_hevc_is_declined_and_the_keyframe_cut_offered() {
    let none = EncoderCapabilities {
        codecs: [
            VideoCodec::H264,
            VideoCodec::Hevc,
            VideoCodec::Vp9,
            VideoCodec::Av1,
        ]
        .into_iter()
        .map(|codec| CodecCapability {
            codec,
            encoders: Vec::new(),
        })
        .collect(),
    };
    let source = Source::at(common::corpus("hevc-open-gop.mp4"), Some(&none));
    let from = source.keyframe(1) + 512;
    let plan = source.plan_clips(vec![source.clip(1, 0, from, source.keyframe(3), &[])]);
    let segment = video(&plan);
    assert_eq!(
        segment.decline,
        Some(Decline::NoEncoder {
            unmatched: Unmatched::NoEncoder {
                codec: VideoCodec::Hevc
            }
        })
    );
    let alternative = segment.alternative.expect("the keyframe cut");
    assert!(
        alternative.source_in == source.keyframe(1) || alternative.source_in == source.keyframe(2)
    );
    let target = common::scratch("smartcut-hevc-declined").join("out.mp4");
    assert!(matches!(
        source.export(&plan, &target, AudioTarget::default()),
        Err(ExportError::Declined(_))
    ));
    assert!(!target.exists());
}

#[test]
fn hevc_cra_leading_pictures_are_recoded_after_the_seam() {
    smart_cut("hevc-open-gop.mp4", VideoCodec::Hevc, 512);
}

#[test]
fn a_cut_ending_on_an_open_keyframe_recodes_only_its_leading_pictures() {
    // In on a keyframe, out exactly on an open-GOP keyframe: the pictures
    // shown just before it are decoded after it, so they alone are recoded.
    let source = Source::with_encoders("hevc-open-gop.mp4");
    // The keyframe at 2 s is a CRA with RASL pictures before it.
    let (from, to) = (source.keyframe(1), source.keyframe(2));
    assert!(source.video().keyframes[2].open);
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[])]);
    let segment = video(&plan);
    if segment.decline.is_some() {
        assert!(encoders().encoders_for(VideoCodec::Hevc).is_empty());
        return;
    }
    assert!(
        segment
            .causes
            .iter()
            .any(|c| matches!(c, Cause::OpenGopAtOutPoint { .. })),
        "{:?}",
        segment.causes
    );
    let target = common::scratch("smartcut-open-out").join("cut.mp4");
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(frame_count(&target), shown(&source, from, to));
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    assert!(min_psnr(&target, &source.path, from, to) > 30.0);
    // The recode starts at the first leading picture, not at the GOP before:
    // it covers fewer frames than a GOP.
    let seam = outcome
        .commands
        .iter()
        .find(|c| c.contains("trim=start_pts"))
        .expect("a seam");
    let start: i64 = seam
        .split("trim=start_pts=")
        .nth(1)
        .and_then(|rest| rest.split(':').next())
        .and_then(|value| value.parse().ok())
        .expect("start");
    assert!(start > source.keyframe(1), "{seam}");
    assert!(start < to);
}
