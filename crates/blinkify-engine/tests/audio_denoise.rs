//! Noise reduction (#47), end to end: `RNNoise` through the bundled model makes
//! a noisy recording measurably cleaner, strength blends in the filter graph,
//! the pictures are copied untouched, a missing model is refused by name, and
//! switching the chain while playing — an A/B comparison — leaves no gap.
//!
//! The speech is the corpus's public-domain reading (`speech-clean.flac`)
//! and the same reading under pink noise (`speech-noisy.flac`), so how much
//! noise came out can be measured against what the speech was.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::similar_names
)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::audio::chain::ChainError;
use blinkify_engine::audio::denoise::{MODEL_FILE, Models};
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{ExportError, ExportRequest, export};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, Player, PlayerOptions, Segment, SourceMedia,
    TransportCommand,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::project::Operation;
use blinkify_engine::project::evaluate::AudioOperation;
use blinkify_engine::time::{self, Rounding};
use common::fixture::Source;

fn models() -> Models {
    Models::in_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("models/rnnoise"))
        .expect("the bundled model")
}

/// Pictures beside the corpus recording `sound`, as 48 kHz mono PCM: the
/// recording's own samples, so the export can be compared with it.
fn source(name: &str, sound: &str) -> Source {
    let path = common::scratch(&format!("audio-denoise-{name}")).join("source.mkv");
    let command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .lavfi_input("testsrc2=size=320x180:rate=30:duration=20")
        .input(&common::corpus(sound))
        .option("-map", "0:v")
        .option("-map", "1:a")
        .option("-pix_fmt", "yuv420p")
        .option("-g", "30")
        .option("-keyint_min", "30")
        .option("-c:a", "pcm_s16le")
        .flag("-shortest");
    common::orchestrator()
        .run_to_end(command.output_file(&path), Priority::Foreground)
        .expect("made");
    Source::at(path, None)
}

fn export_with(
    source: &Source,
    operations: &[Operation],
    models: Option<&Models>,
    target: &Path,
) -> Result<(), ExportError> {
    let plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.first(),
        source.end(),
        operations,
    )]);
    let inputs = source.inputs();
    export(
        &common::orchestrator(),
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target,
            overwrite: false,
            audio: AudioTarget::Flac,
            models,
            cancel: CancelToken::default(),
            on_progress: None,
        },
    )
    .map(|_| ())
}

/// The first audio stream of `path`, decoded to mono `f32`.
fn decoded(path: &Path) -> Vec<f32> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:a:0")
                .option("-ac", "1")
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

/// Scale-invariant signal-to-noise ratio of `output` against `clean`, in dB:
/// how much of the output is the speech, whatever its level.
fn si_snr(output: &[f32], clean: &[f32]) -> f64 {
    let n = output.len().min(clean.len());
    let dot: f64 = (0..n)
        .map(|i| f64::from(output[i]) * f64::from(clean[i]))
        .sum();
    let energy: f64 = clean[..n].iter().map(|c| f64::from(*c).powi(2)).sum();
    let scale = dot / energy;
    let (mut target, mut error) = (0.0, 0.0);
    for i in 0..n {
        let wanted = scale * f64::from(clean[i]);
        target += wanted * wanted;
        error += (f64::from(output[i]) - wanted).powi(2);
    }
    10.0 * (target / error).log10()
}

#[test]
fn noise_is_measurably_reduced_on_a_noisy_speech_recording_and_the_pictures_are_copied() {
    let source = source("snr", "speech-noisy.flac");
    let clean = decoded(&common::corpus("speech-clean.flac"));
    let noisy = decoded(&source.path);
    let before = si_snr(&noisy, &clean);

    let dir = common::scratch("audio-denoise-snr-out");
    let full = dir.join("full.mkv");
    export_with(&source, &[Operation::denoise(1.0)], Some(&models()), &full).expect("exports");
    let after = si_snr(&decoded(&full), &clean);
    assert!(
        after - before > 2.5,
        "SI-SNR {before:.2} dB before, {after:.2} dB after"
    );
    assert_eq!(
        common::md5s(&common::packet_hashes(&full, "v:0")),
        source.expected_video(source.first(), source.end()),
        "the pictures are the source's own packets"
    );
    // The sound stays on its samples: nothing delayed, nothing lost.
    assert_eq!(decoded(&full).len(), noisy.len());
}

#[test]
fn strength_blends_and_at_zero_the_sound_is_the_source_bit_for_bit() {
    let source = source("strength", "speech-noisy.flac");
    let noisy = decoded(&source.path);
    let clean = decoded(&common::corpus("speech-clean.flac"));
    let dir = common::scratch("audio-denoise-strength-out");
    // A step at 0, bypassing nothing, as a graph can hold it.
    let zero = dir.join("zero.mkv");
    export_with(&source, &[Operation::denoise(0.0)], Some(&models()), &zero).expect("exports");
    assert_eq!(decoded(&zero), noisy, "strength 0 is the input, exactly");
    // Half strength is between the two.
    let half = dir.join("half.mkv");
    export_with(&source, &[Operation::denoise(0.5)], Some(&models()), &half).expect("exports");
    let half = decoded(&half);
    let (a, b) = (si_snr(&noisy, &clean), si_snr(&half, &clean));
    assert!(b > a, "{a} then {b}");
    assert_ne!(half, noisy);
}

#[test]
fn the_model_is_resolved_from_where_it_was_installed_whatever_that_path_is_called() {
    // An install directory with a space and a quote in its name: the path
    // reaches the filter graph as itself and nothing more.
    let installed = common::scratch("audio-denoise-install").join("Program Files (it's here)");
    std::fs::create_dir_all(&installed).expect("dir");
    std::fs::copy(models().rnnoise(), installed.join(MODEL_FILE)).expect("copy");
    let models = Models::in_dir(&installed).expect("installed");
    let source = source("installed", "speech-noisy.flac");
    let target = common::scratch("audio-denoise-install-out").join("out.mkv");
    export_with(&source, &[Operation::denoise(0.8)], Some(&models), &target).expect("exports");
    assert!(common::decode_errors(&target).is_empty());
}

#[test]
fn a_missing_model_is_refused_by_name_before_anything_is_written() {
    let source = source("missing", "speech-noisy.flac");
    let target = common::scratch("audio-denoise-missing-out").join("out.mkv");
    let refused = export_with(&source, &[Operation::denoise(0.5)], None, &target);
    assert!(
        matches!(refused, Err(ExportError::AudioChain(ChainError::NoModel))),
        "{refused:?}"
    );
    assert!(refused.expect_err("refused").to_string().contains("model"));
    assert!(!target.exists());
    // A bypassed noise reduction needs no model, and the sound is copied.
    let bypassed = Operation::Denoise {
        strength: 0.5,
        bypassed: true,
    };
    export_with(&source, &[bypassed], None, &target).expect("exports");
}

/// The first `seconds` of `path` as a one-segment plan, with `audio` as its
/// chain.
fn plan(media: &Arc<SourceMedia>, seconds: f64, audio: Vec<AudioOperation>) -> PlaybackPlan {
    let tb = media.time_base();
    let mut segment = Segment::new(
        Arc::clone(media),
        0,
        time::from_seconds(seconds, tb, Rounding::Nearest),
        0,
    );
    segment.audio = audio;
    PlaybackPlan::new(vec![segment]).expect("plan")
}

/// Root-mean-square level of each 10 ms of `samples`, in dBFS.
fn levels(samples: &[f32]) -> Vec<f64> {
    samples
        .chunks(960)
        .map(|chunk| {
            let mean =
                chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / chunk.len() as f64;
            10.0 * mean.max(1e-20).log10()
        })
        .collect()
}

#[test]
fn changing_the_chain_while_playing_switches_without_a_gap() {
    let orchestrator = common::orchestrator();
    let path: PathBuf = common::corpus("h264-high-closed-gop.mp4");
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let index =
        Arc::new(KeyframeIndex::open(&path, &info, orchestrator.clone(), None).expect("index"));
    let media = Arc::new(SourceMedia::new(&path, info, index).expect("playable"));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let player = Player::new(
        orchestrator,
        plan(&media, 4.0, Vec::new()),
        &PlayerOptions {
            audio: AudioChoice::Capture {
                sample_rate: 48_000,
                samples: Arc::clone(&captured),
            },
            max_width: 160,
            max_height: 90,
            default_device: DefaultDevice::System,
            models: Some(models()),
        },
    );
    player.command(TransportCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(10);
    while captured
        .lock()
        .expect("lock")
        .iter()
        .all(|s| s.abs() < 1e-6)
    {
        assert!(Instant::now() < deadline, "nothing played");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(800));
    // The A/B of an audio step: the same segment, only its chain changed.
    player.set_plan(plan(
        &media,
        4.0,
        vec![AudioOperation::Gain {
            db: -12.0,
            ceiling_dbtp: -1.0,
            bypassed: false,
        }],
    ));
    std::thread::sleep(Duration::from_millis(1500));
    player.close();
    let heard = captured.lock().expect("lock").clone();
    let mono: Vec<f32> = heard.chunks(2).map(|frame| frame[0]).collect();
    let start = mono.iter().position(|s| s.abs() > 1e-6).expect("sound");
    let played = &mono[start..];
    let level = levels(played);
    // The tone never drops out: no 10 ms anywhere is silent.
    let quietest = level[..level.len() - 1]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    assert!(quietest > -60.0, "a gap: {quietest} dBFS");
    // And the change was heard: 12 dB down by the end.
    let early = level[10..40].iter().sum::<f64>() / 30.0;
    let late = level[level.len() - 40..level.len() - 10]
        .iter()
        .sum::<f64>()
        / 30.0;
    assert!((early - late - 12.0).abs() < 0.5, "{early} then {late}");
}
