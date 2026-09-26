//! Two-pass loudness normalisation (#48), end to end: the export lands within
//! half a LU of the target with its true peak under the ceiling, a sequence
//! normalised as a whole keeps the levels between its clips, one gain for a
//! clip does not pump where single-pass `loudnorm` does, and a measurement
//! is taken again when what comes before it in the chain changes.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blinkify_engine::audio::chain::ChainError;
use blinkify_engine::audio::loudness::{Loudness, LoudnessMeter};
use blinkify_engine::cache::Cache;
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{ExportError, ExportRequest, export};
use blinkify_engine::export::loudness::Resolver;
use blinkify_engine::export::plan::plan;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::project::evaluate::{AudioOperation, Timeline, evaluate};
use blinkify_engine::project::{
    Clip, LoudnessTarget, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use common::fixture::Source;

/// Pictures beside `sound` — a lavfi graph, or a corpus file — at 48 kHz
/// with `channels` channels, as 16-bit PCM.
fn source(name: &str, sound: &str, channels: u32, seconds: u32) -> Source {
    let path = common::scratch(&format!("audio-normalise-{name}")).join("source.mkv");
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .lavfi_input(&format!("testsrc2=size=320x180:rate=30:duration={seconds}"));
    command = if Path::new(sound)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("flac"))
    {
        command.input(&common::corpus(sound))
    } else {
        command.lavfi_input(sound)
    };
    let command = command
        .option("-map", "0:v")
        .option("-map", "1:a")
        .option("-pix_fmt", "yuv420p")
        .option("-g", "30")
        .option("-keyint_min", "30")
        .option("-c:a", "pcm_s16le")
        .option("-ar", "48000")
        .option("-ac", channels.to_string())
        .flag("-shortest");
    common::orchestrator()
        .run_to_end(command.output_file(&path), Priority::Foreground)
        .expect("made");
    Source::at(path, None)
}

fn project(source: &Source, clips: Vec<Clip>, loudness: Option<LoudnessTarget>) -> Project {
    let mut project = Project::new(
        "t",
        SequenceSettings::matching(&source.video().geometry).expect("valid"),
    );
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(source.path.clone()).export_source()).expect("ref"),
    );
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, clips)];
    project.sequence.loudness = loudness;
    project
}

fn resolver<'a>(
    inputs: &'a BTreeMap<u32, blinkify_engine::export::execute::ExportInput>,
    cache: Option<&'a Cache>,
    orchestrator: &'a blinkify_engine::orchestrator::Orchestrator,
    cancel: &'a CancelToken,
) -> Resolver<'a> {
    Resolver {
        orchestrator,
        cache,
        inputs,
        models: None,
        cancel,
        only_cached: false,
    }
}

/// Resolve `project`'s loudness, plan it, and export it to FLAC.
fn export_resolved(source: &Source, project: &Project, name: &str) -> (PathBuf, Timeline) {
    let orchestrator = common::orchestrator();
    let cancel = CancelToken::default();
    let inputs = source.inputs();
    let facts = BTreeMap::from([(1, source.facts.clone())]);
    let timeline = evaluate(project).expect("evaluates");
    let resolved = resolver(&inputs, None, &orchestrator, &cancel)
        .resolve(&timeline, &project.sequence.settings, &facts)
        .expect("resolved");
    let plan = plan(&resolved, &project.sequence.settings, &facts).expect("plans");
    let target = common::scratch(&format!("audio-normalise-out-{name}")).join("out.mkv");
    export(
        &orchestrator,
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &target,
            overwrite: false,
            audio: AudioTarget::Flac,
            models: None,
            cancel,
            on_progress: None,
        },
    )
    .expect("exports");
    (target, resolved)
}

/// The first audio stream of `path`, decoded as `channels` channels.
fn decoded(path: &Path, channels: u32) -> Vec<f32> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:a:0")
                .option("-ac", channels.to_string())
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

fn measure(samples: &[f32], channels: u32) -> Loudness {
    let mut meter = LoudnessMeter::new(48_000, channels as usize);
    meter.push(samples);
    meter.finish()
}

fn whole(source: &Source, operations: &[Operation]) -> Clip {
    source.clip(1, 0, source.first(), source.end(), operations)
}

const HOT: &str =
    "anoisesrc=d=8:c=pink:a=0.5:seed=7[n];sine=f=5000:d=8[s];[n][s]amix=inputs=2:normalize=0";

#[test]
fn every_reference_lands_within_half_a_lu_of_its_target_under_the_ceiling() {
    let references = [
        ("speech", "speech-clean.flac", 1, 20),
        ("noisy", "speech-noisy.flac", 1, 20),
        ("hot", HOT, 2, 8),
        ("tone", "sine=f=997:d=8,volume=-30dB", 2, 8),
    ];
    for (name, sound, channels, seconds) in references {
        let source = source(name, sound, channels, seconds);
        for target in [-14.0, -23.0] {
            let normalise = Operation::Normalise {
                target_lufs: target,
                ceiling_dbtp: -1.0,
                bypassed: false,
            };
            let project = project(&source, vec![whole(&source, &[normalise])], None);
            let (output, _) = export_resolved(&source, &project, &format!("{name}{target}"));
            let after = measure(&decoded(&output, channels), channels);
            let lufs = after.integrated_lufs.expect("loud");
            assert!(
                (lufs - target).abs() <= 0.5,
                "{name}: {lufs} LUFS for a target of {target}"
            );
            let peak = after.true_peak_dbtp.expect("peak");
            assert!(peak <= -1.0, "{name}: true peak {peak} dBTP");
            if target == -14.0 && name == "speech" {
                assert_eq!(
                    common::md5s(&common::packet_hashes(&output, "v:0")),
                    source.expected_video(source.first(), source.end()),
                    "the pictures are the source's own packets"
                );
            }
        }
    }
}

#[test]
fn a_sequence_normalised_as_a_whole_keeps_the_levels_between_its_clips() {
    // The same reading twice, the first half 6 dB down: after normalising
    // the sequence, the first is still 6 dB below the second. The target is
    // one the peaks allow, so the limiter — which takes more from a louder
    // clip — stays out of the comparison: what is tested is the gain.
    let source = source("sequence", "speech-clean.flac", 1, 20);
    let tb = source.video().time_base;
    let ten = (10 * tb.den.max(1)).div_euclid(tb.num.max(1));
    let first = source.clip(
        1,
        0,
        source.first(),
        source.first() + ten,
        &[Operation::gain(-6.0)],
    );
    let second = source.clip(2, 300, source.first() + ten, source.end(), &[]);
    let target = LoudnessTarget {
        target_lufs: -23.0,
        ceiling_dbtp: -1.0,
    };
    let project = project(&source, vec![first, second], Some(target));
    let before = decoded(&source.path, 1);
    let (output, resolved) = export_resolved(&source, &project, "sequence");
    let after = decoded(&output, 1);
    let halves = |samples: &[f32]| {
        let middle = 480_000.min(samples.len());
        (
            measure(&samples[..middle], 1)
                .integrated_lufs
                .expect("loud"),
            measure(&samples[middle..], 1)
                .integrated_lufs
                .expect("loud"),
        )
    };
    let (a, b) = halves(&before);
    let (x, y) = halves(&after);
    let gap_before = (b - a) + 6.0; // the first half was turned down 6 dB
    let gap_after = y - x;
    assert!(
        (gap_after - gap_before).abs() < 0.2,
        "{gap_before} LU apart before, {gap_after} after"
    );
    let overall = measure(&after, 1).integrated_lufs.expect("loud");
    assert!((overall - -23.0).abs() <= 0.5, "{overall}");
    // One gain, the same on both clips.
    let masters: Vec<Option<f64>> = resolved
        .placements()
        .map(|p| {
            p.audio.iter().rev().find_map(|step| match step {
                AudioOperation::Normalise { gain_db, .. } => Some(*gain_db),
                _ => None,
            })
        })
        .map(Option::flatten)
        .collect();
    assert_eq!(masters.len(), 2);
    assert!(
        masters[0].is_some() && masters[0] == masters[1],
        "{masters:?}"
    );
}

/// The level of each whole second of `samples`, in dBFS, leaving out the
/// fifth of a second either side of each change.
fn second_levels(samples: &[f32]) -> Vec<f64> {
    samples
        .chunks(48_000)
        .filter(|chunk| chunk.len() == 48_000)
        .map(|chunk| {
            let inner = &chunk[9_600..38_400];
            let mean =
                inner.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / inner.len() as f64;
            10.0 * mean.log10()
        })
        .collect()
}

#[test]
fn one_gain_does_not_pump_where_single_pass_loudnorm_does() {
    // A tone that steps between −30 and −10 dBFS every second: speech with
    // quiet and loud phrases, reduced to what a normaliser reacts to.
    let stepped = "aevalsrc='0.3162*if(mod(floor(t),2),1,0.1)*sin(2*PI*440*t)':s=48000:d=12";
    let source = source("pumping", stepped, 1, 12);
    let input = second_levels(&decoded(&source.path, 1));
    let normalise = Operation::normalise(-16.0);
    let project = project(&source, vec![whole(&source, &[normalise])], None);
    let (output, _) = export_resolved(&source, &project, "pumping");
    let ours = second_levels(&decoded(&output, 1));
    let gains: Vec<f64> = ours.iter().zip(&input).map(|(o, i)| o - i).collect();
    let spread = |g: &[f64]| {
        g.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - g.iter().copied().fold(f64::INFINITY, f64::min)
    };
    assert!(spread(&gains) < 0.3, "the gain moved: {gains:?}");

    // FFmpeg's single-pass loudnorm on the same sound rides the level.
    let dynamic = common::scratch("audio-normalise-loudnorm").join("dynamic.wav");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(&source.path)
                .option("-map", "0:a:0")
                .option("-af", "loudnorm=I=-16:TP=-1:LRA=7,aresample=48000")
                .option("-c:a", "pcm_f32le")
                .output_file(&dynamic),
            Priority::Foreground,
        )
        .expect("loudnorm");
    let theirs = second_levels(&decoded(&dynamic, 1));
    let their_gains: Vec<f64> = theirs.iter().zip(&input).map(|(o, i)| o - i).collect();
    assert!(
        spread(&their_gains) > 3.0,
        "single-pass loudnorm did not pump here: {their_gains:?}"
    );
}

#[test]
fn a_measurement_is_taken_again_when_the_chain_before_it_changes() {
    let source = source("invalidate", "speech-clean.flac", 1, 20);
    let orchestrator = common::orchestrator();
    let cancel = CancelToken::default();
    let inputs = source.inputs();
    let facts = BTreeMap::from([(1, source.facts.clone())]);
    let dir = common::scratch("audio-normalise-cache");
    let cache = Cache::new(dir.clone(), 1 << 20);
    let resolve = |gain: f64| -> f64 {
        let mut operations = vec![Operation::normalise(-23.0)];
        if gain != 0.0 {
            operations.insert(0, Operation::gain(gain));
        }
        let project = project(&source, vec![whole(&source, &operations)], None);
        let timeline = evaluate(&project).expect("evaluates");
        let resolved = resolver(&inputs, Some(&cache), &orchestrator, &cancel)
            .resolve(&timeline, &project.sequence.settings, &facts)
            .expect("resolved");
        resolved
            .placements()
            .flat_map(|p| p.audio.iter())
            .find_map(|step| match step {
                AudioOperation::Normalise { gain_db, .. } => *gain_db,
                _ => None,
            })
            .expect("resolved gain")
    };
    let entries = || std::fs::read_dir(dir.join("loudness")).map_or(0, Iterator::count);
    let plain = resolve(0.0);
    let measured = entries();
    assert!(measured >= 1);
    assert_eq!(resolve(0.0), plain);
    assert_eq!(
        entries(),
        measured,
        "the same chain is answered from the cache"
    );
    // Six dB taken off before it: measured again, and six dB more to add.
    let quieter = resolve(-6.0);
    assert!(entries() > measured, "a changed chain is a new measurement");
    assert!(
        (quieter - plain - 6.0).abs() < 0.25,
        "{plain} then {quieter}"
    );
}

#[test]
fn near_silence_is_left_where_it_is_rather_than_raised_by_sixty_decibels() {
    // About −75 dBFS: below R128's −70 LUFS gate, above 16-bit silence.
    let source = source("silence", "anoisesrc=d=4:c=white:a=0.0003:seed=3", 2, 4);
    let project = project(
        &source,
        vec![whole(&source, &[Operation::normalise(-14.0)])],
        None,
    );
    let before = decoded(&source.path, 2);
    let (output, resolved) = export_resolved(&source, &project, "silence");
    let gain = resolved
        .placements()
        .flat_map(|p| p.audio.iter())
        .find_map(|step| match step {
            AudioOperation::Normalise { gain_db, .. } => *gain_db,
            _ => None,
        });
    assert_eq!(gain, Some(0.0));
    let after = decoded(&output, 2);
    let rms = |s: &[f32]| {
        10.0 * (s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / s.len() as f64).log10()
    };
    // Still near silence: not brought up towards −14 LUFS.
    assert!(
        rms(&after) < rms(&before) + 1.0,
        "{} then {}",
        rms(&before),
        rms(&after)
    );
    assert!(rms(&after) < -70.0, "{}", rms(&after));
}

#[test]
fn an_unmeasured_normalisation_is_refused_before_anything_is_written() {
    let source = source("unmeasured", HOT, 2, 8);
    let plan = source.plan_clips(vec![whole(&source, &[Operation::normalise(-16.0)])]);
    let target = common::scratch("audio-normalise-out-unmeasured").join("out.mkv");
    let refused = source.export(&plan, &target, AudioTarget::Flac);
    assert!(
        matches!(
            refused,
            Err(ExportError::AudioChain(ChainError::NotMeasured))
        ),
        "{refused:?}"
    );
    assert!(!target.exists());
}
