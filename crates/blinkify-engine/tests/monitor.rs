//! Audio monitoring against the real sidecar (#31): the meter against
//! FFmpeg's own EBU R128 meter, the clip indication, a monitor volume that
//! never reaches the programme, silence through a gap, solo and mute across
//! tracks, and following the default output device.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::similar_names
)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{Limits, Orchestrator, Priority, SidecarCommand};
use blinkify_engine::playback::{
    AudioChoice, AudioTrack, DefaultDevice, MAIN_TRACK, MonitorCommand, PlaybackPlan, Player,
    PlayerOptions, Segment, SourceMedia, TransportCommand,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::time::{self, Rounding};

const RATE: u32 = 48_000;

type Captured = Arc<Mutex<Vec<f32>>>;

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

fn source(orchestrator: &Orchestrator, path: &Path) -> Arc<SourceMedia> {
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index =
        Arc::new(KeyframeIndex::open(path, &info, orchestrator.clone(), None).expect("index"));
    Arc::new(SourceMedia::new(path, info, index).expect("playable"))
}

fn player(
    orchestrator: &Orchestrator,
    plan: PlaybackPlan,
    default_device: DefaultDevice,
) -> (Player, Captured) {
    let samples: Captured = Arc::new(Mutex::new(Vec::new()));
    let player = Player::new(
        orchestrator.clone(),
        plan,
        &PlayerOptions {
            audio: AudioChoice::Capture {
                sample_rate: RATE,
                samples: Arc::clone(&samples),
            },
            max_width: 160,
            max_height: 90,
            default_device,
        },
    );
    (player, samples)
}

/// A clip of `graph`'s sound (and a small picture), `seconds` long, as FLAC so
/// what is decoded is exactly what was generated.
fn tone(orchestrator: &Orchestrator, dir: &Path, name: &str, graph: &str, seconds: u32) -> PathBuf {
    let path = dir.join(name);
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!("testsrc2=size=160x90:rate=30:duration={seconds}"))
                .lavfi_input(&format!("{graph},atrim=duration={seconds}"))
                .option("-c:v", "mjpeg")
                .option("-c:a", "flac")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("tone");
    path
}

fn whole(source: &Arc<SourceMedia>) -> Segment {
    let (source_in, source_out) = source.full_range();
    Segment {
        source: Arc::clone(source),
        source_in,
        source_out,
        timeline_start: 0,
    }
}

fn segment(source: &Arc<SourceMedia>, from: f64, to: f64, at: f64) -> Segment {
    let tb = source.time_base();
    Segment {
        source: Arc::clone(source),
        source_in: time::from_seconds(from, tb, Rounding::Nearest),
        source_out: time::from_seconds(to, tb, Rounding::Nearest),
        timeline_start: (at * 1_000_000.0).round() as i64,
    }
}

/// Wait until at least `seconds` of sound follow the silence captured before
/// Play, and return everything captured.
fn capture_seconds(captured: &Captured, seconds: f64) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let heard = captured.lock().expect("captured").clone();
        let start = heard
            .iter()
            .position(|s| s.abs() > 1e-6)
            .unwrap_or(heard.len());
        if (heard.len() - start) as f64 >= seconds * 2.0 * f64::from(RATE) {
            return heard;
        }
        assert!(Instant::now() < deadline, "not enough sound was played");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The power of `frequency` in the left channel, by the Goertzel algorithm.
fn power_at(samples: &[f32], frequency: f64) -> f64 {
    let coefficient = 2.0 * (2.0 * std::f64::consts::PI * frequency / f64::from(RATE)).cos();
    let (mut previous, mut before) = (0.0, 0.0);
    for frame in samples.as_chunks::<2>().0 {
        let current = f64::from(frame[0]) + coefficient * previous - before;
        before = previous;
        previous = current;
    }
    let n = samples.len() as f64 / 2.0;
    (previous * previous + before * before - coefficient * previous * before) / (n * n)
}

/// What FFmpeg's own EBU R128 meter says the short-term loudness of `path`
/// is, at its end.
fn ffmpeg_short_term(orchestrator: &Orchestrator, path: &Path) -> f64 {
    let output = orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "info")
                .flag("-nostats")
                .input(path)
                .option("-map", "0:a:0")
                .option("-af", "ebur128")
                .output_null(),
            Priority::Interactive,
        )
        .expect("ebur128");
    // Lines like `t: 4.0  TARGET:-23 LUFS    M: -20.0 S: -20.0 ...`; the
    // tail of stderr holds the last of them, before the summary.
    output
        .stderr_tail
        .iter()
        .rev()
        .find_map(|line| {
            let s = line.split(" S:").nth(1)?;
            s.split_whitespace().next()?.parse::<f64>().ok()
        })
        .expect("a short-term reading")
}

#[test]
fn the_meter_agrees_with_ffmpegs_ebur128_and_reads_the_peak() {
    let orchestrator = orchestrator();
    let dir = common::scratch("monitor-meter");
    // A steady 1 kHz tone in both channels (the mono source spread to stereo
    // at −3 dB each): its short-term loudness is the same wherever it is
    // measured, and for a 1 kHz tone BS.1770 puts it within a tenth of a LU
    // of the sample peak in dBFS.
    let path = tone(
        &orchestrator,
        &dir,
        "minus 20.mkv",
        "sine=frequency=1000:sample_rate=48000,volume=0.8:precision=double,aformat=channel_layouts=stereo",
        6,
    );
    let (player, captured) = player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        DefaultDevice::System,
    );
    player.command(TransportCommand::Play);
    capture_seconds(&captured, 4.0);
    let levels = player.levels();
    player.close();

    let ours = levels.short_term_lufs.expect("three seconds were heard");
    let theirs = ffmpeg_short_term(&orchestrator, &path);
    println!("short-term: ours {ours:.2} LUFS, ffmpeg {theirs:.2} LUFS");
    assert!((ours - theirs).abs() < 0.2, "{ours} against {theirs}");
    let expected_peak = 20.0 * (0.8_f64 / 8.0 / std::f64::consts::SQRT_2).log10();
    for peak in levels.peak_db {
        let peak = peak.expect("a peak");
        assert!((peak - expected_peak).abs() < 0.05, "{peak} dBFS");
        assert!((ours - peak).abs() < 0.1, "{ours} LUFS against {peak} dBFS");
    }
    assert!(!levels.clipped);
}

#[test]
fn a_clip_is_latched_until_it_is_reset() {
    let orchestrator = orchestrator();
    let dir = common::scratch("monitor-clip");
    // Full scale for a moment, then quiet.
    let path = tone(
        &orchestrator,
        &dir,
        "clips once.mkv",
        "sine=frequency=440:sample_rate=48000,volume='if(lt(t,0.3),20,0.4)':eval=frame,aformat=channel_layouts=stereo",
        4,
    );
    let (player, captured) = player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        DefaultDevice::System,
    );
    player.command(TransportCommand::Play);
    capture_seconds(&captured, 1.5);
    assert!(player.levels().clipped, "the overload was not caught");
    capture_seconds(&captured, 2.5);
    assert!(
        player.levels().clipped,
        "the indication went out on its own"
    );
    player.monitor(MonitorCommand::ResetClip);
    assert!(!player.levels().clipped);
    player.close();
}

#[test]
fn the_monitor_volume_changes_what_is_heard_and_never_the_programme() {
    let orchestrator = orchestrator();
    let dir = common::scratch("monitor-volume");
    let path = tone(
        &orchestrator,
        &dir,
        "tone.mkv",
        "sine=frequency=1000:sample_rate=48000,volume=4:precision=double,aformat=channel_layouts=stereo",
        6,
    );
    let run = |volume: f64, muted: bool| {
        let (player, captured) = player(
            &orchestrator,
            PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
            DefaultDevice::System,
        );
        player.monitor(MonitorCommand::Volume { level: volume });
        player.monitor(MonitorCommand::Mute { muted });
        player.command(TransportCommand::Play);
        // Muted, nothing is heard; wait by the clock instead.
        let deadline = Instant::now() + Duration::from_secs(20);
        while player.position().1 < 3_500_000 {
            assert!(Instant::now() < deadline, "playback stalled");
            std::thread::sleep(Duration::from_millis(20));
        }
        let levels = player.levels();
        let heard = captured.lock().expect("captured").clone();
        player.close();
        let loudest = heard.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
        (levels, loudest)
    };
    let (full, full_peak) = run(1.0, false);
    let (quarter, quarter_peak) = run(0.25, false);
    let (muted, muted_peak) = run(1.0, true);
    // What is heard follows the monitor volume…
    assert!(full_peak > 0.3, "{full_peak}");
    assert!(
        (quarter_peak - full_peak * 0.25).abs() < 0.001,
        "{quarter_peak} against {full_peak}"
    );
    assert_eq!(muted_peak, 0.0);
    // …and the programme — what the meter measures, what an export is made
    // of — does not.
    let reading =
        |levels: blinkify_engine::audio::MonitorLevels| levels.short_term_lufs.expect("loudness");
    assert!((reading(full) - reading(quarter)).abs() < 0.05);
    assert!((reading(full) - reading(muted)).abs() < 0.05);
}

#[test]
fn a_gap_in_the_timeline_is_silence_and_the_playhead_runs_through_it() {
    let orchestrator = orchestrator();
    let dir = common::scratch("monitor-gap");
    let path = tone(
        &orchestrator,
        &dir,
        "tone.mkv",
        "sine=frequency=440:sample_rate=48000,aformat=channel_layouts=stereo",
        4,
    );
    let src = source(&orchestrator, &path);
    // One second of tone, one second of nothing, one second of tone.
    let plan = PlaybackPlan::new(vec![
        segment(&src, 0.0, 1.0, 0.0),
        segment(&src, 1.0, 2.0, 2.0),
    ])
    .expect("plan");
    let (player, captured) = player(&orchestrator, plan, DefaultDevice::System);
    player.command(TransportCommand::Play);
    let heard = capture_seconds(&captured, 3.05);
    let (_, position) = player.position();
    player.close();

    let start = heard.iter().position(|s| s.abs() > 1e-6).expect("sound") & !1;
    let second =
        |k: usize| &heard[start + k * 2 * RATE as usize..start + (k + 1) * 2 * RATE as usize];
    assert!(power_at(second(0), 440.0) > 1e-4, "the first clip is heard");
    // The gap is exactly silent — a few samples either side are the edges.
    let gap = &second(1)[64..second(1).len() - 64];
    assert!(gap.iter().all(|s| *s == 0.0), "the gap is not silent");
    assert!(
        power_at(second(2), 440.0) > 1e-4,
        "the second clip is heard"
    );
    assert!(
        position > 2_500_000,
        "the playhead stopped at the gap: {position}"
    );
}

#[test]
fn solo_and_mute_decide_which_of_several_tracks_is_heard() {
    const MUSIC: u32 = 7;
    let orchestrator = orchestrator();
    let dir = common::scratch("monitor-tracks");
    let low = source(
        &orchestrator,
        &tone(
            &orchestrator,
            &dir,
            "440.mkv",
            "sine=frequency=440:sample_rate=48000,aformat=channel_layouts=stereo",
            8,
        ),
    );
    let high = source(
        &orchestrator,
        &tone(
            &orchestrator,
            &dir,
            "880.mkv",
            "sine=frequency=880:sample_rate=48000,aformat=channel_layouts=stereo",
            8,
        ),
    );
    let listen = |commands: &[MonitorCommand]| {
        let plan = PlaybackPlan::new(vec![whole(&low)])
            .expect("plan")
            .with_audio_track(AudioTrack {
                id: MUSIC,
                segments: vec![whole(&high)],
            })
            .expect("a second track");
        let (player, captured) = player(&orchestrator, plan, DefaultDevice::System);
        for command in commands {
            player.monitor(*command);
        }
        player.command(TransportCommand::Play);
        let deadline = Instant::now() + Duration::from_secs(20);
        while player.position().1 < 1_500_000 {
            assert!(Instant::now() < deadline, "playback stalled");
            std::thread::sleep(Duration::from_millis(20));
        }
        let heard = captured.lock().expect("captured").clone();
        player.close();
        // The last second.
        let tail = &heard[heard.len() - 2 * RATE as usize..];
        (power_at(tail, 440.0) > 1e-4, power_at(tail, 880.0) > 1e-4)
    };
    assert_eq!(listen(&[]), (true, true), "both, mixed");
    assert_eq!(
        listen(&[MonitorCommand::Solo {
            track: MUSIC,
            on: true
        }]),
        (false, true),
        "solo the music"
    );
    assert_eq!(
        listen(&[MonitorCommand::MuteTrack {
            track: MUSIC,
            on: true
        }]),
        (true, false),
        "mute the music"
    );
    assert_eq!(
        listen(&[
            MonitorCommand::Solo {
                track: MAIN_TRACK,
                on: true
            },
            MonitorCommand::Solo {
                track: MUSIC,
                on: true
            },
        ]),
        (true, true),
        "solo both"
    );
    assert_eq!(
        listen(&[
            MonitorCommand::MuteTrack {
                track: MAIN_TRACK,
                on: true
            },
            MonitorCommand::MuteTrack {
                track: MUSIC,
                on: true
            },
        ]),
        (false, false),
        "mute both"
    );
}

#[test]
fn playback_follows_the_default_device_when_it_changes() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let default = Arc::new(Mutex::new(Some("speakers".to_owned())));
    let (player, _captured) = player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        DefaultDevice::Given(Arc::clone(&default)),
    );
    player.command(TransportCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(10);
    while player.position().1 < 500_000 {
        assert!(Instant::now() < deadline, "playback never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(player.audio_switches(), 0);
    // The user plugs in headphones.
    *default.lock().expect("default") = Some("headphones".to_owned());
    let deadline = Instant::now() + Duration::from_secs(5);
    while player.audio_switches() == 0 {
        assert!(Instant::now() < deadline, "the change was not followed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (_, before) = player.position();
    std::thread::sleep(Duration::from_millis(700));
    let (_, after) = player.position();
    let status = player.status();
    player.close();
    assert_eq!(
        status.state,
        blinkify_engine::playback::PlaybackState::Playing
    );
    assert!(after > before + 300_000, "playback stopped at the switch");
    // A device going away altogether (the default becomes none) is not a
    // switch to nothing: playback keeps its output until a new one appears.
    assert_eq!(player.audio_switches(), 1);
}
