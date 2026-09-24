//! The keyframe indicator against the corpus (#35): for every frame of a
//! clip, a cut is called lossless exactly where the frame on screen is a
//! clean keyframe, and the keyframes it names bracket the position.

#![allow(clippy::expect_used, clippy::panic, clippy::integer_division)]

mod common;

use std::collections::BTreeMap;

use blinkify_engine::keyframes::{GopKind, KeyframeIndex};
use blinkify_engine::orchestrator::{Limits, Orchestrator};
use blinkify_engine::probe::Prober;
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::split::cut_point;
use blinkify_engine::project::{Clip, Operation, Project, SequenceSettings, Track, TrackKind};
use blinkify_engine::proxy::MediaAsset;

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

/// Check every frame of `name`'s first seconds, placed at its own rate.
fn check(name: &str, seconds: i64) -> usize {
    let orchestrator = orchestrator();
    let path = common::corpus(name);
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let (stream, video) = info.video().next().expect("pictures");
    let time_base = stream.time_base.expect("time base");
    let rate = video
        .frame_rate
        .real_base
        .or(video.frame_rate.average)
        .expect("rate");
    let index = KeyframeIndex::open(&path, &info, orchestrator, None).expect("index");
    let first = index
        .frame_after(stream.index, i64::MIN)
        .expect("frames")
        .expect("a first frame");
    let ticks = seconds * time_base.den / time_base.num;

    let mut project = Project::new(
        "cut points",
        SequenceSettings {
            frame_rate: rate,
            ..SequenceSettings::default()
        },
    );
    let source = project
        .add_source(&MediaAsset::new(path.clone()).export_source())
        .expect("source");
    project.sequence.tracks.push(Track {
        id: 1,
        kind: TrackKind::Video,
        clips: vec![Clip::new(
            1,
            source,
            stream.index,
            time_base,
            0,
            vec![Operation::Trim {
                from: first,
                to: first + ticks,
            }],
        )],
        ..Track::default()
    });
    let timeline = evaluate(&project).expect("evaluates");
    let placement = timeline.placements().next().expect("placed").clone();

    let keyframes: BTreeMap<i64, GopKind> = index
        .keyframes(stream.index)
        .expect("keyframes")
        .into_iter()
        .map(|keyframe| (keyframe.pts, keyframe.gop))
        .collect();
    let mut lossless = 0;
    for position in placement.start..placement.end() {
        let tick = placement.source_at(position).expect("on screen");
        let shown = index
            .frame_at_or_before(stream.index, tick)
            .expect("frames")
            .unwrap_or(tick);
        let before = index.at_or_before(stream.index, shown).expect("index");
        let after = index.at_or_after(stream.index, shown + 1).expect("index");
        let cut = cut_point(&placement, position, shown, before, after);
        let clean = before
            .is_some_and(|k| k.pts == shown && k.gop == GopKind::Closed && !k.has_leading_pictures);
        assert_eq!(cut.lossless, clean, "{name} at frame {position}");
        assert_eq!(
            cut.lossless || cut.open_gop,
            keyframes.contains_key(&shown),
            "{name} at frame {position}"
        );
        if let Some(previous) = cut.previous {
            assert!(previous <= position, "{name}: {previous} after {position}");
        }
        if let Some(next) = cut.next {
            assert!(next > position, "{name}: {next} not after {position}");
        }
        lossless += usize::from(cut.lossless);
    }
    lossless
}

#[test]
fn the_indicator_agrees_with_the_keyframe_index_frame_for_frame() {
    // Closed GOPs: some frames are clean cuts, most are not.
    let clean = check("h264-high-closed-gop.mp4", 4);
    assert!(clean > 0, "no lossless cut found in a closed-GOP source");
    // Open GOPs are named as such, never called lossless when they are not.
    check("h264-open-gop.mp4", 4);
    check("hevc-open-gop.mp4", 4);
    // A phone's variable frame rate, and a portrait one.
    check("vfr-screen.mp4", 3);
    check("portrait-phone.mp4", 3);
}

#[test]
fn a_long_gop_source_has_few_lossless_cuts() {
    let frames = 4 * 30;
    let clean = check("h264-high-closed-gop.mp4", 4);
    assert!(
        clean * 4 < frames,
        "{clean} clean cuts in {frames} frames is not a long-GOP source"
    );
}
