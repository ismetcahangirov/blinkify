//! Clip gain with true-peak limiting (#46), end to end: what the export
//! writes is held under the ceiling, the pictures are copied untouched, and
//! the preview decodes through exactly the filters the export encodes with.
//!
//! The sources are made here with the sidecar, so each has the sound a case
//! needs — hot, clipped, silent, already limited — beside pictures that are
//! only ever copied.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blinkify_engine::audio::chain;
use blinkify_engine::audio::gain::{advise, measure_request};
use blinkify_engine::audio::loudness::{Loudness, LoudnessMeter, measure, measure_cached};
use blinkify_engine::cache::Cache;
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::playback::{PlaybackPlan, SourceMedia, audio_request};
use blinkify_engine::project::evaluate::{AudioOperation, evaluate};
use blinkify_engine::project::{
    AudioStage, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use common::fixture::Source;

const CEILING: f64 = -1.0;

/// Four seconds of pictures with a keyframe every second, and `sound` — a
/// lavfi audio graph — as 48 kHz stereo PCM, so the sound in the file is
/// exactly what the graph made.
fn source(name: &str, sound: &str) -> Source {
    let path = common::scratch(&format!("audio-gain-{name}")).join("source.mkv");
    let command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .lavfi_input("testsrc2=size=320x180:rate=30:duration=4")
        .lavfi_input(sound)
        .option("-map", "0:v")
        .option("-map", "1:a")
        .option("-pix_fmt", "yuv420p")
        .option("-g", "30")
        .option("-keyint_min", "30")
        .option("-c:a", "pcm_s16le")
        .option("-ar", "48000")
        .option("-ac", "2")
        .flag("-shortest");
    common::orchestrator()
        .run_to_end(command.output_file(&path), Priority::Foreground)
        .expect("made");
    Source::at(path, None)
}

/// Loud pink noise and a high tone, peaking around −8 dBFS.
const HOT: &str = "anoisesrc=d=4:c=pink:a=0.5:seed=7[n];sine=f=5000:d=4[s];[n][s]amix=inputs=2:normalize=0,aformat=channel_layouts=stereo";

/// The whole of `source` as one clip with `operations`, exported to FLAC in
/// Matroska.
fn export(source: &Source, operations: &[Operation], name: &str) -> PathBuf {
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.first(),
        source.end(),
        operations,
    )]);
    let target = common::scratch(&format!("audio-gain-out-{name}")).join("out.mkv");
    source
        .export(&plan, &target, AudioTarget::Flac)
        .expect("exports");
    target
}

/// The first audio stream of `path`, decoded to interleaved stereo `f32`.
fn decoded(path: &Path) -> Vec<f32> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:a:0")
                .option("-ac", "2")
                .option("-f", "f32le")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decodes");
    let bytes = bytes.lock().expect("lock");
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

fn loudness(samples: &[f32]) -> Loudness {
    let mut meter = LoudnessMeter::new(48_000, 2);
    meter.push(samples);
    meter.finish()
}

fn assert_pictures_copied(source: &Source, output: &Path) {
    assert_eq!(
        common::md5s(&common::packet_hashes(output, "v:0")),
        source.expected_video(source.first(), source.end()),
        "the pictures are the source's own packets"
    );
}

/// The longest run of consecutive samples in one channel at the same
/// magnitude, above half scale: what a hard clip leaves and a limiter does
/// not.
fn flat_top(samples: &[f32]) -> usize {
    let mut longest = 0;
    for channel in 0..2 {
        let mut run = 0;
        let mut previous = f32::NAN;
        for &sample in samples.iter().skip(channel).step_by(2) {
            if sample.abs() > 0.5 && (sample - previous).abs() < 1e-7 {
                run += 1;
                longest = longest.max(run + 1);
            } else {
                run = 0;
            }
            previous = sample;
        }
    }
    longest
}

#[test]
fn gain_past_full_scale_is_limited_under_the_ceiling_and_the_pictures_are_copied() {
    let source = source("hot", HOT);
    let before = loudness(&decoded(&source.path));
    let source_peak = before.true_peak_dbtp.expect("sound");
    // +18 dB puts the peaks well over 0 dBFS.
    assert!(source_peak + 18.0 > 6.0, "{source_peak}");
    let output = export(&source, &[Operation::gain(18.0)], "hot");
    let after = loudness(&decoded(&output));
    let peak = after.true_peak_dbtp.expect("sound");
    assert!(peak <= CEILING, "true peak {peak} dBTP over the ceiling");
    // Limited, not silenced: the gain still made it much louder.
    let gained = after.integrated_lufs.expect("loud") - before.integrated_lufs.expect("loud");
    assert!(gained > 8.0, "{gained}");
    assert_pictures_copied(&source, &output);
}

#[test]
fn extreme_gain_still_holds_the_ceiling() {
    let source = source("extreme", HOT);
    let output = export(&source, &[Operation::gain(40.0)], "extreme");
    let peak = loudness(&decoded(&output)).true_peak_dbtp.expect("sound");
    assert!(peak <= CEILING, "{peak}");
    // A lower ceiling of the user's is held too.
    let quieter = Operation::Gain {
        db: 40.0,
        ceiling_dbtp: -6.0,
        bypassed: false,
    };
    let output = export(&source, &[quieter], "extreme-6");
    let peak = loudness(&decoded(&output)).true_peak_dbtp.expect("sound");
    assert!(peak <= -6.0, "{peak}");
}

#[test]
fn a_clipped_input_is_limited_rather_than_clipped_again() {
    // A tone driven past full scale into 16-bit PCM: flat-topped in the file.
    let source = source(
        "clipped",
        "sine=f=440:d=4,volume=24dB,aformat=channel_layouts=stereo",
    );
    let input = decoded(&source.path);
    assert!(flat_top(&input) > 10, "the source is clipped");
    let output = export(&source, &[Operation::gain(3.0)], "clipped");
    let samples = decoded(&output);
    let peak = loudness(&samples).true_peak_dbtp.expect("sound");
    assert!(peak <= CEILING, "{peak}");
    // The limiter turned the whole waveform down; it did not shave a new
    // flat top at the ceiling.
    let ceiling = 10_f32.powf(CEILING as f32 / 20.0);
    let at_ceiling = samples
        .iter()
        .filter(|s| (s.abs() - ceiling).abs() < 1e-4)
        .count();
    assert!(at_ceiling < samples.len().div_euclid(100), "{at_ceiling}");
    assert_pictures_copied(&source, &output);
}

#[test]
fn silence_stays_silent_at_any_gain() {
    let source = source("silent", "anullsrc=r=48000:cl=stereo:d=4");
    let output = export(&source, &[Operation::gain(24.0)], "silent");
    let samples = decoded(&output);
    assert!(!samples.is_empty());
    assert!(samples.iter().all(|s| *s == 0.0));
    assert_pictures_copied(&source, &output);
}

#[test]
fn an_already_limited_input_turned_down_is_only_turned_down() {
    // A tone sitting just under the ceiling: a cut leaves the limiter idle,
    // and the output is the input, 6 dB down.
    let source = source(
        "limited",
        "sine=f=997:d=4,volume=-2dB,aformat=channel_layouts=stereo",
    );
    let before = loudness(&decoded(&source.path));
    let output = export(&source, &[Operation::gain(-6.0)], "limited");
    let after = loudness(&decoded(&output));
    let drop = before.integrated_lufs.expect("tone") - after.integrated_lufs.expect("tone");
    assert!((drop - 6.0).abs() < 0.05, "{drop}");
    let peak_drop = before.true_peak_dbtp.expect("tone") - after.true_peak_dbtp.expect("tone");
    assert!((peak_drop - 6.0).abs() < 0.05, "{peak_drop}");
}

#[test]
fn the_meter_agrees_with_ffmpegs_ebur128() {
    let source = source("meter", HOT);
    let ours = loudness(&decoded(&source.path));
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .flags(&["-nostats", "-hide_banner"])
                .input(&source.path)
                .option("-map", "0:a:0")
                .option("-af", "ebur128=peak=true:framelog=quiet")
                .output_null(),
            Priority::Foreground,
            JobOptions::default().on_stderr_line(move |line| {
                sink.lock().expect("lock").push(line.trim().to_owned());
            }),
        )
        .wait()
        .expect("measures");
    let lines = lines.lock().expect("lock");
    let value = |label: &str| -> f64 {
        lines
            .iter()
            .rev()
            .find_map(|line| line.strip_prefix(label))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("no {label} in {lines:?}"))
    };
    let integrated = value("I:");
    let range = value("LRA:");
    let peak = value("Peak:");
    assert!(
        (ours.integrated_lufs.expect("loud") - integrated).abs() < 0.1,
        "{ours:?} vs I {integrated}"
    );
    assert!(
        (ours.range_lu.expect("range") - range).abs() < 0.5,
        "{ours:?} vs LRA {range}"
    );
    assert!(
        (ours.true_peak_dbtp.expect("peak") - peak).abs() < 0.2,
        "{ours:?} vs peak {peak}"
    );
}

/// A project of `source` with one clip carrying `operations`, evaluated, and
/// the preview's plan of it.
fn preview_of(source: &Source, operations: &[Operation]) -> (Project, PlaybackPlan) {
    let mut project = Project::new(
        "t",
        SequenceSettings::matching(&source.video().geometry).expect("valid"),
    );
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(source.path.clone()).export_source()).expect("ref"),
    );
    project.sequence.tracks = vec![Track::new(
        1,
        TrackKind::Video,
        vec![source.clip(1, 0, source.first(), source.end(), operations)],
    )];
    let timeline = evaluate(&project).expect("evaluates");
    let index = KeyframeIndex::open(&source.path, &source.info, common::orchestrator(), None)
        .expect("index");
    let media = SourceMedia::new(&source.path, Arc::clone(&source.info), Arc::new(index))
        .expect("playable");
    let plan = PlaybackPlan::from_timeline(&timeline, &BTreeMap::from([(1, Arc::new(media))]))
        .expect("plan");
    (project, plan)
}

#[test]
fn the_preview_decodes_through_exactly_the_filters_the_export_encodes_with() {
    let source = source("same-chain", HOT);
    let operations = [Operation::gain(7.5)];
    let (_, plan) = preview_of(&source, &operations);
    let segment = plan.segments().first().expect("a segment");
    let chain = chain::filters(&segment.audio, 48_000);
    assert!(chain.starts_with("volume=7.5dB,"), "{chain}");

    let preview = audio_request(segment, segment.timeline_start, 48_000, 1.0)
        .expect("sound")
        .command()
        .to_string();
    let target = common::scratch("audio-gain-out-same-chain").join("out.mkv");
    let outcome = source
        .export(
            &source.plan_clips(vec![source.clip(
                1,
                0,
                source.first(),
                source.end(),
                &operations,
            )]),
            &target,
            AudioTarget::Flac,
        )
        .expect("exports");
    let encoder = outcome
        .commands
        .iter()
        .find(|command| command.contains("volume="))
        .expect("an encoder");
    // Both run the chain whole, straight after resampling to their rate.
    assert!(
        preview.contains(&format!("aresample=48000,{chain},")),
        "{preview}"
    );
    assert!(
        encoder.contains(&format!("aresample=48000,{chain},")),
        "{encoder}"
    );

    // Bypassed, the chain is gone from both.
    let bypassed = [Operation::Gain {
        db: 7.5,
        ceiling_dbtp: CEILING,
        bypassed: true,
    }];
    let (_, plan) = preview_of(&source, &bypassed);
    let segment = plan.segments().first().expect("a segment");
    let preview = audio_request(segment, segment.timeline_start, 48_000, 1.0)
        .expect("sound")
        .command()
        .to_string();
    assert!(!preview.contains("volume="), "{preview}");
    assert!(!preview.contains("alimiter"), "{preview}");
}

#[test]
fn the_advice_predicts_the_limiting_the_export_does() {
    let source = source("advice", HOT);
    let (project, _) = preview_of(&source, &[Operation::gain(12.0)]);
    let timeline = evaluate(&project).expect("evaluates");
    let placement = timeline.placements().next().expect("clip");
    let request =
        measure_request(placement, &source.path, &source.info, AudioStage::Gain).expect("sound");
    // Measured before the gain: nothing of the chain runs before it here.
    assert!(request.filters.is_empty());
    let before =
        measure(&common::orchestrator(), &request, &CancelToken::default()).expect("measures");
    let advice = advise(before, &placement.audio);
    assert!((advice.gain_db - 12.0).abs() < f64::EPSILON);
    let expected = before.true_peak_dbtp.expect("peak") + 12.0 - CEILING;
    assert!((advice.limiting_db - expected).abs() < 1e-9);
    assert!(advice.limiting_db > 3.0, "{advice:?}");
    // The suggestion aims at −16 LUFS and says what it would cost.
    let suggested = advice.suggested_db.expect("measurable");
    assert!(
        (before.integrated_lufs.expect("loud") + suggested - -16.0).abs() < 0.06,
        "{advice:?}"
    );
    let _: &[AudioOperation] = &placement.audio;
}

#[test]
fn a_measurement_is_cached_by_content_and_by_the_filters_before_it() {
    let source = source("cached", HOT);
    let cache_dir = common::scratch("audio-gain-cache");
    let cache = Cache::new(cache_dir.clone(), 1 << 20);
    let (project, _) = preview_of(&source, &[Operation::gain(3.0)]);
    let timeline = evaluate(&project).expect("evaluates");
    let placement = timeline.placements().next().expect("clip");
    let request =
        measure_request(placement, &source.path, &source.info, AudioStage::Gain).expect("sound");
    let orchestrator = common::orchestrator();
    let cancel = CancelToken::default();
    let first = measure_cached(&orchestrator, Some(&cache), &request, &cancel).expect("first");
    let entries = || std::fs::read_dir(cache_dir.join("loudness")).map_or(0, Iterator::count);
    assert_eq!(entries(), 1);
    let again = measure_cached(&orchestrator, Some(&cache), &request, &cancel).expect("again");
    assert_eq!(first, again);
    assert_eq!(entries(), 1, "answered from the cache");
    // A different chain before the point measured is a different answer.
    let later = measure_request(placement, &source.path, &source.info, AudioStage::Normalise)
        .expect("sound");
    assert_ne!(later.filters, request.filters);
    let after_gain = measure_cached(&orchestrator, Some(&cache), &later, &cancel).expect("later");
    assert_eq!(entries(), 2);
    assert!(after_gain.integrated_lufs > first.integrated_lufs);
}
