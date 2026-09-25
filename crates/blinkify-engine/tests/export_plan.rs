//! The export planner (#39) over real files: the facts it plans from are read
//! from the corpus by the probe and the keyframe index, and a cut the index
//! says is on a keyframe plans as a copy while one beside it plans as a seam.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::integer_division,
    clippy::cast_precision_loss
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use blinkify_engine::export::facts::source_facts;
use blinkify_engine::export::plan::{Media, SourceFacts, plan};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{Limits, Orchestrator};
use blinkify_engine::probe::Prober;
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::{ExportTier, ReEncodeReason, SeamReason};

fn facts_of(path: &Path) -> SourceFacts {
    let orchestrator = Orchestrator::new(common::sidecar(), Limits::for_this_machine());
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index = KeyframeIndex::open(path, &info, orchestrator, None).expect("index");
    index.complete_in_background(|_| {}).expect("indexed");
    source_facts(&info, Some(&index), None)
}

/// A project over `path` whose sequence is the file's own shape, with one
/// clip trimmed to `from..to` ticks of its video stream.
fn project_of(path: &Path, facts: &SourceFacts, from: i64, to: i64, gain: bool) -> Project {
    let video = facts.video.as_ref().expect("video");
    let mut project = Project::new(
        "t",
        SequenceSettings::matching(&video.geometry).expect("valid"),
    );
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(path.to_path_buf()).export_source()).expect("source"),
    );
    let mut clip = Clip::new(
        1,
        1,
        video.stream,
        video.time_base,
        0,
        vec![Operation::Trim { from, to }],
    );
    if gain {
        clip.push(Operation::Gain { db: 3.0 });
    }
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, vec![clip])];
    project
}

fn tiers(
    path: &Path,
    facts: &SourceFacts,
    from: i64,
    to: i64,
    gain: bool,
) -> Vec<(Media, ExportTier)> {
    let project = project_of(path, facts, from, to, gain);
    let timeline = evaluate(&project).expect("evaluates");
    let plan = plan(
        &timeline,
        &project.sequence.settings,
        &BTreeMap::from([(1, facts.clone())]),
    )
    .expect("plans");
    plan.segments.iter().map(|s| (s.media, s.tier)).collect()
}

#[test]
fn the_facts_are_read_from_the_file() {
    let path = common::corpus("h264-high-closed-gop.mp4");
    let facts = facts_of(&path);
    let video = facts.video.as_ref().expect("video");
    assert!(video.keyframes_complete);
    assert!(video.reorders, "B-frames");
    assert_eq!(video.encoding.codec.as_deref(), Some("h264"));
    assert!(
        video
            .encoding
            .configuration
            .as_deref()
            .is_some_and(|hash| hash.starts_with("SHA256:")),
        "{:?}",
        video.encoding.configuration
    );
    let seconds: Vec<f64> = video
        .keyframes
        .iter()
        .map(|k| k.pts as f64 * video.time_base.num as f64 / video.time_base.den as f64)
        .collect();
    for (found, expected) in seconds.iter().zip([0.0, 0.7, 1.9, 2.2, 3.5]) {
        assert!((found - expected).abs() < 0.02, "{seconds:?}");
    }
    assert!(video.keyframes.iter().all(|k| !k.open));
    assert!(facts.default_audio.is_some());

    let open = facts_of(&common::corpus("hevc-open-gop.mp4"));
    let open = open.video.expect("video");
    assert!(open.keyframes.iter().skip(1).any(|k| k.open));
}

#[test]
fn a_cut_on_a_keyframe_copies_and_one_beside_it_is_a_seam() {
    let path = common::corpus("h264-high-closed-gop.mp4");
    let facts = facts_of(&path);
    let video = facts.video.as_ref().expect("video");
    let k = |i: usize| video.keyframes[i].pts;

    let aligned = tiers(&path, &facts, k(1), k(2), false);
    assert!(
        aligned
            .iter()
            .all(|(_, tier)| *tier == ExportTier::StreamCopy),
        "{aligned:?}"
    );

    let one_frame = video.time_base.den / 30 / video.time_base.num;
    let beside = tiers(&path, &facts, k(1) + one_frame, k(2), false);
    assert_eq!(
        beside[0],
        (
            Media::Video,
            ExportTier::SmartCut {
                reason: SeamReason::InPointNotKeyframeAligned
            }
        )
    );

    let loud = tiers(&path, &facts, k(1), k(2), true);
    assert_eq!(loud[0], (Media::Video, ExportTier::StreamCopy));
    assert_eq!(
        loud[1],
        (
            Media::Audio,
            ExportTier::FullReEncode {
                reason: ReEncodeReason::AudioFilter
            }
        )
    );
}

#[test]
fn a_whole_file_copies_to_its_end() {
    for name in [
        "h264-high-closed-gop.mp4",
        "vp9.webm",
        "av1.mp4",
        "vfr-screen.mp4",
    ] {
        let path = common::corpus(name);
        let facts = facts_of(&path);
        let video = facts.video.as_ref().expect("video");
        let end = video.end.expect("end");
        let first = video.keyframes[0].pts.max(0);
        let whole = tiers(&path, &facts, first, end, false);
        assert!(
            whole
                .iter()
                .all(|(_, tier)| *tier == ExportTier::StreamCopy),
            "{name}: {whole:?}"
        );
    }
}

#[test]
fn an_edit_listed_file_does_not_start_on_a_keyframe() {
    // The pre-roll the edit list hides starts on the keyframe; the first
    // shown picture does not. Planning a copy from it would be a guess.
    let path = common::corpus("edit-list.mp4");
    let facts = facts_of(&path);
    let video = facts.video.as_ref().expect("video");
    assert!(video.keyframes[0].pts < 0, "{:?}", video.keyframes);
    let from_zero = tiers(&path, &facts, 0, video.end.expect("end"), false);
    assert!(
        matches!(from_zero[0].1, ExportTier::SmartCut { .. }),
        "{from_zero:?}"
    );
}
