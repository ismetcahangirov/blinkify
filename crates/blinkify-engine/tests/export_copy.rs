//! Keyframe-aligned lossless cutting by stream copy (#40), end to end: a plan
//! over a corpus file is exported, and the output's packets are compared with
//! the source's by payload hash — the bytes, not "it plays".

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::integer_division,
    clippy::cast_precision_loss
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportRequest, export, partial_path,
};
use blinkify_engine::export::facts::source_facts;
use blinkify_engine::export::plan::{ExportPlan, SourceFacts, plan};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Limits, Orchestrator};
use blinkify_engine::probe::{MediaInfo, Prober, StreamKind};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

struct Source {
    path: PathBuf,
    info: Arc<MediaInfo>,
    facts: SourceFacts,
}

fn source(name: &str) -> Source {
    let path = common::corpus(name);
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let index = KeyframeIndex::open(&path, &info, orchestrator, None).expect("index");
    index.complete_in_background(|_| {}).expect("indexed");
    let facts = source_facts(&info, Some(&index), None);
    Source { path, info, facts }
}

impl Source {
    fn keyframe(&self, i: usize) -> i64 {
        self.facts.video.as_ref().expect("video").keyframes[i].pts
    }

    fn end(&self) -> i64 {
        self.facts.video.as_ref().expect("video").end.expect("end")
    }

    /// A plan placing `ranges` of this file's video one after another.
    fn plan(&self, ranges: &[(i64, i64)]) -> ExportPlan {
        let video = self.facts.video.as_ref().expect("video");
        let mut project = Project::new(
            "t",
            SequenceSettings::matching(&video.geometry).expect("valid"),
        );
        project.sources.insert(
            1,
            SourceRef::of(&MediaAsset::new(self.path.clone()).export_source()).expect("ref"),
        );
        let mut clips = Vec::new();
        let mut start = 0;
        for (id, &(from, to)) in ranges.iter().enumerate() {
            let clip = Clip::new(
                u32::try_from(id + 1).expect("small"),
                1,
                video.stream,
                video.time_base,
                start,
                vec![Operation::Trim { from, to }],
            );
            project.sequence.tracks = vec![Track::new(1, TrackKind::Video, vec![clip.clone()])];
            start = evaluate(&project).expect("evaluates").length();
            clips.push(clip);
        }
        project.sequence.tracks = vec![Track::new(1, TrackKind::Video, clips)];
        let timeline = evaluate(&project).expect("evaluates");
        plan(
            &timeline,
            &project.sequence.settings,
            &BTreeMap::from([(1, self.facts.clone())]),
        )
        .expect("plans")
    }

    fn inputs(&self) -> BTreeMap<u32, ExportInput> {
        BTreeMap::from([(
            1,
            ExportInput {
                source: MediaAsset::new(self.path.clone()).export_source(),
                info: Arc::clone(&self.info),
            },
        )])
    }
}

fn run(plan: &ExportPlan, source: &Source, target: &Path) -> Result<Vec<u64>, ExportError> {
    let inputs = source.inputs();
    export(
        &orchestrator(),
        ExportRequest {
            plan,
            inputs: &inputs,
            target,
            overwrite: false,
            audio: AudioTarget::default(),
            models: None,
            cancel: CancelToken::default(),
            on_progress: None,
        },
    )
    .map(|outcome| outcome.packets)
}

/// The source's video packets a copy of `from..to` must contain, in decode
/// order: from the in-point's keyframe on, shown inside the range.
fn expected(source: &Source, from: i64, to: i64) -> Vec<String> {
    let all = common::packet_hashes(&source.path, "v:0");
    let start = all
        .iter()
        .position(|p| p.pts == from)
        .expect("the in-point is a packet");
    all[start..]
        .iter()
        .filter(|p| p.pts >= from && p.pts < to)
        .map(|p| p.md5.clone())
        .collect()
}

#[test]
fn a_keyframe_aligned_cut_copies_every_packet_byte_for_byte() {
    let source = source("h264-high-closed-gop.mp4");
    let before = std::fs::read(&source.path).expect("read");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan(&[(from, to)]);
    assert!(plan.summary.lossless, "{:#?}", plan.segments);
    let target = common::scratch("export-copy-single").join("cut.mp4");
    let packets = run(&plan, &source, &target).expect("exports");

    let output = common::packet_hashes(&target, "v:0");
    assert_eq!(common::md5s(&output), expected(&source, from, to));
    assert_eq!(packets[0], u64::try_from(output.len()).expect("fits"));
    // The output starts at zero, whatever the source's in-point was.
    assert_eq!(output.iter().map(|p| p.pts).min(), Some(0));
    // The sound is copied too: every output audio packet is a source packet.
    let source_audio = common::md5s(&common::packet_hashes(&source.path, "a:0"));
    let output_audio = common::md5s(&common::packet_hashes(&target, "a:0"));
    assert!(!output_audio.is_empty());
    let first = source_audio
        .iter()
        .position(|h| *h == output_audio[0])
        .expect("the first audio packet is a source packet");
    assert_eq!(
        output_audio,
        source_audio[first..first + output_audio.len()].to_vec()
    );
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
    // The source is read, never written.
    assert_eq!(std::fs::read(&source.path).expect("read"), before);
    assert!(!partial_path(&target).exists());
}

#[test]
fn segments_join_without_a_gap_or_an_overlap() {
    let source = source("h264-high-closed-gop.mp4");
    let ranges = [
        (source.keyframe(0), source.keyframe(1)),
        (source.keyframe(2), source.keyframe(4)),
    ];
    let plan = source.plan(&ranges);
    assert!(plan.summary.lossless);
    let target = common::scratch("export-copy-joined").join("joined.mp4");
    run(&plan, &source, &target).expect("exports");

    let output = common::packet_hashes(&target, "v:0");
    let mut wanted = expected(&source, ranges[0].0, ranges[0].1);
    wanted.extend(expected(&source, ranges[1].0, ranges[1].1));
    assert_eq!(common::md5s(&output), wanted);

    // Presentation times are the source's frame times, continuous across
    // the join: sorted, they step by exactly one frame everywhere.
    let mut times: Vec<i64> = output.iter().map(|p| p.pts).collect();
    times.sort_unstable();
    let steps: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();
    let frame = steps[0];
    assert!(frame > 0);
    assert!(steps.iter().all(|step| *step == frame), "{steps:?}");
    assert_eq!(times[0], 0);
    assert_eq!(common::decode_errors(&target), Vec::<String>::new());
}

fn video_of(path: &Path) -> blinkify_engine::probe::VideoInfo {
    let info = Prober::new(orchestrator()).probe(path).expect("probe");
    info.streams
        .iter()
        .find_map(|s| match &s.kind {
            StreamKind::Video(video) => Some((**video).clone()),
            _ => None,
        })
        .expect("video")
}

#[test]
fn rotation_and_colour_survive_the_copy() {
    let portrait = source("portrait-phone.mp4");
    let plan = portrait.plan(&[(portrait.keyframe(0).max(0), portrait.end())]);
    let target = common::scratch("export-copy-rotation").join("portrait.mp4");
    run(&plan, &portrait, &target).expect("exports");
    let (was, is) = (video_of(&portrait.path), video_of(&target));
    assert_eq!(is.rotation, was.rotation);
    assert_ne!(is.rotation, 0);
    assert_eq!(
        (is.display_width, is.display_height),
        (was.display_width, was.display_height)
    );

    let hdr = source("hevc-hdr10.mp4");
    let plan = hdr.plan(&[(hdr.keyframe(0).max(0), hdr.end())]);
    assert!(plan.summary.lossless);
    let target = common::scratch("export-copy-colour").join("hdr.mp4");
    run(&plan, &hdr, &target).expect("exports");
    let (was, is) = (video_of(&hdr.path), video_of(&target));
    assert_eq!(is.color, was.color);
    assert_eq!(is.color.primaries.as_deref(), Some("bt2020"));
    assert_eq!(is.hdr.is_some(), was.hdr.is_some());
    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        common::md5s(&common::packet_hashes(&hdr.path, "v:0"))
    );
}

#[test]
fn a_matroska_source_copies_with_its_chapters_and_language() {
    let source = source("multi-audio.mkv");
    let plan = source.plan(&[(source.keyframe(0).max(0), source.end())]);
    assert!(plan.summary.lossless, "{:#?}", plan.segments);
    let target = common::scratch("export-copy-mkv").join("copy.mkv");
    run(&plan, &source, &target).expect("exports");
    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        common::md5s(&common::packet_hashes(&source.path, "v:0"))
    );
    let info = Prober::new(orchestrator()).probe(&target).expect("probe");
    let titles: Vec<Option<String>> = info
        .container
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec![Some("Opening".to_owned()), Some("Closing".to_owned())]
    );
}

#[test]
fn the_target_is_never_overwritten_unasked_and_a_failure_leaves_nothing() {
    let source = source("h264-high-closed-gop.mp4");
    let plan = source.plan(&[(source.keyframe(1), source.keyframe(2))]);
    let dir = common::scratch("export-copy-failures");

    let existing = dir.join("existing.mp4");
    std::fs::write(&existing, b"the user's file").expect("write");
    assert!(matches!(
        run(&plan, &source, &existing),
        Err(ExportError::TargetExists(_))
    ));
    assert_eq!(std::fs::read(&existing).expect("read"), b"the user's file");

    let unwritable = dir.join("no such folder").join("out.mp4");
    assert!(run(&plan, &source, &unwritable).is_err());
    assert!(!unwritable.exists());
    assert!(!partial_path(&unwritable).exists());

    // A source that is gone is an error, and nothing is written.
    let missing = Source {
        path: dir.join("gone.mp4"),
        info: Arc::clone(&source.info),
        facts: source.facts.clone(),
    };
    std::fs::copy(&source.path, &missing.path).expect("copy");
    let gone_plan = missing.plan(&[(source.keyframe(1), source.keyframe(2))]);
    std::fs::remove_file(&missing.path).expect("remove");
    let target = dir.join("from-gone.mp4");
    assert!(run(&gone_plan, &missing, &target).is_err());
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());

    // Cancelled before it starts: no process runs to completion, nothing is
    // left behind.
    let cancelled = dir.join("cancelled.mp4");
    let cancel = CancelToken::default();
    cancel.cancel();
    let inputs = source.inputs();
    let result = export(
        &orchestrator(),
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &cancelled,
            overwrite: false,
            audio: AudioTarget::default(),
            models: None,
            cancel,
            on_progress: None,
        },
    );
    assert!(result.is_err());
    assert!(!cancelled.exists());
    assert!(!partial_path(&cancelled).exists());
}

#[test]
fn a_codec_the_container_cannot_hold_is_refused_before_anything_runs() {
    let source = source("h264-high-closed-gop.mp4");
    let plan = source.plan(&[(source.keyframe(1), source.keyframe(2))]);
    let target = common::scratch("export-copy-container").join("out.webm");
    assert!(matches!(
        run(&plan, &source, &target),
        Err(ExportError::CodecNotInContainer { .. })
    ));
    assert!(!target.exists());
}

#[test]
fn an_export_runs_however_few_shared_slots_the_machine_has() {
    // #110: a reader per stream and the muxer are coupled by pipes and must
    // all run at once. With one shared slot they used to wait on each other
    // for ever; an export's processes now have slots of their own.
    let source = source("h264-high-closed-gop.mp4");
    let plan = source.plan(&[(source.keyframe(0).max(0), source.keyframe(3))]);
    let target = common::scratch("export-copy-one-slot").join("cut.mp4");
    let starved = Orchestrator::new(
        common::sidecar(),
        Limits {
            interactive: 1,
            playback: 1,
            shared: 1,
            background: 0,
            export: 8,
        },
    );
    let (done, finished) = std::sync::mpsc::channel();
    let inputs = source.inputs();
    let worker_target = target.clone();
    std::thread::spawn(move || {
        let result = export(
            &starved,
            ExportRequest {
                plan: &plan,
                inputs: &inputs,
                target: &worker_target,
                overwrite: false,
                audio: AudioTarget::default(),
                models: None,
                cancel: CancelToken::default(),
                on_progress: None,
            },
        );
        let _ = done.send(result.map(|outcome| outcome.packets));
    });
    let result = finished
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the export finished rather than waiting on itself");
    assert!(result.is_ok(), "{result:?}");
    assert!(target.exists());
}
