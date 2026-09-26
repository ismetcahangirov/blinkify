//! What the audio inspector (#49) relies on, end to end: sweeping a control
//! while playing never stops or stutters the sound, the meter reads the
//! processed signal, bypassing the whole chain gives back the source's own
//! sound bit for bit, and the preview and the export run one chain of all
//! three steps.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::audio::chain;
use blinkify_engine::audio::denoise::Models;
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{ExportRequest, export};
use blinkify_engine::export::plan::Media;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, Player, PlayerOptions, Segment, SourceMedia,
    TransportCommand, audio_request,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::project::Operation;
use blinkify_engine::project::evaluate::AudioOperation;
use blinkify_engine::tier::ExportTier;
use blinkify_engine::time::{self, Rounding};
use common::fixture::Source;

const TONE: &str = "h264-high-closed-gop.mp4";

fn models() -> Models {
    Models::in_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("models/rnnoise"))
        .expect("the bundled model")
}

fn media(path: &Path) -> Arc<SourceMedia> {
    let orchestrator = common::orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index = Arc::new(KeyframeIndex::open(path, &info, orchestrator, None).expect("index"));
    Arc::new(SourceMedia::new(path, info, index).expect("playable"))
}

fn plan(media: &Arc<SourceMedia>, audio: Vec<AudioOperation>) -> PlaybackPlan {
    let tb = media.time_base();
    let mut segment = Segment::new(
        Arc::clone(media),
        0,
        time::from_seconds(4.0, tb, Rounding::Nearest),
        0,
    );
    segment.audio = audio;
    PlaybackPlan::new(vec![segment]).expect("plan")
}

fn gain(db: f64) -> AudioOperation {
    AudioOperation::Gain {
        db,
        ceiling_dbtp: -1.0,
        bypassed: false,
    }
}

type Captured = Arc<Mutex<Vec<f32>>>;

fn playing(plan: PlaybackPlan) -> (Player, Captured) {
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let player = Player::new(
        common::orchestrator(),
        plan,
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
    (player, captured)
}

/// The level of each 10 ms of the left channel, in dBFS, from the first
/// sound on.
fn levels(captured: &Captured) -> Vec<f64> {
    let heard = captured.lock().expect("lock").clone();
    let left: Vec<f32> = heard.chunks(2).map(|frame| frame[0]).collect();
    let start = left.iter().position(|s| s.abs() > 1e-6).expect("sound");
    left[start..]
        .chunks(480)
        .filter(|chunk| chunk.len() == 480)
        .map(|chunk| {
            let mean = chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 480.0;
            10.0 * mean.max(1e-20).log10()
        })
        .collect()
}

#[test]
fn sweeping_a_control_while_playing_never_stops_the_sound() {
    let media = media(&common::corpus(TONE));
    let (player, captured) = playing(plan(&media, Vec::new()));
    // A drag of the gain slider: a new chain every 60 ms, for a second and
    // a half, faster than any decoder can start.
    for step in 0..25 {
        player.set_plan(plan(&media, vec![gain(-12.0 + f64::from(step) * 0.5)]));
        std::thread::sleep(Duration::from_millis(60));
    }
    std::thread::sleep(Duration::from_millis(1000));
    let (_, position) = player.position();
    player.close();
    let level = levels(&captured);
    let quietest = level[..level.len() - 1]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    assert!(quietest > -60.0, "the sound dropped out: {quietest} dBFS");
    // The clock ran through it all: the picture was never held.
    assert!(position > 2_000_000, "{position}");
    // The last value of the drag, 0 dB, is what is heard at the end: the
    // level it started at, before the drag went down to −12 dB and back.
    let tail = level[level.len() - 30..level.len() - 5].iter().sum::<f64>() / 25.0;
    let head = level[5..30].iter().sum::<f64>() / 25.0;
    assert!((head - tail).abs() < 0.5, "{head} then {tail}");
    let lowest = level.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(
        head - lowest > 6.0,
        "the drag was heard: {head} down to {lowest}"
    );
}

#[test]
fn the_meter_reads_the_processed_signal() {
    let media = media(&common::corpus(TONE));
    let (plain, _) = playing(plan(&media, Vec::new()));
    std::thread::sleep(Duration::from_millis(700));
    let unprocessed = plain.levels().peak_db[0].expect("a peak");
    plain.close();
    let (quieter, _) = playing(plan(&media, vec![gain(-12.0)]));
    std::thread::sleep(Duration::from_millis(700));
    let processed = quieter.levels().peak_db[0].expect("a peak");
    quieter.close();
    assert!(
        (unprocessed - processed - 12.0).abs() < 0.5,
        "{unprocessed} then {processed}"
    );
}

#[test]
fn a_chain_bypassed_whole_gives_back_the_sources_own_sound() {
    let source = Source::corpus(TONE);
    let all = [
        Operation::Denoise {
            strength: 0.8,
            bypassed: true,
        },
        Operation::Gain {
            db: 9.0,
            ceiling_dbtp: -1.0,
            bypassed: true,
        },
        Operation::Normalise {
            target_lufs: -14.0,
            ceiling_dbtp: -1.0,
            bypassed: true,
        },
    ];
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &all)]);
    let audio: Vec<ExportTier> = plan
        .segments
        .iter()
        .filter(|s| s.media == Media::Audio)
        .map(|s| s.tier)
        .collect();
    assert_eq!(audio, vec![ExportTier::StreamCopy], "nothing applies");
    let target: PathBuf = common::scratch("audio-inspector-bypass").join("out.mp4");
    let inputs = source.inputs();
    export(
        &common::orchestrator(),
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &target,
            overwrite: false,
            audio: AudioTarget::default(),
            models: None,
            cancel: CancelToken::default(),
            on_progress: None,
        },
    )
    .expect("exports");
    // The sound is the source's own packets: bit for bit.
    let source_audio = common::md5s(&common::packet_hashes(&source.path, "a:0"));
    let output_audio = common::md5s(&common::packet_hashes(&target, "a:0"));
    assert!(!output_audio.is_empty());
    assert!(
        output_audio.iter().all(|hash| source_audio.contains(hash)),
        "a bypassed chain re-encoded the sound"
    );
}

#[test]
fn the_preview_and_the_export_run_one_chain_of_all_three_steps() {
    let path = common::corpus(TONE);
    let media = media(&path);
    let steps = vec![
        AudioOperation::Normalise {
            target_lufs: -16.0,
            ceiling_dbtp: -1.5,
            bypassed: false,
            gain_db: Some(7.25),
        },
        gain(-2.0),
        AudioOperation::Denoise {
            strength: 0.6,
            bypassed: false,
        },
    ];
    let expected = chain::filters(&steps, 48_000, Some(&models())).expect("built");
    let preview_plan = plan(&media, steps.clone());
    let segment = preview_plan.segments().first().expect("a segment");
    let preview = audio_request(
        segment,
        segment.timeline_start,
        48_000,
        1.0,
        Some(&models()),
    )
    .expect("sound")
    .command()
    .to_string();
    assert!(preview.contains(&expected), "{preview}");
    let denoise = expected.find("arnndn").expect("denoise");
    let gained = expected.find("volume=-2dB").expect("gain");
    let normalised = expected.find("volume=7.25dB").expect("normalise");
    assert!(denoise < gained && gained < normalised, "{expected}");

    // The export of the same steps runs the same string.
    let source = Source::corpus(TONE);
    let operations = [
        Operation::normalise(-16.0),
        Operation::gain(-2.0),
        Operation::denoise(0.6),
    ];
    let mut export_plan = source.plan_clips(vec![source.clip(
        1,
        0,
        source.keyframe(1),
        source.keyframe(3),
        &operations,
    )]);
    // As resolved by a measurement (#48), fixed here to compare strings.
    for segment in &mut export_plan.segments {
        for input in &mut segment.sources {
            input.audio.clone_from(&steps);
        }
    }
    let target = common::scratch("audio-inspector-chain").join("out.mkv");
    let inputs = source.inputs();
    let outcome = export(
        &common::orchestrator(),
        ExportRequest {
            plan: &export_plan,
            inputs: &inputs,
            target: &target,
            overwrite: false,
            audio: AudioTarget::Flac,
            models: Some(&models()),
            cancel: CancelToken::default(),
            on_progress: None,
        },
    )
    .expect("exports");
    let encoder = outcome
        .commands
        .iter()
        .find(|command| command.contains("arnndn"))
        .expect("an encoder");
    assert!(encoder.contains(&expected), "{encoder}");
}
