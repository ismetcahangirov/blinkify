//! A cropped clip through the full re-encode executor (#128): its pictures,
//! and only its pictures, are rendered — cut to the rectangle the user drew
//! on the upright picture, then fitted to the sequence — while every packet
//! of the clips around it, and every packet of its own sound, is still the
//! source's, byte for byte, by the hash boundary of #45.
//!
//! The sources are VP9 and AAC in MP4, made here with the sidecar's own
//! encoders, so the render runs on every machine; one of them carries a 90°
//! display matrix, as a phone's portrait clip does. H.264 needs a hardware
//! encoder (ADR-0003): the corpus's portrait phone clip is rendered where one
//! exists and refused where not.
//!
//! **PSNR threshold: 30 dB**, the smallest frame's, against a reference crop
//! FFmpeg makes itself — its own autorotation, then `crop` in display pixels,
//! then the same scale. A crop of the wrong region, the wrong corner of a
//! rotated picture, or a sideways picture measures below 15 dB on these
//! sources; the re-encode itself costs a few dB against 45 and up. The
//! existing render tests (#55) hold held and reversed frames to the same
//! number.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::integer_division
)]

mod common;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportOutcome, ExportRequest, export, partial_path,
};
use blinkify_engine::export::overview::{OverviewRequest, overview};
use blinkify_engine::export::plan::{Decline, ExportPlan, Media, Segment, plan};
use blinkify_engine::export::report::{Execution, ReportRequest, build};
use blinkify_engine::export::verify::{PacketHash, contains, payload_hashes, range_of};
use blinkify_engine::orchestrator::{CancelToken, Priority};
use blinkify_engine::probe::{Prober, Rational, StreamKind, VideoInfo};
use blinkify_engine::project::crop::CropRect;
use blinkify_engine::project::evaluate::{Timeline, evaluate};
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::{ExportTier, ReEncodeReason};
use common::fixture::{Source, encoders, psnr_between, turned};

/// The smallest PSNR a cropped frame may have against FFmpeg's own crop.
const PSNR_THRESHOLD: f64 = 30.0;

const CROPPED: ExportTier = ExportTier::FullReEncode {
    reason: ReEncodeReason::FilterChangesPixels,
};

/// Four seconds of 640 × 360 VP9 and AAC, turned `rotation` degrees.
fn made(name: &str, rotation: u32) -> Source {
    turned(&format!("crop-{name}"), (640, 360), rotation)
}

fn crop(x: u32, y: u32, width: u32, height: u32) -> Operation {
    Operation::crop(CropRect {
        x,
        y,
        width,
        height,
    })
}

fn video_segments(plan: &ExportPlan) -> Vec<&Segment> {
    plan.segments
        .iter()
        .filter(|s| s.media == Media::Video)
        .collect()
}

fn video_info(path: &Path) -> VideoInfo {
    let info = Prober::new(common::orchestrator())
        .probe(path)
        .expect("probe");
    info.streams
        .iter()
        .find_map(|s| match &s.kind {
            StreamKind::Video(video) => Some((**video).clone()),
            _ => None,
        })
        .expect("video")
}

fn hashes(path: &Path, selector: &str) -> Vec<PacketHash> {
    payload_hashes(&common::orchestrator(), path, selector).expect("packets")
}

fn micros(source: &Source, ticks: i64) -> i64 {
    (source.seconds(ticks) * 1_000_000.0).round() as i64
}

/// Output frames shown in `[from, to)` seconds.
fn frames_between(path: &Path, from: f64, to: f64) -> usize {
    common::packet_times(path, "v:0")
        .iter()
        .filter(|t| **t >= from - 1e-3 && **t < to - 1e-3)
        .count()
}

/// The project of `clips` over `source`, in `settings`, as the export
/// dialog and the export both see it.
fn project(source: &Source, settings: SequenceSettings, clips: Vec<Clip>) -> Project {
    let mut project = Project::new("crop", settings);
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(source.path.clone()).export_source()).expect("ref"),
    );
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, clips)];
    project
}

/// Clip 1 copied, clip 2 cropped, clip 3 copied: one second each.
fn three(source: &Source, rect: Operation) -> Vec<Clip> {
    let (k0, k1, k2, k3) = (
        source.keyframe(0),
        source.keyframe(1),
        source.keyframe(2),
        source.keyframe(3),
    );
    vec![
        source.clip(1, 0, k0, k1, &[]),
        source.clip(2, 30, k1, k2, &[rect]),
        source.clip(3, 60, k2, k3, &[]),
    ]
}

/// The PSNR of output frames `from..to` against the source's `k_from..k_to`
/// ticks, cut by FFmpeg — autorotated, cropped to `rect` in display pixels —
/// and brought to `size` as the renderer brings it.
fn cropped_psnr(
    output: &Path,
    (from, to): (i64, i64),
    source: &Source,
    (k_from, k_to): (i64, i64),
    rect: (u32, u32, u32, u32),
    size: (u32, u32),
) -> f64 {
    let (x, y, w, h) = rect;
    let psnr = psnr_between(
        output,
        &source.path,
        &format!("[0:v]trim=start_frame={from}:end_frame={to},setpts=PTS-STARTPTS[o]"),
        &format!(
            "[1:v]trim=start_pts={k_from}:end_pts={k_to},setpts=PTS-STARTPTS,crop={w}:{h}:{x}:{y},scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2:color=black[r]",
            size.0, size.1, size.0, size.1
        ),
    );
    println!("{}: {rect:?} measured {psnr:.1} dB", output.display());
    psnr
}

/// The export dialog, before anything runs: the cropped segment is listed,
/// where it starts, and why.
fn the_dialog_names_the_crop(
    plan: &ExportPlan,
    timeline: &Timeline,
    inputs: &BTreeMap<u32, ExportInput>,
    target: &Path,
) {
    let before = overview(&OverviewRequest {
        plan,
        timeline,
        inputs,
        target,
        audio: AudioTarget::default(),
        models: None,
        available: None,
    });
    assert!(before.problems.is_empty(), "{:?}", before.problems);
    let reasons: Vec<_> = before
        .reasons
        .iter()
        .filter(|r| r.tier == CROPPED)
        .collect();
    assert_eq!(reasons.len(), 1, "{:?}", before.reasons);
    assert_eq!(reasons[0].media, Media::Video);
    assert!((reasons[0].at_seconds - 1.0).abs() < 1e-9);
    assert_eq!(
        reasons[0].sentence,
        "The clip is cropped, so its pictures are re-encoded; its sound is copied."
    );
}

/// Every sound packet of `target` is one of `source`'s, and every source
/// packet wholly inside the cropped clip's `k1..k2` is there, in order.
fn every_sound_packet_is_the_sources(source: &Source, target: &Path, (k1, k2): (i64, i64)) {
    let (source_sound, output_sound) = (hashes(&source.path, "a:0"), hashes(target, "a:0"));
    let known: HashSet<[u8; 32]> = source_sound.iter().map(|p| p.sha256).collect();
    assert!(!output_sound.is_empty());
    assert!(
        output_sound.iter().all(|p| known.contains(&p.sha256)),
        "a sound packet was re-encoded"
    );
    // An AAC packet is 1024 samples: at 48 kHz, 21 333.3 µs.
    let packet = 21_334;
    let inner: Vec<[u8; 32]> = source_sound
        .iter()
        .filter(|p| p.micros >= micros(source, k1) && p.micros + packet <= micros(source, k2))
        .map(|p| p.sha256)
        .collect();
    assert!(inner.len() >= 45, "{}", inner.len());
    assert_eq!(contains(&output_sound, &inner), Ok(()));
}

/// The export report, after: the cropped segment re-encoded as planned, with
/// its reason and what to do instead; every other segment copied.
fn the_report_names_the_crop(
    plan: &ExportPlan,
    outcome: &ExportOutcome,
    inputs: &BTreeMap<u32, ExportInput>,
) {
    let report = build(
        &common::orchestrator(),
        &ReportRequest {
            name: "Crop",
            plan,
            outcome,
            inputs,
            lossless_audio_target: false,
            created: 1_790_458_800_000,
        },
    )
    .expect("measured");
    let cropped = report
        .segments
        .iter()
        .find(|s| s.planned == CROPPED)
        .expect("the cropped segment");
    assert_eq!(cropped.execution, Execution::ReEncoded);
    assert!(cropped.as_planned);
    assert_eq!(cropped.identical, 0);
    assert!(
        cropped.reasons.iter().any(|r| r.contains("cropped")),
        "{:?}",
        cropped.reasons
    );
    assert_eq!(
        cropped.suggestions,
        vec!["Remove the crop and the clip is copied.".to_owned()]
    );
    for segment in report.segments.iter().filter(|s| s.planned != CROPPED) {
        assert_eq!(segment.execution, Execution::Copied, "{segment:?}");
        assert_eq!(segment.identical, segment.packets, "{segment:?}");
    }
}

#[test]
fn a_cropped_clip_between_two_copies_is_the_only_thing_re_encoded() {
    let source = made("between", 0);
    let (k0, k1, k2, k3) = (
        source.keyframe(0),
        source.keyframe(1),
        source.keyframe(2),
        source.keyframe(3),
    );
    let settings = SequenceSettings::matching(&source.video().geometry).expect("valid");
    let project = project(&source, settings, three(&source, crop(160, 90, 320, 180)));
    let timeline = evaluate(&project).expect("evaluates");
    let plan = plan(
        &timeline,
        &settings,
        &BTreeMap::from([(1, source.facts.clone())]),
    )
    .expect("plans");
    let video = video_segments(&plan);
    assert_eq!(
        video.iter().map(|s| s.tier).collect::<Vec<_>>(),
        vec![ExportTier::StreamCopy, CROPPED, ExportTier::StreamCopy]
    );
    assert_eq!(
        video[1].sources[0].crop,
        Some(CropRect {
            x: 160,
            y: 90,
            width: 320,
            height: 180
        }),
        "the rectangle reaches the renderer through the plan"
    );
    // The crop does not reach the sound: every piece of it is a copy.
    assert!(
        plan.segments
            .iter()
            .filter(|s| s.media == Media::Audio)
            .all(|s| s.tier == ExportTier::StreamCopy && s.sources.iter().all(|p| p.crop.is_none())),
        "{:?}",
        plan.segments
    );

    // Before the export: the dialog names the cropped segment, and where.
    let target = common::scratch("crop-between-out").join("out.mp4");
    let inputs = source.inputs();
    the_dialog_names_the_crop(&plan, &timeline, &inputs, &target);

    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());

    // The neighbours' pictures are the source's packets, bit for bit.
    let (source_video, output_video) = (hashes(&source.path, "v:0"), hashes(&target, "v:0"));
    for (from, to) in [(k0, k1), (k2, k3)] {
        let expected =
            range_of(&source_video, micros(&source, from), micros(&source, to)).expect("range");
        assert_eq!(expected.len(), 30);
        assert_eq!(contains(&output_video, &expected), Ok(()), "{from}..{to}");
    }
    // Every sound packet is the source's, the cropped clip's own too.
    every_sound_packet_is_the_sources(&source, &target, (k1, k2));

    // The cropped second is exactly its planned frames, and the region drawn.
    assert_eq!(frames_between(&target, 1.0, 2.0), 30);
    assert_eq!(common::packet_times(&target, "v:0").len(), 90);
    assert!((common::stream_end(&target, "v:0") - 3.0).abs() < 0.04);
    let psnr = cropped_psnr(
        &target,
        (30, 60),
        &source,
        (k1, k2),
        (160, 90, 320, 180),
        (640, 360),
    );
    assert!(psnr > PSNR_THRESHOLD, "cropped frames' PSNR {psnr}");
    // The control: the same frames against the wrong corner fail it.
    let wrong = cropped_psnr(
        &target,
        (30, 60),
        &source,
        (k1, k2),
        (0, 0, 320, 180),
        (640, 360),
    );
    assert!(
        wrong < PSNR_THRESHOLD - 10.0,
        "the wrong corner measured {wrong}"
    );

    // One encoder ran, for the crop, and nothing wrote a cropped file.
    let encodes: Vec<&String> = outcome
        .commands
        .iter()
        .filter(|c| c.contains("-c:v libvpx-vp9"))
        .collect();
    assert_eq!(encodes.len(), 1, "{:?}", outcome.commands);
    assert!(
        encodes[0].contains("crop=320:180:160:90:exact=1"),
        "{}",
        encodes[0]
    );
    assert!(encodes[0].ends_with("pipe:1"), "{}", encodes[0]);
    let dir: Vec<PathBuf> = std::fs::read_dir(target.parent().expect("dir"))
        .expect("dir")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(dir, vec![target.clone()], "only the output was written");

    // After: the report shows it re-encoded as planned, and what to do.
    the_report_names_the_crop(&plan, &outcome, &inputs);
}

#[test]
fn a_crop_of_a_turned_clip_exports_the_region_drawn_upright() {
    // Coded 640 × 360, shown 360 × 640. The rectangle is drawn on the
    // upright picture: 180 × 320 from (40, 100).
    let source = made("portrait", 90);
    assert_eq!(source.video().geometry.rotation, 90);
    let (k1, k2) = (source.keyframe(1), source.keyframe(2));
    let plan = source.plan_clips(three(&source, crop(40, 100, 180, 320)));
    assert_eq!(
        video_segments(&plan)
            .iter()
            .map(|s| s.tier)
            .collect::<Vec<_>>(),
        vec![ExportTier::StreamCopy, CROPPED, ExportTier::StreamCopy]
    );
    let target = common::scratch("crop-portrait-out").join("out.mp4");
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    // The copies keep their coded shape and their display matrix, and the
    // rendered pictures were made to match them — never transposed.
    let written = video_info(&target);
    assert_eq!(
        (written.width, written.height, written.rotation),
        (640, 360, 90)
    );
    let render = outcome
        .commands
        .iter()
        .find(|c| c.contains("-c:v libvpx-vp9"))
        .expect("a render");
    assert!(render.contains("-noautorotate"), "{render}");
    assert!(!render.contains("transpose"), "{render}");
    assert!(render.contains("crop=320:180:220:40:exact=1"), "{render}");
    // Shown upright, the cropped second is the region drawn.
    let psnr = cropped_psnr(
        &target,
        (30, 60),
        &source,
        (k1, k2),
        (40, 100, 180, 320),
        (360, 640),
    );
    assert!(psnr > PSNR_THRESHOLD, "cropped frames' PSNR {psnr}");
    let wrong = cropped_psnr(
        &target,
        (30, 60),
        &source,
        (k1, k2),
        (140, 220, 180, 320),
        (360, 640),
    );
    assert!(
        wrong < PSNR_THRESHOLD - 10.0,
        "the wrong corner measured {wrong}"
    );
    // And the copies either side are the source's packets.
    let (source_video, output_video) = (hashes(&source.path, "v:0"), hashes(&target, "v:0"));
    let first = range_of(
        &source_video,
        micros(&source, source.keyframe(0)),
        micros(&source, k1),
    )
    .expect("range");
    assert_eq!(contains(&output_video, &first), Ok(()));
}

#[test]
fn a_nine_by_sixteen_crop_fills_a_nine_by_sixteen_sequence() {
    let source = made("vertical", 0);
    let (k0, k2) = (source.keyframe(0), source.keyframe(2));
    let settings = SequenceSettings {
        width: 180,
        height: 320,
        frame_rate: Rational { num: 30, den: 1 },
        ..SequenceSettings::default()
    };
    let plan = source.plan_in(
        settings,
        vec![Track::new(
            1,
            TrackKind::Video,
            vec![source.clip(1, 0, k0, k2, &[crop(230, 20, 180, 320)])],
        )],
    );
    assert_eq!(video_segments(&plan)[0].tier, CROPPED);
    let target = common::scratch("crop-vertical-out").join("out.mp4");
    source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    let written = video_info(&target);
    assert_eq!(
        (written.width, written.height, written.rotation),
        (180, 320, 0)
    );
    assert_eq!(common::packet_times(&target, "v:0").len(), 60);
    // Against the crop alone, with no scale and no bars: any bar or any
    // scaling would pull the PSNR far down.
    let psnr = psnr_between(
        &target,
        &source.path,
        "[0:v]setpts=PTS-STARTPTS[o]",
        &format!(
            "[1:v]trim=start_pts={k0}:end_pts={k2},setpts=PTS-STARTPTS,crop=180:320:230:20[r]"
        ),
    );
    assert!(psnr > PSNR_THRESHOLD, "cropped frames' PSNR {psnr}");
    // The left and right columns are picture, not black bars.
    let edges = psnr_between(
        &target,
        &source.path,
        "[0:v]setpts=PTS-STARTPTS,crop=2:320:0:0[o]",
        &format!("[1:v]trim=start_pts={k0}:end_pts={k2},setpts=PTS-STARTPTS,crop=2:320:230:20[r]"),
    );
    assert!(
        edges > PSNR_THRESHOLD - 5.0,
        "the left edge measured {edges}"
    );
}

#[test]
fn a_cropped_portrait_phone_clip_exports_upright_where_an_encoder_exists() {
    // H.264: rendered with a hardware encoder where there is one, refused
    // where there is none — never encoded with whatever is available.
    let source = Source::with_encoders("portrait-phone.mp4");
    assert_eq!(source.video().geometry.rotation, 90);
    let (from, to) = (source.first(), source.end());
    // Drawn upright on the 720 × 1280 picture.
    let rect = (100, 200, 360, 640);
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        from,
        to,
        &[crop(rect.0, rect.1, rect.2, rect.3)],
    )]);
    let segment = video_segments(&plan)[0].clone();
    assert_eq!(segment.tier, CROPPED);
    let target = common::scratch("crop-phone-out").join("out.mp4");
    if segment.decline.is_some() {
        assert!(matches!(segment.decline, Some(Decline::NoEncoder { .. })));
        assert!(matches!(
            source.export(&plan, &target, AudioTarget::default()),
            Err(ExportError::Declined(_))
        ));
        assert!(!target.exists());
        println!("no H.264 encoder here: the crop is refused, as it must be");
        return;
    }
    source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    let frames = common::packet_times(&target, "v:0").len();
    assert_eq!(i64::try_from(frames).expect("frames"), segment.length);
    let psnr = cropped_psnr(
        &target,
        (0, segment.length),
        &source,
        (from, to),
        rect,
        (720, 1280),
    );
    assert!(psnr > PSNR_THRESHOLD, "cropped frames' PSNR {psnr}");
}

#[test]
fn a_crop_of_an_hdr_clip_is_refused_before_anything_is_written() {
    let source = Source::with_encoders("hevc-hdr10.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.first(),
        source.end(),
        &[crop(0, 0, 320, 180)],
    )]);
    let segment = video_segments(&plan)[0].clone();
    assert_eq!(segment.tier, CROPPED);
    assert_eq!(segment.decline, Some(Decline::HdrWouldBeRendered));
    let target = common::scratch("crop-hdr-out").join("out.mp4");
    assert!(matches!(
        source.export(&plan, &target, AudioTarget::default()),
        Err(ExportError::Declined(_))
    ));
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());
}

#[test]
fn with_no_encoder_a_crop_is_refused_not_substituted() {
    // H.264 with this machine's encoders unknown: nothing qualifies.
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(2),
        &[crop(0, 0, 320, 180)],
    )]);
    let segment = video_segments(&plan)[0].clone();
    assert_eq!(segment.tier, CROPPED);
    assert!(matches!(segment.decline, Some(Decline::NoEncoder { .. })));
    let target = common::scratch("crop-no-encoder-out").join("out.mp4");
    assert!(matches!(
        source.export(&plan, &target, AudioTarget::default()),
        Err(ExportError::Declined(_))
    ));
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());
}

#[test]
fn a_source_gone_before_its_crop_is_rendered_fails_and_leaves_nothing() {
    let made = made("gone", 0);
    let dir = common::scratch("crop-gone-out");
    let path = dir.join("going.mp4");
    std::fs::copy(&made.path, &path).expect("copy");
    let source = Source::at(path.clone(), Some(encoders()));
    let plan = source.plan_clips(three(&source, crop(160, 90, 320, 180)));
    std::fs::remove_file(&path).expect("remove");
    let target = dir.join("out.mp4");
    assert!(
        source
            .export(&plan, &target, AudioTarget::default())
            .is_err()
    );
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());
}

#[test]
fn cancelling_a_crop_mid_render_leaves_no_file_and_no_process() {
    let source = made("cancel", 0);
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(0),
        source.end(),
        &[crop(160, 90, 320, 180)],
    )]);
    let target = common::scratch("crop-cancel-out").join("cancelled.mp4");
    let orchestrator = common::orchestrator();
    let cancel = CancelToken::default();
    let inputs = source.inputs();
    let stopper = cancel.clone();
    let observer = orchestrator.clone();
    std::thread::spawn(move || {
        // Once the render is running, pull the plug.
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
            audio: AudioTarget::default(),
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
