//! The full re-encode executor (#55): segments the plan cannot copy are
//! rendered at the sequence's shape and rate and joined to the copied ones,
//! and every copied packet around them is still the source's.
//!
//! The sources are VP9, made here with the sidecar's software encoder, so
//! the tests run on every machine; H.264 needs a hardware encoder (ADR-0003)
//! and is covered where one exists.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::integer_division
)]

mod common;

use std::path::Path;
use std::time::Duration;

use blinkify_engine::capability::VideoCodec;
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{ExportError, ExportRequest, export, partial_path};
use blinkify_engine::export::plan::{ExportPlan, Media};
use blinkify_engine::export::render::REVERSE_CHUNK_FRAMES;
use blinkify_engine::orchestrator::{CancelToken, Priority};
use blinkify_engine::probe::{Prober, Rational, StreamKind};
use blinkify_engine::project::{Operation, SequenceSettings, Track, TrackKind};
use blinkify_engine::tier::{ExportTier, ReEncodeReason};
use common::fixture::{Source, keyframed, packets, speed};

/// A 30 fps VP9 source in Matroska: ticks are milliseconds.
fn vp9() -> Source {
    keyframed(
        "render-vp9",
        &[
            ("-c:v", "libvpx-vp9"),
            ("-deadline", "realtime"),
            ("-b:v", "0"),
            ("-crf", "30"),
        ],
    )
}

fn tiers(plan: &ExportPlan) -> Vec<ExportTier> {
    plan.segments
        .iter()
        .filter(|s| s.media == Media::Video)
        .map(|s| s.tier)
        .collect()
}

fn frames(path: &Path) -> usize {
    common::packet_times(path, "v:0").len()
}

/// Every source packet shown in `from..to`, from its keyframe, in order.
fn copied(source: &Source, from: i64, to: i64) -> Vec<u64> {
    let all = packets(&source.path, VideoCodec::Vp9);
    let (low, high) = (source.seconds(from), source.seconds(to));
    all.iter()
        .skip_while(|p| !(p.1 && (p.0 - low).abs() < 1e-6))
        .take_while(|p| !(p.1 && p.0 >= high - 1e-6))
        .filter(|p| p.0 >= low - 1e-6 && p.0 < high - 1e-6)
        .map(|p| p.2)
        .collect()
}

fn contains_run(haystack: &[u64], needle: &[u64]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn a_held_frame_holds_the_exact_frame_between_copies() {
    let source = vp9();
    let (k0, k1, k2, k3) = (
        source.keyframe(0),
        source.keyframe(1),
        source.keyframe(2),
        source.keyframe(3),
    );
    let held_at = k1 + 5 * 33;
    let plan = source.plan_clips(vec![
        source.clip(1, 0, k0, k1, &[]),
        source.clip(
            2,
            30,
            held_at,
            held_at + 33,
            &[Operation::Freeze { frames: 45 }],
        ),
        source.clip(3, 75, k2, k3, &[]),
    ]);
    assert_eq!(
        tiers(&plan),
        vec![
            ExportTier::StreamCopy,
            ExportTier::FullReEncode {
                reason: ReEncodeReason::FreezeFrame
            },
            ExportTier::StreamCopy,
        ]
    );
    let target = common::scratch("render-hold").join("held.mkv");
    let outcome = source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    assert_eq!(frames(&target), 30 + 45 + 30);
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    // The held frames are the source's frame at the hold, 45 times.
    let psnr = common::fixture::min_psnr_of_hold(&target, &source.path, 30, 75, held_at);
    assert!(psnr > 30.0, "held frames' PSNR {psnr}");
    // The copied segments either side are the source's own packets.
    let output: Vec<u64> = packets(&target, VideoCodec::Vp9)
        .iter()
        .map(|p| p.2)
        .collect();
    assert!(contains_run(&output, &copied(&source, k0, k1)));
    assert!(contains_run(&output, &copied(&source, k2, k3)));
    // The re-encode is named in what ran, and nothing else was encoded.
    assert_eq!(
        outcome
            .commands
            .iter()
            .filter(|c| c.contains("-c:v libvpx-vp9"))
            .count(),
        1
    );
}

#[test]
fn a_reversed_clip_plays_backwards_a_chunk_at_a_time() {
    let source = vp9();
    let (from, to) = (source.keyframe(0), source.keyframe(3));
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[Operation::Reverse])]);
    assert_eq!(
        tiers(&plan),
        vec![ExportTier::FullReEncode {
            reason: ReEncodeReason::Reverse
        }]
    );
    let target = common::scratch("render-reverse").join("reversed.mkv");
    let outcome = source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    assert_eq!(frames(&target), 90);
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    // Frame n of the output is frame 89 − n of the source.
    let psnr = common::fixture::min_psnr_reversed(&target, &source.path, from, to);
    assert!(psnr > 30.0, "reversed frames' PSNR {psnr}");
    // Memory is bounded by the chunk: no decoder is asked for more.
    let chunk = REVERSE_CHUNK_FRAMES * 1000 / 30 + 1;
    let decoders: Vec<&String> = outcome
        .commands
        .iter()
        .filter(|c| c.contains(",reverse,"))
        .collect();
    assert!(decoders.len() >= 3, "{decoders:?}");
    for decoder in decoders {
        let trim = decoder
            .split("trim=start_pts=")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .expect("a trim");
        let (start, end) = trim.split_once(":end_pts=").expect("a range");
        let span: i64 = end.parse::<i64>().expect("end") - start.parse::<i64>().expect("start");
        assert!(span <= chunk, "{decoder}");
    }
}

#[test]
fn a_speed_no_file_carries_is_retimed_to_the_sequence() {
    let source = vp9();
    let (from, to) = (source.keyframe(0), source.keyframe(2));
    // Ten times 30 fps is 300 fps: re-timed to the sequence's 30.
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[speed(10, 1)])]);
    assert_eq!(
        tiers(&plan),
        vec![ExportTier::FullReEncode {
            reason: ReEncodeReason::SpeedFrameRateOutsideContainer
        }]
    );
    let length = plan.length;
    let target = common::scratch("render-speed").join("fast.mkv");
    source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    assert_eq!(frames(&target), usize::try_from(length).expect("small"));
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
}

#[test]
fn a_clip_in_another_shape_is_rendered_at_the_sequences() {
    let source = vp9();
    let settings = SequenceSettings {
        width: 320,
        height: 180,
        frame_rate: Rational { num: 25, den: 1 },
        ..SequenceSettings::default()
    };
    let plan = source.plan_in(
        settings,
        vec![Track::new(
            1,
            TrackKind::Video,
            vec![source.clip(1, 0, source.keyframe(0), source.keyframe(2), &[])],
        )],
    );
    assert_eq!(
        tiers(&plan),
        vec![ExportTier::FullReEncode {
            reason: ReEncodeReason::SequenceSettingsDiffer
        }]
    );
    let target = common::scratch("render-shape").join("small.mkv");
    source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    assert_eq!(frames(&target), 50);
    let info = Prober::new(common::orchestrator())
        .probe(&target)
        .expect("probe");
    let video = info
        .streams
        .iter()
        .find_map(|s| match &s.kind {
            StreamKind::Video(video) => Some(video.clone()),
            _ => None,
        })
        .expect("video");
    assert_eq!((video.width, video.height), (320, 180));
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
}

#[test]
fn a_gap_between_clips_is_black_of_exactly_its_length() {
    let source = vp9();
    let (k0, k1, k2, k3) = (
        source.keyframe(0),
        source.keyframe(1),
        source.keyframe(2),
        source.keyframe(3),
    );
    let plan = source.plan_clips(vec![
        source.clip(1, 0, k0, k1, &[]),
        source.clip(2, 45, k2, k3, &[]),
    ]);
    assert_eq!(
        tiers(&plan)[1],
        ExportTier::FullReEncode {
            reason: ReEncodeReason::Gap
        }
    );
    let target = common::scratch("render-gap").join("gap.mkv");
    source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    assert_eq!(frames(&target), 30 + 15 + 30);
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    let output: Vec<u64> = packets(&target, VideoCodec::Vp9)
        .iter()
        .map(|p| p.2)
        .collect();
    assert!(contains_run(&output, &copied(&source, k2, k3)));
}

#[test]
fn with_no_encoder_a_re_encode_is_refused_not_substituted() {
    // H.264 with this machine's encoders unknown: no encoder qualifies.
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(1) + 512,
        &[Operation::Freeze { frames: 30 }],
    )]);
    let target = common::scratch("render-refused").join("out.mp4");
    assert!(matches!(
        source.export(&plan, &target, AudioTarget::default()),
        Err(ExportError::Declined(_))
    ));
    assert!(!target.exists());
}

#[test]
fn cancelling_mid_segment_leaves_no_file_and_no_process() {
    let source = vp9();
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(0),
        source.end(),
        &[Operation::Reverse],
    )]);
    let target = common::scratch("render-cancel").join("cancelled.mkv");
    let orchestrator = common::orchestrator();
    let cancel = CancelToken::default();
    let inputs = source.inputs();
    let stopper = cancel.clone();
    let observer = orchestrator.clone();
    std::thread::spawn(move || {
        // Once something of the export is running, pull the plug.
        for _ in 0..500 {
            if observer.running(Priority::Export) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        stopper.cancel();
    });
    let result = export(
        &orchestrator,
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &target,
            overwrite: false,
            audio: AudioTarget::Opus { kilobits: 128 },
            models: None,
            cancel,
            on_progress: None,
        },
    );
    assert!(result.is_err(), "{result:?}");
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());
    for _ in 0..200 {
        if orchestrator.running(Priority::Export) == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(orchestrator.running(Priority::Export), 0);
}
