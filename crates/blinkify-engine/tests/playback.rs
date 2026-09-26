//! The player against the real sidecar (#28): the audio master clock, A/V
//! sync, transport, exact frame steps across clip boundaries, seamless
//! boundaries, preview speed, loops, and an audio output that changes or is
//! missing.
//!
//! Audio is played into a capturing sink at real time, so what was "heard"
//! can be compared sample for sample with an independent decode of the source.

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

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{
    Flow, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::playback::timecode;
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, LoopRange, PlaybackPlan, PlaybackState, Player, PlayerOptions,
    PreviewSpeed, Segment, ShownFrame, SourceMedia, TransportCommand,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::time::{self, Rounding};

const RATE: u32 = 48_000;

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

type Captured = Arc<Mutex<Vec<f32>>>;

fn capturing_player(orchestrator: &Orchestrator, plan: PlaybackPlan) -> (Player, Captured) {
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
            default_device: DefaultDevice::System,
            models: None,
        },
    );
    (player, samples)
}

/// A segment of `source` from `from` to `to` seconds, at `at` seconds on the
/// timeline.
fn segment(source: &Arc<SourceMedia>, from: f64, to: f64, at: f64) -> Segment {
    let tb = source.time_base();
    Segment::new(
        Arc::clone(source),
        time::from_seconds(from, tb, Rounding::Nearest),
        time::from_seconds(to, tb, Rounding::Nearest),
        (at * 1_000_000.0).round() as i64,
    )
}

/// The source's first audio stream, decoded from the start as interleaved
/// stereo at 48 kHz: what the player must have played.
fn reference_audio(orchestrator: &Orchestrator, path: &Path) -> Vec<f32> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    orchestrator
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:a:0")
                .option(
                    "-af",
                    "aresample=48000,aformat=sample_fmts=flt:channel_layouts=stereo",
                )
                .option("-f", "f32le")
                .output_stdout(),
            Priority::Interactive,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("sink").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("reference audio");
    bytes
        .lock()
        .expect("bytes")
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}

/// Every frame's presentation timestamp, as `ffprobe` decodes them.
fn reference_pts(orchestrator: &Orchestrator, path: &Path) -> Vec<i64> {
    let output = orchestrator
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", "v:0")
                .option("-show_entries", "frame=pts")
                .option("-of", "csv=p=0")
                .input(path),
            Priority::Interactive,
        )
        .expect("ffprobe frames");
    let mut pts: Vec<i64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
        .collect();
    pts.sort_unstable();
    pts
}

/// The frame at `pts`, decoded independently at 160x90.
fn reference_frame(orchestrator: &Orchestrator, path: &Path, pts: i64) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    orchestrator
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .flag("-copyts")
                .flag("-noautorotate")
                .input(path)
                .option("-map", "0:v:0")
                .option(
                    "-vf",
                    format!("select=eq(pts\\,{pts}),scale=160:90:flags=bilinear,format=rgba"),
                )
                .option("-fps_mode", "passthrough")
                .option("-frames:v", "1")
                .option("-f", "rawvideo")
                .output_stdout(),
            Priority::Interactive,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("sink").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("reference frame");
    bytes.lock().expect("bytes").clone()
}

/// The newest frame shown, once the player has stopped resolving a seek or a
/// step and nothing newer arrives for a moment.
fn settle(player: &Player) -> Arc<ShownFrame> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.status().resolving {
        assert!(Instant::now() < deadline, "the frame never arrived");
        let _ = player.next_frame(u64::MAX, Duration::from_millis(10));
    }
    let mut last = player
        .next_frame(0, Duration::from_secs(5))
        .expect("a frame");
    while let Some(newer) = player.next_frame(last.seq, Duration::from_millis(150)) {
        last = newer;
    }
    last
}

fn seconds(position: i64) -> f64 {
    position as f64 / 1_000_000.0
}

/// Wait until the clock has left `from`, and return where it is. Starting,
/// resuming and switching outputs each pre-roll first, and on a loaded
/// machine that takes longer than any fixed sleep would allow for.
fn moving_from(player: &Player, from: i64) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, position) = player.position();
        if position != from {
            return position;
        }
        assert!(Instant::now() < deadline, "the clock never started");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// How far the clock moves, in timeline seconds per real second, over
/// `span` once it is moving.
fn clock_rate(player: &Player, span: Duration) -> f64 {
    let (_, from) = player.position();
    let from = moving_from(player, from);
    let started = Instant::now();
    std::thread::sleep(span);
    let (_, to) = player.position();
    seconds(to - from) / started.elapsed().as_secs_f64()
}

/// Wait until at least `frames` frames of sound have been captured.
fn capture_at_least(captured: &Captured, frames: usize) -> Vec<f32> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let heard = captured.lock().expect("captured").clone();
        let sound = heard
            .iter()
            .position(|s| s.abs() > 1e-6)
            .map_or(0, |start| heard.len() - start);
        if sound >= frames * 2 {
            return heard;
        }
        assert!(Instant::now() < deadline, "not enough sound was played");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Where the captured audio stops being the silence captured before Play.
fn first_sound(samples: &[f32]) -> usize {
    samples
        .iter()
        .position(|s| s.abs() > 1e-6)
        .expect("something was played")
        & !1
}

#[test]
fn what_is_heard_is_the_source_sample_for_sample_and_the_clock_follows_it() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let src = source(&orchestrator, &path);
    let (player, captured) =
        capturing_player(&orchestrator, PlaybackPlan::whole(src).expect("plan"));
    player.command(TransportCommand::Play);
    capture_at_least(&captured, 2 * RATE as usize);
    // The clock and the capture read at the same moment.
    let (heard, position) = {
        let heard = captured.lock().expect("captured");
        (heard.clone(), player.position().1)
    };
    player.close();

    let reference = reference_audio(&orchestrator, &path);
    let start = first_sound(&heard);
    let played = &heard[start..];
    let reference_start = first_sound(&reference);
    let expected = &reference[reference_start..reference_start + played.len()];
    // Nothing dropped, nothing repeated, nothing out of place: two seconds
    // of audio identical to an independent decode.
    assert!(
        played.len() >= 2 * 2 * RATE as usize,
        "{} samples",
        played.len()
    );
    assert!(played == expected, "the audio heard is not the source's");
    // And the clock is where that audio is: within a frame of the samples
    // actually consumed.
    let heard_seconds = played.len() as f64 / 2.0 / f64::from(RATE);
    assert!(
        (seconds(position) - heard_seconds).abs() < 1.0 / 30.0,
        "clock {} s, audio {heard_seconds} s",
        seconds(position)
    );
}

#[test]
fn video_stays_within_a_frame_of_the_audio_clock() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, _captured) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut after = 0;
    let mut worst: f64 = 0.0;
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(50)) {
            after = frame.seq;
            worst = worst.max(seconds(frame.chosen_at - frame.position));
        }
    }
    player.close();
    assert!(worst < 1.0 / 30.0, "video lagged audio by {worst} s");
}

#[test]
fn stepping_moves_exactly_one_real_frame_each_way_and_matches_by_content() {
    let orchestrator = orchestrator();
    for name in ["h264-high-closed-gop.mp4", "vfr-screen.mp4"] {
        let path = common::corpus(name);
        let reference = reference_pts(&orchestrator, &path);
        let (player, _) = capturing_player(
            &orchestrator,
            PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        );
        let mut shown = Vec::new();
        for _ in 0..40 {
            player.command(TransportCommand::Step { frames: 1 });
            let frame = settle(&player);
            shown.push(frame.picture.as_ref().expect("a picture").pts);
        }
        assert_eq!(shown, reference[1..=40].to_vec(), "{name}: forwards");
        for _ in 0..5 {
            player.command(TransportCommand::Step { frames: -1 });
        }
        let back = settle(&player);
        let picture = back.picture.as_ref().expect("a picture");
        assert_eq!(picture.pts, reference[35], "{name}: backwards");
        assert!(
            picture.pixels == reference_frame(&orchestrator, &path, reference[35]),
            "{name}: the frame stepped to is not the frame at its timestamp"
        );
        player.close();
    }
}

#[test]
fn a_step_crosses_a_clip_boundary_onto_the_neighbouring_frame() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let src = source(&orchestrator, &path);
    let reference = reference_pts(&orchestrator, &path);
    let tb = src.time_base();
    // 0–1 s, then 2–3 s placed straight after it.
    let plan = PlaybackPlan::new(vec![
        segment(&src, 0.0, 1.0, 0.0),
        segment(&src, 2.0, 3.0, 1.0),
    ])
    .expect("plan");
    let (player, _) = capturing_player(&orchestrator, plan);
    let one_second = time::from_seconds(1.0, tb, Rounding::Nearest);
    let two_seconds = time::from_seconds(2.0, tb, Rounding::Nearest);
    let last_of_first = *reference
        .iter()
        .rfind(|pts| **pts < one_second)
        .expect("frame");
    let first_of_second = *reference
        .iter()
        .find(|pts| **pts >= two_seconds)
        .expect("frame");

    player.command(TransportCommand::Seek { position: 966_667 });
    assert_eq!(
        settle(&player).picture.as_ref().expect("picture").pts,
        last_of_first
    );
    player.command(TransportCommand::Step { frames: 1 });
    let across = settle(&player);
    assert_eq!(
        across.picture.as_ref().expect("picture").pts,
        first_of_second
    );
    assert_eq!(across.position, 1_000_000, "it starts the second clip");
    player.command(TransportCommand::Step { frames: -1 });
    assert_eq!(
        settle(&player).picture.as_ref().expect("picture").pts,
        last_of_first
    );
    player.close();
}

#[test]
fn a_clip_boundary_plays_without_a_gap_in_audio_or_video() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let src = source(&orchestrator, &path);
    let plan = PlaybackPlan::new(vec![
        segment(&src, 0.0, 1.0, 0.0),
        segment(&src, 2.0, 3.5, 1.0),
    ])
    .expect("plan");
    let (player, captured) = capturing_player(&orchestrator, plan);
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut after = 0;
    let mut positions = Vec::new();
    while started.elapsed() < Duration::from_millis(2000) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(50)) {
            after = frame.seq;
            positions.push(frame.position);
        }
    }
    let heard = captured.lock().expect("captured").clone();
    player.close();

    // Video: every frame shown in turn, across the boundary, none skipped
    // and none repeated.
    let across: Vec<_> = positions
        .windows(2)
        .filter(|w| w[0] < 1_000_000 && w[1] >= 1_000_000)
        .collect();
    assert_eq!(
        across.len(),
        1,
        "the boundary was crossed once: {positions:?}"
    );
    assert!(
        positions
            .windows(2)
            .all(|w| w[1] > w[0] && w[1] - w[0] < 70_000),
        "a gap or a repeat in the picture: {positions:?}"
    );

    // Audio: the sample after the boundary is the source's sample at 2.0 s,
    // and the one before it the source's last before 1.0 s — on the exact
    // sample. Exact equality is not available after a seek: the corpus audio
    // is AAC with perceptual noise substitution, whose decoder carries a
    // random generator that a seek restarts, leaving differences near
    // -95 dB. A one-sample misplacement of a 440 Hz tone differs by more than
    // -50 dB, so a tolerance between the two still pins the sample.
    let reference = reference_audio(&orchestrator, &path);
    let start = first_sound(&heard);
    let origin = first_sound(&reference);
    let boundary = start + 2 * RATE as usize;
    let one = origin + 2 * RATE as usize;
    let two = origin + 2 * 2 * RATE as usize;
    let span = 4800;
    let after = &heard[boundary..boundary + span];
    let before = &heard[boundary - span..boundary];
    assert!(
        max_difference(after, &reference[two..two + span]) < 1e-4,
        "the second clip's audio does not start on its first sample"
    );
    assert!(
        max_difference(after, &reference[two + 2..two + 2 + span]) > 1e-3
            && max_difference(after, &reference[two - 2..two - 2 + span]) > 1e-3,
        "the tolerance cannot tell neighbouring samples apart"
    );
    assert!(
        max_difference(before, &reference[one - span..one]) < 1e-4,
        "the first clip's audio does not run to its last sample"
    );
}

fn max_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

#[test]
fn timecode_names_the_frame_on_screen() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, _) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    let mut status = player.status();
    for _ in 0..35 {
        status = player.command(TransportCommand::Step { frames: 1 });
    }
    assert_eq!(status.timecode, "00:00:01:05");
    assert_eq!(status.duration_timecode, "00:00:04:00");
    let shown = settle(&player);
    assert_eq!(
        timecode::format(shown.frame_number, status.frame_rate),
        status.timecode
    );
    player.close();
}

#[test]
fn preview_speed_scales_the_clock_and_keeps_the_pitch() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    for (speed, factor) in [(PreviewSpeed::Double, 2.0), (PreviewSpeed::Half, 0.5)] {
        let (player, captured) = capturing_player(
            &orchestrator,
            PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        );
        player.command(TransportCommand::SetSpeed { speed });
        player.command(TransportCommand::Play);
        let rate = clock_rate(&player, Duration::from_millis(1000));
        let heard = capture_at_least(&captured, RATE as usize);
        player.close();
        assert!(
            (rate - factor).abs() < 0.15,
            "{speed:?}: the clock ran at {rate}x"
        );

        // The source is a 440 Hz tone. Time-stretched, it is still 440 Hz:
        // count rising zero crossings of the left channel over a second.
        let left: Vec<f32> = heard[first_sound(&heard)..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|frame| frame[0])
            .take(RATE as usize)
            .collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (430..=450).contains(&crossings),
            "{speed:?}: pitch moved to {crossings} Hz"
        );
    }
}

#[test]
fn a_loop_plays_its_range_over_and_over() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, _) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    let range = LoopRange {
        start: 1_000_000,
        end: 1_500_000,
    };
    player.command(TransportCommand::SetLoop { range: Some(range) });
    player.command(TransportCommand::Seek {
        position: range.start,
    });
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut after = 0;
    let mut wraps = 0;
    let mut previous = range.start;
    while started.elapsed() < Duration::from_millis(1800) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(50)) {
            after = frame.seq;
            assert!(
                frame.position >= range.start && frame.position < range.end,
                "{} is outside the loop",
                frame.position
            );
            if frame.position < previous {
                wraps += 1;
            }
            previous = frame.position;
        }
    }
    player.close();
    assert!(wraps >= 2, "the loop wrapped {wraps} times in 1.8 s");
}

#[test]
fn the_transport_jumps_stops_and_ends_where_it_says() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let reference = reference_pts(&orchestrator, &path);
    let src = source(&orchestrator, &path);
    let plan = PlaybackPlan::new(vec![segment(&src, 0.0, 1.0, 0.0)]).expect("plan");
    let (player, _) = capturing_player(&orchestrator, plan);
    let one_second = time::from_seconds(1.0, src.time_base(), Rounding::Nearest);
    let last = *reference
        .iter()
        .rfind(|pts| **pts < one_second)
        .expect("frame");

    player.command(TransportCommand::JumpToEnd);
    assert_eq!(settle(&player).picture.as_ref().expect("picture").pts, last);
    player.command(TransportCommand::JumpToStart);
    assert_eq!(
        settle(&player).picture.as_ref().expect("picture").pts,
        reference[0]
    );

    player.command(TransportCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(4);
    while player.status().state != PlaybackState::Ended {
        assert!(Instant::now() < deadline, "playback never ended");
        let _ = player.next_frame(u64::MAX, Duration::from_millis(20));
    }
    assert_eq!(settle(&player).picture.as_ref().expect("picture").pts, last);
    // Play at the end starts again from the start.
    let status = player.command(TransportCommand::Play);
    assert_eq!(status.state, PlaybackState::Playing);
    std::thread::sleep(Duration::from_millis(200));
    let (_, position) = player.position();
    assert!(position < 500_000, "restarted at {position}");
    let status = player.command(TransportCommand::Stop);
    assert_eq!(status.state, PlaybackState::Paused);
    assert_eq!(
        settle(&player).picture.as_ref().expect("picture").pts,
        reference[0]
    );
    player.close();
}

#[test]
fn pause_holds_the_frame_and_play_resumes_from_it() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, _) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    player.command(TransportCommand::Play);
    std::thread::sleep(Duration::from_millis(800));
    let paused = player.command(TransportCommand::Pause);
    let held = settle(&player);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        player.position().1,
        paused.position,
        "the clock moved while paused"
    );
    assert!(
        player
            .next_frame(held.seq, Duration::from_millis(200))
            .is_none(),
        "paused, yet a new frame"
    );
    player.command(TransportCommand::Play);
    let resumed = moving_from(&player, paused.position);
    assert!(
        resumed > paused.position && resumed < paused.position + 200_000,
        "resumed at {resumed} after pausing at {}",
        paused.position
    );
    player.close();
}

#[test]
fn playback_carries_on_through_a_change_of_audio_output() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, _) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    player.command(TransportCommand::Play);
    moving_from(&player, 0);
    std::thread::sleep(Duration::from_millis(300));
    let (_, before) = player.position();
    // A different device, at a different rate — as unplugging headphones
    // onto 44.1 kHz speakers is.
    let other: Captured = Arc::new(Mutex::new(Vec::new()));
    player.switch_audio(&AudioChoice::Capture {
        sample_rate: 44_100,
        samples: Arc::clone(&other),
    });
    // It carries on from where it was — not from the start, not ahead…
    let (_, at_switch) = player.position();
    assert!(
        at_switch >= before && at_switch < before + 300_000,
        "the switch moved playback from {before} to {at_switch}"
    );
    // …and keeps real time once the new output plays.
    let rate = clock_rate(&player, Duration::from_millis(700));
    let status = player.status();
    player.close();
    assert_eq!(status.state, PlaybackState::Playing);
    assert!(
        (rate - 1.0).abs() < 0.15,
        "after the switch the clock ran at {rate}x"
    );
    assert!(
        other.lock().expect("other").iter().any(|s| s.abs() > 1e-6),
        "the new output plays sound"
    );
}

#[test]
fn with_no_audio_output_playback_runs_in_silence() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let player = Player::new(
        orchestrator.clone(),
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
        &PlayerOptions {
            audio: AudioChoice::Silent,
            max_width: 160,
            max_height: 90,
            default_device: DefaultDevice::System,
            models: None,
        },
    );
    let status = player.command(TransportCommand::Play);
    assert!(matches!(
        status.audio,
        blinkify_engine::audio::AudioOutputState::Silent { .. }
    ));
    let rate = clock_rate(&player, Duration::from_millis(600));
    player.close();
    assert!(
        (rate - 1.0).abs() < 0.15,
        "in silence the clock ran at {rate}x"
    );
}

#[test]
fn under_load_frames_drop_and_audio_carries_on() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let (player, captured) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    player.command(TransportCommand::Play);
    moving_from(&player, 0);
    let started = Instant::now();
    let mut after = 0;
    // A renderer that can only manage four frames a second.
    while started.elapsed() < Duration::from_millis(2200) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(10)) {
            after = frame.seq;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let heard = capture_at_least(&captured, 2 * RATE as usize);
    let stats = player.stats();
    player.close();
    assert!(stats.dropped_frames > 30, "{stats:?}");
    // Two seconds of tone with no silence in it: a 440 Hz sine is never at
    // zero for more than a sample or two.
    let played = &heard[first_sound(&heard)..first_sound(&heard) + 2 * 2 * RATE as usize];
    let longest_silence = played
        .chunks(2)
        .fold((0, 0), |(run, longest), frame| {
            if frame[0].abs() < 1e-4 {
                (run + 1, longest.max(run + 1))
            } else {
                (0, longest)
            }
        })
        .1;
    assert!(
        longest_silence < 8,
        "audio stalled for {longest_silence} samples"
    );
}

/// A/V drift under a frame over ten minutes (#28, Epic #4).
///
/// Real time by construction, so it runs in the heavy workflow.
#[test]
#[ignore = "ten minutes of real-time playback; runs in heavy.yml"]
fn av_drift_stays_under_a_frame_over_ten_minutes() {
    let orchestrator = orchestrator();
    let dir = common::scratch("playback-drift");
    let path = dir.join("ten minutes with sound.mkv");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=640x360:rate=30:duration=610")
                .lavfi_input("sine=frequency=440:sample_rate=48000:duration=610")
                .option("-c:v", "mpeg4")
                .option("-q:v", "8")
                .option("-c:a", "flac")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("source");
    let (player, captured) = capturing_player(
        &orchestrator,
        PlaybackPlan::whole(source(&orchestrator, &path)).expect("plan"),
    );
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut after = 0;
    let mut worst: f64 = 0.0;
    while started.elapsed() < Duration::from_secs(600) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(50)) {
            after = frame.seq;
            worst = worst.max(seconds(frame.chosen_at - frame.position).abs());
        }
    }
    let (_, position) = player.position();
    let heard = captured.lock().expect("captured").len();
    player.close();
    let audio_seconds = (heard - first_sound(&captured.lock().expect("c"))) as f64 / 2.0 / 48_000.0;
    println!(
        "worst video-to-clock {worst} s; clock {} s; audio heard {audio_seconds} s",
        seconds(position)
    );
    assert!(worst < 1.0 / 30.0, "video drifted {worst} s from the clock");
    assert!(
        (seconds(position) - audio_seconds).abs() < 1.0 / 30.0,
        "the clock drifted from the audio heard"
    );
}
