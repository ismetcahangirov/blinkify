//! Lossless constant speed by timestamp rescaling (#42): the pictures'
//! packets are copied untouched and only their timestamps change; the sound
//! is resampled beside them and stays in sync.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

mod common;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::ExportError;
use blinkify_engine::export::plan::Media;
use blinkify_engine::tier::{ExportTier, ReEncodeReason};
use common::fixture::{Source, speed};

fn video_tier(plan: &blinkify_engine::export::plan::ExportPlan) -> ExportTier {
    plan.segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video")
        .tier
}

#[test]
fn double_speed_copies_every_picture_and_halves_its_time() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[speed(2, 1)])]);
    assert_eq!(video_tier(&plan), ExportTier::StreamCopy);
    // The sound is resampled: a speed is not a copy for audio.
    assert!(plan.segments.iter().any(|s| s.media == Media::Audio
        && s.tier
            == ExportTier::FullReEncode {
                reason: ReEncodeReason::AudioSpeed
            }));
    let target = common::scratch("export-speed-double").join("fast.mp4");
    source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");

    // Payloads: bit-identical. Timestamps are outside the hash, by design.
    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        source.expected_video(from, to)
    );
    // Every picture shown at half its source time from the in-point.
    let wanted: Vec<f64> = common::packet_times(&source.path, "v:0")
        .into_iter()
        .filter(|t| *t >= source.seconds(from) - 1e-9 && *t < source.seconds(to) - 1e-9)
        .map(|t| (t - source.seconds(from)) / 2.0)
        .collect();
    let got = common::packet_times(&target, "v:0");
    assert_eq!(got.len(), wanted.len());
    for (got, wanted) in got.iter().zip(&wanted) {
        assert!((got - wanted).abs() < 0.001, "{got} vs {wanted}");
    }
    // Sound and pictures stay together.
    let pictures = common::stream_end(&target, "v:0");
    let sound = common::stream_end(&target, "a:0");
    let expected = (source.seconds(to) - source.seconds(from)) / 2.0;
    assert!(
        (pictures - expected).abs() < 0.02,
        "{pictures} vs {expected}"
    );
    assert!((pictures - sound).abs() < 0.03, "{pictures} vs {sound}");
    assert!(common::decode_errors(&target).is_empty());
}

#[test]
fn a_variable_rate_source_keeps_its_rhythm_packet_by_packet() {
    // 30 fps then 10 fps: rescaling must follow each packet's own time, not
    // multiply a nominal rate.
    let source = Source::corpus("vfr-screen.mp4");
    let (from, to) = (source.first(), source.end());
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[speed(2, 1)])]);
    assert_eq!(video_tier(&plan), ExportTier::StreamCopy);
    let target = common::scratch("export-speed-vfr").join("fast.mp4");
    source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        source.expected_video(from, to)
    );
    let source_times: Vec<f64> = common::packet_times(&source.path, "v:0")
        .into_iter()
        .filter(|t| *t >= source.seconds(from) - 1e-9 && *t < source.seconds(to) - 1e-9)
        .collect();
    let got = common::packet_times(&target, "v:0");
    assert_eq!(got.len(), source_times.len());
    let gaps = |times: &[f64]| -> Vec<f64> { times.windows(2).map(|w| w[1] - w[0]).collect() };
    for (got, wanted) in gaps(&got).iter().zip(gaps(&source_times)) {
        assert!(
            (got - wanted / 2.0).abs() < 0.001,
            "{got} vs {}",
            wanted / 2.0
        );
    }
    assert!(common::decode_errors(&target).is_empty());
}

#[test]
fn an_extreme_slowdown_is_still_a_copy_of_every_frame() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(2));
    // A tenth of 30 fps is 3 fps: carried, lossless, and said to stutter.
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[speed(1, 10)])]);
    assert_eq!(video_tier(&plan), ExportTier::StreamCopy);
    let target = common::scratch("export-speed-slow").join("slow.mp4");
    source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        source.expected_video(from, to)
    );
    let expected = (source.seconds(to) - source.seconds(from)) * 10.0;
    let pictures = common::stream_end(&target, "v:0");
    let sound = common::stream_end(&target, "a:0");
    assert!(
        (pictures - expected).abs() < 0.2,
        "{pictures} vs {expected}"
    );
    assert!((pictures - sound).abs() < 0.2, "{pictures} vs {sound}");
}

#[test]
fn a_rate_no_container_carries_falls_back_with_its_reason() {
    // Ten times 30 fps is 300 fps: the plan re-encodes the pictures at the
    // sequence's rate and says why; nothing is written as a copy at 300 fps.
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[speed(10, 1)])]);
    assert_eq!(
        video_tier(&plan),
        ExportTier::FullReEncode {
            reason: ReEncodeReason::SpeedFrameRateOutsideContainer
        }
    );
    let target = common::scratch("export-speed-fallback").join("too-fast.mp4");
    match source.export(&plan, &target, AudioTarget::default()) {
        // The full re-encode executor (#55) carries it out.
        Ok(_) => assert!(common::decode_errors(&target).is_empty()),
        // Where no encoder here can make it, it is refused, never copied.
        Err(ExportError::Declined(_)) => assert!(!target.exists()),
        Err(other) => panic!("{other}"),
    }
}
