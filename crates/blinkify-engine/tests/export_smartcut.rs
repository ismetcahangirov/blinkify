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

use std::path::Path;

use blinkify_engine::capability::{CodecCapability, EncoderCapabilities, VideoCodec};
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::ExportError;
use blinkify_engine::export::plan::{Cause, Decline, ExportPlan, Media, Segment};
use blinkify_engine::export::profile::Unmatched;

use blinkify_engine::tier::ExportTier;
use common::fixture::{Source, encoders};

fn video(plan: &ExportPlan) -> &Segment {
    plan.segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video")
}

/// The frames of `path` shown, decoded: how many, and the PSNR summary of
/// the output's frames against the source's frames of `from..to`.
fn frame_count(path: &Path) -> usize {
    common::packet_times(path, "v:0").len()
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
    let psnr = common::fixture::min_psnr(&target, &source.path, from, to);
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
    let source_packets = common::fixture::packets(&source.path, codec);
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
    let output: Vec<u64> = common::fixture::packets(&target, codec)
        .iter()
        .map(|p| p.2)
        .collect();
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

#[test]
fn vp9_is_cut_with_the_software_encoder_on_every_machine() {
    let source = common::fixture::keyframed(
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
    let source = common::fixture::keyframed("av1", &[("-c:v", "libsvtav1"), ("-preset", "12")]);
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
    assert!(common::fixture::min_psnr(&target, &source.path, from, to) > 30.0);
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
