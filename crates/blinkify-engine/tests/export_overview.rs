//! What the export dialog states before an export (#50): each stream's
//! claim as the plan decided it, every reason with its time, the keyframe
//! snap and what accepting it does to the plan, the size, and the space for
//! it.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::integer_division
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::overview::{ExportOverview, OverviewRequest, overview};
use blinkify_engine::export::plan::{ExportPlan, Media, plan};
use blinkify_engine::project::edit::{Document, Edit, EditContext};
use blinkify_engine::project::evaluate::{Timeline, evaluate};
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::ExportTier;
use common::fixture::Source;

/// A project of `clips` on one video track over `source`.
fn project(source: &Source, clips: Vec<Clip>) -> Project {
    let settings = SequenceSettings::matching(&source.video().geometry).expect("valid");
    let mut project = Project::new("overview", settings);
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(source.path.clone()).export_source()).expect("ref"),
    );
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, clips)];
    project
}

fn planned(source: &Source, project: &Project) -> (Timeline, ExportPlan) {
    let timeline = evaluate(project).expect("evaluates");
    let plan = plan(
        &timeline,
        &project.sequence.settings,
        &BTreeMap::from([(1, source.facts.clone())]),
    )
    .expect("plans");
    (timeline, plan)
}

fn overview_of(
    source: &Source,
    project: &Project,
    target: &Path,
    available: Option<u64>,
) -> (ExportPlan, ExportOverview) {
    let (timeline, plan) = planned(source, project);
    let inputs = source.inputs();
    let told = overview(&OverviewRequest {
        plan: &plan,
        timeline: &timeline,
        inputs: &inputs,
        target,
        audio: AudioTarget::default(),
        models: None,
        available,
    });
    (plan, told)
}

/// Every claim the overview makes is the plan's, stream by stream.
fn assert_matches_plan(plan: &ExportPlan, told: &ExportOverview) {
    assert_eq!(told.lossless, plan.summary.lossless);
    for (media, claim, totals) in [
        (Media::Video, told.video, plan.summary.video),
        (Media::Audio, told.audio, plan.summary.audio),
    ] {
        let has = plan.segments.iter().any(|s| s.media == media);
        assert_eq!(claim.is_some(), has, "{media:?}");
        if let Some(claim) = claim {
            assert_eq!(claim.lossless, totals.re_encoded == 0, "{media:?}");
            let copied = plan
                .segments
                .iter()
                .any(|s| s.media == media && s.tier.is_lossless());
            assert_eq!(claim.copied_seconds > 0.0, copied || totals.copied > 0);
        }
    }
    // Every segment that is not a copy has a reason, and a copy has none.
    for segment in &plan.segments {
        let named = told.reasons.iter().any(|reason| {
            reason.media == segment.media
                && reason.tier == segment.tier
                && !reason.sentence.is_empty()
        });
        assert_eq!(named, !segment.tier.is_lossless(), "{segment:?}");
    }
}

#[test]
fn an_aligned_cut_is_claimed_lossless_with_nothing_to_explain() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let clip = source.clip(1, 0, source.keyframe(1), source.keyframe(3), &[]);
    let target = common::scratch("overview-aligned").join("out.mp4");
    let (plan, told) = overview_of(&source, &project(&source, vec![clip]), &target, None);
    assert_matches_plan(&plan, &told);
    assert!(told.lossless);
    assert!(told.video.is_some_and(|v| v.lossless));
    assert!(told.reasons.is_empty());
    assert!(told.snap.is_none());
    assert!(told.size.precise);
    assert!(told.problems.is_empty(), "{:?}", told.problems);
}

#[test]
fn a_cut_between_keyframes_names_its_time_and_its_distance_from_the_keyframe() {
    let source = Source::with_encoders("vp9-keyframes.webm");
    let frame = source.video().time_base.den / 30;
    let from = source.keyframe(1) + 12 * frame;
    let clip = source.clip(1, 0, from, source.keyframe(3), &[]);
    let target = common::scratch("overview-smartcut").join("out.mkv");
    let (plan, told) = overview_of(&source, &project(&source, vec![clip]), &target, None);
    assert_matches_plan(&plan, &told);
    assert!(!told.lossless);
    assert!(!told.video.expect("video").lossless);
    let reason = told
        .reasons
        .iter()
        .find(|r| r.media == Media::Video)
        .expect("a reason");
    assert!(matches!(reason.tier, ExportTier::SmartCut { .. }));
    assert!((reason.at_seconds - 0.0).abs() < 1e-9);
    // Twelve frames after a keyframe, eighteen before the next: 0.40 s.
    assert!(
        reason.sentence.contains("0.40 s from the nearest keyframe"),
        "{}",
        reason.sentence
    );
    assert!(!told.size.precise);
}

#[test]
fn a_declined_smart_cut_says_why_in_those_terms_and_offers_the_snap() {
    // No encoders measured: nothing can make a seam that joins this stream.
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let frame = source.video().time_base.den / 30;
    let clip = source.clip(
        1,
        0,
        source.keyframe(1) + 5 * frame,
        source.keyframe(3),
        &[],
    );
    let target = common::scratch("overview-declined").join("out.mp4");
    let (plan, told) = overview_of(&source, &project(&source, vec![clip]), &target, None);
    assert_matches_plan(&plan, &told);
    let declined = told.reasons.iter().find(|r| r.declined).expect("a decline");
    assert!(
        declined.sentence.contains("cannot be smart-cut")
            && declined
                .sentence
                .contains("Snapping the cut to the nearest keyframes"),
        "{}",
        declined.sentence
    );
    // The executor would refuse it, and the dialog says so before.
    assert!(!told.problems.is_empty());
    assert!(told.snap.is_some());
}

#[test]
fn accepting_the_snap_leaves_no_picture_re_encoded() {
    let source = Source::with_encoders("vp9-keyframes.webm");
    let frame = source.video().time_base.den / 30;
    let (k1, k2, k3) = (source.keyframe(1), source.keyframe(2), source.keyframe(3));
    // Two clips back to back, each cut between keyframes at both ends.
    let first = source.clip(1, 0, k1 + 4 * frame, k2 + 20 * frame, &[]);
    let first_length = (k2 + 20 * frame - (k1 + 4 * frame)) / frame;
    let second = source.clip(2, first_length, k2 + 3 * frame, k3 + 25 * frame, &[]);
    let mut document = Document::new(project(&source, vec![first, second])).expect("valid");
    let target = common::scratch("overview-snap").join("out.mkv");

    let (plan, told) = overview_of(&source, document.project(), &target, None);
    assert!(!plan.summary.video.re_encoded.eq(&0));
    let snap = told.snap.expect("a snap is offered");
    assert_eq!(snap.unsnappable, 0);
    assert_eq!(snap.cuts.len(), 2);
    // Each cut says how far it moves, in seconds, and the sequence's change
    // in length is theirs.
    let first_cut = snap.cuts[0];
    assert!(first_cut.in_shift_seconds.abs() <= 0.5 + 1e-9);
    assert!(first_cut.in_shift_seconds.abs() > 0.0);
    let moved: f64 = snap
        .cuts
        .iter()
        .map(|c| c.out_shift_seconds - c.in_shift_seconds)
        .sum();
    assert!((snap.length_change_seconds - moved).abs() < 1e-9);

    let before = document.project().clone();
    document
        .apply(&snap.edit, &EditContext::default())
        .expect("the snap applies");
    let (plan, told) = overview_of(&source, document.project(), &target, None);
    assert_matches_plan(&plan, &told);
    let video: Vec<_> = plan
        .segments
        .iter()
        .filter(|s| s.media == Media::Video)
        .collect();
    assert!(
        video.iter().all(|s| s.tier == ExportTier::StreamCopy),
        "{:?}",
        video.iter().map(|s| s.tier).collect::<Vec<_>>()
    );
    assert_eq!(plan.summary.video.re_encoded, 0);
    assert!(told.video.expect("video").lossless);
    assert!(told.snap.is_none());
    // The second clip moved up to meet the first: no gap was opened.
    let timeline = evaluate(document.project()).expect("evaluates");
    let placements: Vec<_> = timeline.placements().collect();
    assert_eq!(placements[0].end(), placements[1].start);

    // And it is an ordinary edit: undo gives the cuts back exactly.
    document.undo().expect("undo");
    assert_eq!(document.project(), &before);
}

#[test]
fn a_clip_partly_covered_from_above_is_not_snapped() {
    let source = Source::with_encoders("vp9-keyframes.webm");
    let frame = source.video().time_base.den / 30;
    let (k1, k3) = (source.keyframe(1), source.keyframe(3));
    let below = source.clip(1, 0, k1 + 4 * frame, k3, &[]);
    let mut above = source.clip(2, 10, source.keyframe(0), source.keyframe(1), &[]);
    above.id = 2;
    let mut project = project(&source, vec![below]);
    project
        .sequence
        .tracks
        .insert(0, Track::new(2, TrackKind::Video, vec![above]));
    let target = common::scratch("overview-covered").join("out.mkv");
    let (plan, told) = overview_of(&source, &project, &target, None);
    assert_matches_plan(&plan, &told);
    if let Some(snap) = &told.snap {
        // The stretch of the lower clip before the cover starts at its own
        // trim and can move; those the cover cuts cannot.
        assert!(snap.cuts.iter().all(|cut| cut.clip == 1));
    }
}

#[test]
fn sound_and_pictures_are_claimed_apart() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let clip = source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(3),
        &[Operation::Gain {
            db: -6.0,
            ceiling_dbtp: -1.0,
            bypassed: false,
        }],
    );
    let target = common::scratch("overview-gain").join("out.mp4");
    let (plan, told) = overview_of(&source, &project(&source, vec![clip]), &target, None);
    assert_matches_plan(&plan, &told);
    assert!(told.video.expect("video").lossless);
    assert!(!told.audio.expect("audio").lossless);
    assert!(!told.lossless);
    assert!(told.reasons.iter().all(|r| r.media == Media::Audio));
    // The sound's encoding is named: AAC, as the target asks.
    assert_eq!(
        told.audio_encoding.as_ref().map(|e| e.codec.as_str()),
        Some("aac")
    );
}

#[test]
fn too_little_space_is_reported_before_the_export() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let clip = source.clip(1, 0, source.keyframe(0), source.end(), &[]);
    let target = common::scratch("overview-space").join("out.mp4");
    let (_, told) = overview_of(&source, &project(&source, vec![clip]), &target, Some(1024));
    let space = told.space.expect("space");
    assert!(!space.enough);
    assert!(
        told.problems.iter().any(|p| p.contains("free")),
        "{:?}",
        told.problems
    );
    let (_, plenty) = overview_of(
        &source,
        &project(
            &source,
            vec![source.clip(1, 0, source.keyframe(0), source.end(), &[])],
        ),
        &target,
        Some(u64::MAX / 2),
    );
    assert!(plenty.space.expect("space").enough);
    assert!(plenty.problems.is_empty(), "{:?}", plenty.problems);
}

/// The documented tolerance for a copy whose files record each stream's
/// rate: the estimate is those rates over the copied time, and the output
/// differs by the variation of the rate across the file and the container's
/// own overhead.
const COPY_TOLERANCE: f64 = 0.10;

#[test]
fn a_copy_is_estimated_within_the_documented_tolerance() {
    for (name, whole, precise) in [
        ("h264-high-closed-gop.mp4", true, true),
        ("h264-high-closed-gop.mp4", false, true),
        ("av1.mp4", true, true),
        // Matroska records no stream's rate: with pictures and sound both
        // unrecorded, the split is assumed, and said to be.
        ("vp9-keyframes.webm", true, false),
        ("multi-audio.mkv", true, false),
    ] {
        let source = Source::corpus(name);
        let (from, to) = if whole {
            (source.first(), source.end())
        } else {
            (source.keyframe(1), source.keyframe(3))
        };
        let clip = source.clip(1, 0, from, to, &[]);
        let project = project(&source, vec![clip]);
        let extension = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mkv");
        let extension = if extension == "webm" {
            "mkv"
        } else {
            extension
        };
        let target = common::scratch(&format!("overview-size-{name}-{whole}"))
            .join(format!("out.{extension}"));
        let (plan, told) = overview_of(&source, &project, &target, None);
        assert_eq!(told.size.precise, precise, "{name}");
        if !precise {
            continue;
        }
        source
            .export(&plan, &target, AudioTarget::default())
            .expect("exports");
        let actual = std::fs::metadata(&target).expect("output").len() as f64;
        let estimate = told.size.bytes as f64;
        let error = (estimate - actual).abs() / actual;
        assert!(
            error <= COPY_TOLERANCE,
            "{name} (whole: {whole}): estimated {estimate}, wrote {actual}: {:.1} %",
            error * 100.0
        );
    }
}

#[test]
fn preserving_the_source_exactly_copies_an_aligned_timeline_whole() {
    // The preset: the source's own container, and lossless sound should any
    // need encoding. On an aligned timeline nothing does.
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let clip = source.clip(1, 0, source.keyframe(1), source.keyframe(3), &[]);
    let project = project(&source, vec![clip]);
    let target = common::scratch("overview-preserve").join("out.mp4");
    let (timeline, plan) = planned(&source, &project);
    let inputs = source.inputs();
    let told = overview(&OverviewRequest {
        plan: &plan,
        timeline: &timeline,
        inputs: &inputs,
        target: &target,
        audio: AudioTarget::Flac,
        models: None,
        available: None,
    });
    assert!(told.lossless);
    assert!(told.audio_encoding.is_none());
    let outcome = source
        .export(&plan, &target, AudioTarget::Flac)
        .expect("exports");
    assert!(outcome.audio.is_none(), "no sound was encoded");
    assert!(
        outcome
            .commands
            .iter()
            .all(|c| c.contains("-c copy") || !c.contains("-c:")),
        "{:?}",
        outcome.commands
    );
    let edit = Edit::SnapToKeyframes { snaps: Vec::new() };
    assert_eq!(edit.label(), "Snap cuts of 0 clips");
}
