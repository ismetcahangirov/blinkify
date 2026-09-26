//! The export report (#52): what an export did, measured on the file it
//! wrote — per segment, with its reason and what could have been done
//! instead; the bit-identical share as the verification suite measures it;
//! and what actually happened where that is not what the plan said.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::integer_division
)]

mod common;

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::ExportOutcome;
use blinkify_engine::export::plan::{ExportPlan, Media};
use blinkify_engine::export::queue::{
    ExportJob, ExportQueue, ExportSpec, Report, RunOutcome, Stage,
};
use blinkify_engine::export::report::{Execution, ExportReport, ReportRequest, build};
use blinkify_engine::export::verify::payload_hashes;
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::project::{Operation, Project, SequenceSettings};
use blinkify_engine::tier::ExportTier;
use common::fixture::Source;

fn report(
    source: &Source,
    plan: &ExportPlan,
    outcome: &ExportOutcome,
    lossless_audio_target: bool,
) -> ExportReport {
    build(
        &common::orchestrator(),
        &ReportRequest {
            name: "Trip",
            plan,
            outcome,
            inputs: &source.inputs(),
            lossless_audio_target,
            created: 1_790_458_800_000,
        },
    )
    .expect("measured")
}

/// The verification suite's measure: output packets whose payload is one of
/// the source's, by the hash boundary of #45.
fn identical_by_the_suite(source: &Path, output: &Path, selector: &str) -> (usize, usize) {
    let orchestrator = common::orchestrator();
    let known: HashSet<[u8; 32]> = payload_hashes(&orchestrator, source, selector)
        .expect("source")
        .into_iter()
        .map(|p| p.sha256)
        .collect();
    let written = payload_hashes(&orchestrator, output, selector).expect("output");
    let identical = written.iter().filter(|p| known.contains(&p.sha256)).count();
    (identical, written.len())
}

#[test]
fn a_copy_is_reported_fully_lossless_and_says_so_plainly() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(3),
        &[],
    )]);
    let target = common::scratch("report-copy").join("out.mp4");
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    let report = report(&source, &plan, &outcome, false);

    assert!(report.lossless);
    assert!((report.identical_percent - 100.0).abs() < 1e-9);
    assert!(report.summary.starts_with("Fully lossless"));
    // Every segment of the output is listed, copied, as planned.
    assert_eq!(report.segments.len(), plan.segments.len());
    for segment in &report.segments {
        assert_eq!(segment.execution, Execution::Copied, "{segment:?}");
        assert!(segment.as_planned);
        assert!(segment.packets > 0 && segment.identical == segment.packets);
        assert!(segment.reasons.is_empty() && segment.suggestions.is_empty());
        assert!(segment.encoder.is_none());
    }
    // The files are described.
    assert_eq!(report.output.name, "out.mp4");
    assert_eq!(
        report.output.size_bytes,
        std::fs::metadata(&target).expect("out").len()
    );
    assert!(report.output.codecs.contains(&"h264".to_owned()));
    assert_eq!(report.sources.len(), 1);
    // The measure is the verification suite's.
    let video = report.video.expect("video");
    let (identical, packets) = identical_by_the_suite(&source.path, &target, "v:0");
    assert_eq!(
        (video.identical as usize, video.packets as usize),
        (identical, packets)
    );
}

#[test]
fn a_smart_cut_names_its_encoder_its_reason_and_what_to_do_instead() {
    let source = Source::with_encoders("vp9-keyframes.webm");
    let frame = source.video().time_base.den / 30;
    let from = source.keyframe(1) + 12 * frame;
    let plan = source.plan_clips(vec![source.clip(1, 0, from, source.keyframe(3), &[])]);
    let video = plan
        .segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video");
    assert!(matches!(video.tier, ExportTier::SmartCut { .. }));
    let target = common::scratch("report-smartcut").join("out.mkv");
    let outcome = source
        .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    let report = report(&source, &plan, &outcome, false);

    assert!(!report.lossless);
    assert!(report.summary.starts_with("Not fully lossless"));
    let cut = report
        .segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video");
    assert_eq!(cut.execution, Execution::PartlyReEncoded);
    assert!(cut.as_planned);
    assert!(cut.identical > 0 && cut.identical < cut.packets);
    assert!(
        cut.encoder.as_deref().is_some_and(|e| e.contains("vp9")),
        "{:?}",
        cut.encoder
    );
    assert!(
        cut.reasons
            .iter()
            .any(|r| r.contains("0.40 s from the nearest keyframe")),
        "{:?}",
        cut.reasons
    );
    assert!(cut.suggestions.iter().any(|s| s.contains("Snap the cut")));
    assert!(
        report
            .suggestions
            .iter()
            .any(|s| s.contains("Snap the cut"))
    );
    // The bit-identical share is the verification suite's measurement.
    let totals = report.video.expect("video");
    let (identical, packets) = identical_by_the_suite(&source.path, &target, "v:0");
    assert_eq!(
        (totals.identical as usize, totals.packets as usize),
        (identical, packets)
    );
    assert!(totals.re_encoded_seconds > 0.0 && totals.copied_seconds > totals.re_encoded_seconds);
}

#[test]
fn what_was_executed_is_reported_where_it_differs_from_the_plan() {
    // The plan says copy; the file on disk was made otherwise. The report
    // measures the file, so it says what the file is.
    let source = Source::with_encoders("vp9-keyframes.webm");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let copy = source.plan_clips(vec![source.clip(1, 0, from, to, &[])]);
    let reversed = source.plan_clips(vec![source.clip(1, 0, from, to, &[Operation::Reverse])]);
    let target = common::scratch("report-diverged").join("out.mkv");
    let outcome = source
        .export(&reversed, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    let report = report(&source, &copy, &outcome, false);

    let video = report
        .segments
        .iter()
        .find(|s| s.media == Media::Video)
        .expect("video");
    assert_eq!(video.planned, ExportTier::StreamCopy);
    assert_eq!(video.execution, Execution::ReEncoded);
    assert!(!video.as_planned);
    assert!(
        video
            .reasons
            .iter()
            .any(|r| r
                .contains("The plan decided a copy, but the output shows every packet re-encoded")),
        "{:?}",
        video.reasons
    );
    assert!(!report.lossless);
    assert!(report.to_text(false).contains("planned as a copy"));
}

#[test]
fn the_text_names_no_folder_unless_asked() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(3),
        &[],
    )]);
    let dir = common::scratch("report-text");
    let target = dir.join("out.mp4");
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    let report = report(&source, &plan, &outcome, false);

    let shared = report.to_text(false);
    assert!(shared.contains("Fully lossless"));
    assert!(shared.contains("out.mp4") && shared.contains("h264-high-closed-gop.mp4"));
    assert!(shared.contains("Exported 2026-09-26 21:40 UTC"));
    let folders = [
        dir.display().to_string(),
        source.path.parent().expect("folder").display().to_string(),
    ];
    for folder in &folders {
        assert!(!shared.contains(folder.as_str()), "{folder} in:\n{shared}");
    }
    // No absolute path at all: no drive, no root, no commands.
    assert!(
        !shared.contains(":\\") && !shared.contains(":/"),
        "{shared}"
    );
    assert!(!shared.contains("Commands"));

    let full = report.to_text(true);
    assert!(full.contains(&target.display().to_string()));
    assert!(full.contains("Commands"));
}

#[test]
fn a_report_is_kept_with_the_history_across_a_restart() {
    let dir = common::scratch("report-retained");
    let store = dir.join("exports.json");
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(3),
        &[],
    )]);
    let target = dir.join("out.mp4");
    let outcome = source
        .export(&plan, &target, AudioTarget::default())
        .expect("exports");
    let measured = report(&source, &plan, &outcome, false);

    let queue = {
        let measured = measured.clone();
        ExportQueue::open(
            Some(store.clone()),
            move |_: &ExportSpec, _: &CancelToken, report: Report| {
                report(Stage::Verifying, 0.0);
                Ok(RunOutcome {
                    bytes: 1,
                    report: Some(measured.clone()),
                })
            },
            |_: &ExportJob| {},
        )
    };
    let job = queue
        .submit(ExportSpec {
            project: Project::new("Trip", SequenceSettings::default()),
            name: "Trip".to_owned(),
            target: dir.join("queued.mp4"),
            overwrite: false,
            audio: AudioTarget::default(),
        })
        .expect("queued");
    let finished = queue.wait(job.id, Duration::from_secs(60)).expect("job");
    assert!(finished.has_report);
    drop(queue);

    let relaunched = ExportQueue::open(
        Some(store),
        |_: &ExportSpec, _: &CancelToken, _: Report| panic!("nothing may run"),
        |_: &ExportJob| {},
    );
    assert_eq!(relaunched.report(job.id), Some(measured));
    assert!(relaunched.jobs()[0].has_report);
}
