//! The preview decode pipeline against the real sidecar (#27): real
//! timestamps, frame-exact starts, bounded memory under a throttled consumer,
//! frames dropped rather than shown late, rotation, teardown, and a source
//! that disappears. Presentation runs through the player (#28), with its
//! audio played into silence.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

mod common;

use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::decode::{DecodeRequest, FrameRing, FrameSize, VideoDecoder, VideoFrame};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{
    Flow, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::playback::{
    AudioChoice, PlaybackPlan, Player, PlayerOptions, ShownFrame, SourceMedia, TransportCommand,
};
use blinkify_engine::probe::{MediaInfo, Prober, Rational};

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

fn probe(orchestrator: &Orchestrator, path: &Path) -> Arc<MediaInfo> {
    Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe")
}

fn index(orchestrator: &Orchestrator, path: &Path, info: &MediaInfo) -> Arc<KeyframeIndex> {
    Arc::new(KeyframeIndex::open(path, info, orchestrator.clone(), None).expect("index"))
}

fn first_video(info: &MediaInfo) -> (u32, Rational, u32, u32) {
    let (stream, video) = info.video().next().expect("a video stream");
    (
        stream.index,
        stream.time_base.expect("time base"),
        video.width,
        video.height,
    )
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

/// The frame at `pts`, decoded independently: from the start of the file,
/// every frame, no seek, no scale — the reference a seek must match.
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
                .option("-vf", format!("select=eq(pts\\,{pts}),format=rgba"))
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
        .expect("reference decode");
    bytes.lock().expect("bytes").clone()
}

/// Run a decode to its end and collect every frame.
fn decode_all(orchestrator: &Orchestrator, request: &DecodeRequest) -> Vec<VideoFrame> {
    let ring = Arc::new(FrameRing::new(1024));
    let decoder = VideoDecoder::start(orchestrator, request, Arc::clone(&ring));
    let deadline = Instant::now() + Duration::from_secs(60);
    while decoder.end().is_none() {
        assert!(Instant::now() < deadline, "decode did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        decoder.end(),
        Some(blinkify_engine::decode::DecodeEnd::Finished)
    );
    // `take_due` hands back the newest due frame, so drain one at a time.
    let mut frames = Vec::new();
    while let Some(next) = ring.earliest_pts() {
        frames.push(ring.take_due(next).expect("the earliest frame is due"));
    }
    frames
}

/// Whether a process with this id exists, asked of Windows.
fn process_exists(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An all-intra clip, so every frame is a keyframe: long enough to play, made
/// by the sidecar itself.
fn intra_clip(orchestrator: &Orchestrator, path: &Path, size: &str, seconds: u32) {
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!("testsrc2=size={size}:rate=30:duration={seconds}"))
                .option("-c:v", "mjpeg")
                .option("-q:v", "5")
                .output_file(path),
            Priority::Foreground,
        )
        .expect("clip written");
}

#[test]
fn frames_carry_the_streams_own_timestamps() {
    let orchestrator = orchestrator();
    for name in [
        "h264-high-closed-gop.mp4",
        "vfr-screen.mp4",
        "edit-list.mp4",
        "hevc-open-gop.mp4",
    ] {
        let path = common::corpus(name);
        let info = probe(&orchestrator, &path);
        let (stream, time_base, _, _) = first_video(&info);
        let frames = decode_all(
            &orchestrator,
            &DecodeRequest {
                source: path.clone(),
                stream,
                time_base,
                seek_to: None,
                first_pts: i64::MIN,
                size: FrameSize {
                    width: 64,
                    height: 36,
                },
                max_frames: None,
                concat: false,
            },
        );
        let decoded: Vec<i64> = frames.iter().map(|f| f.pts).collect();
        assert_eq!(decoded, reference_pts(&orchestrator, &path), "{name}");
        assert!(
            frames
                .iter()
                .all(|f| f.pixels.len() == 64 * 36 * 4 && f.width == 64),
            "{name}: every frame is the requested size"
        );
    }
}

#[test]
fn a_decode_starts_at_exactly_the_requested_frame_and_matches_it_by_content() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let info = probe(&orchestrator, &path);
    let (stream, time_base, width, height) = first_video(&info);
    let index = index(&orchestrator, &path, &info);
    // Mid-GOP: the keyframe at 0.7 s, the target five frames later.
    let target = time_base_ticks(0.7 + 5.0 / 30.0, time_base);
    let keyframe = index
        .at_or_before(stream, target)
        .expect("index")
        .expect("a keyframe");
    assert!(keyframe.pts < target, "the target is not itself a keyframe");

    let frames = decode_all(
        &orchestrator,
        &DecodeRequest {
            source: path.clone(),
            stream,
            time_base,
            seek_to: Some(keyframe.pts),
            first_pts: target,
            size: FrameSize { width, height },
            max_frames: Some(1),
            concat: false,
        },
    );
    let [frame] = frames.as_slice() else {
        panic!("exactly one frame, got {}", frames.len());
    };
    assert_eq!(frame.pts, target);
    assert!(
        frame.pixels == reference_frame(&orchestrator, &path, target),
        "the seeked frame is not the frame at {target}"
    );
}

fn time_base_ticks(seconds: f64, time_base: Rational) -> i64 {
    (seconds / time_base.value().expect("time base")).round() as i64
}

#[test]
fn a_throttled_consumer_bounds_memory_and_stalls_the_decoder() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let info = probe(&orchestrator, &path);
    let (stream, time_base, width, height) = first_video(&info);
    let ring = Arc::new(FrameRing::new(4));
    let decoder = VideoDecoder::start(
        &orchestrator,
        &DecodeRequest {
            source: path,
            stream,
            time_base,
            seek_to: None,
            first_pts: i64::MIN,
            size: FrameSize { width, height },
            max_frames: None,
            concat: false,
        },
        Arc::clone(&ring),
    );
    // Nobody consumes. The four-second clip decodes in far less than this.
    wait_until("the ring to fill", Duration::from_secs(10), || {
        ring.stats().buffered_frames == 4
    });
    std::thread::sleep(Duration::from_millis(1500));
    let stalled = ring.stats();
    assert_eq!(stalled.buffered_frames, 4);
    assert_eq!(stalled.buffered_bytes, 4 * (width * height * 4) as usize);
    assert!(
        decoder.frames() <= 5,
        "the decoder ran on past a full ring: {} frames",
        decoder.frames()
    );
    assert!(
        decoder.end().is_none(),
        "the decoder is paused, not finished"
    );
    let pid = decoder.pid().expect("a process");
    assert!(
        process_exists(pid),
        "backpressure pauses FFmpeg, not kills it"
    );

    // Consuming one frame lets exactly the decoder move on.
    let first = ring.earliest_pts().expect("buffered");
    ring.take_due(first);
    wait_until("the decoder to resume", Duration::from_secs(5), || {
        ring.stats().decoded_frames == stalled.decoded_frames + 1
    });
    decoder.stop();
    assert!(!process_exists(pid));
}

/// The whole of `path` as a plan.
fn whole(orchestrator: &Orchestrator, path: &Path) -> PlaybackPlan {
    let info = probe(orchestrator, path);
    let index = index(orchestrator, path, &info);
    let source = SourceMedia::new(path, info, index).expect("playable");
    PlaybackPlan::whole(Arc::new(source)).expect("plan")
}

/// A paused player for `plan`, playing its audio into silence.
fn player_of(
    orchestrator: &Orchestrator,
    plan: PlaybackPlan,
    max_width: u32,
    max_height: u32,
) -> Player {
    Player::new(
        orchestrator.clone(),
        plan,
        &PlayerOptions {
            audio: AudioChoice::Silent,
            max_width,
            max_height,
        },
    )
}

fn player(orchestrator: &Orchestrator, path: &Path, max_width: u32, max_height: u32) -> Player {
    player_of(
        orchestrator,
        whole(orchestrator, path),
        max_width,
        max_height,
    )
}

fn seconds_of(frame: &ShownFrame) -> f64 {
    frame.position as f64 / 1_000_000.0
}

#[test]
fn a_consumer_that_cannot_keep_up_sees_frames_dropped_not_delayed() {
    let orchestrator = orchestrator();
    let dir = common::scratch("preview-drops");
    let path = dir.join("intra 30fps.avi");
    intra_clip(&orchestrator, &path, "320x180", 20);
    let player = player(&orchestrator, &path, 0, 0);
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut after = 0;
    let mut lateness = Vec::new();
    // A renderer managing five frames a second against a 30 fps source.
    while started.elapsed() < Duration::from_secs(3) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(100)) {
            after = frame.seq;
            lateness.push((frame.chosen_at - frame.position) as f64 / 1_000_000.0);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let stats = player.stats();
    assert!(
        stats.dropped_frames > 50,
        "late frames are dropped: {stats:?}"
    );
    // Every frame shown was the one due when it was chosen — less than a
    // frame behind the clock — never a backlog replayed.
    assert!(
        lateness.iter().all(|late| *late < 1.0 / 30.0),
        "a frame was shown late: {lateness:?}"
    );
    player.close();
}

#[test]
fn a_decoder_left_far_behind_restarts_ahead_of_the_clock() {
    let orchestrator = orchestrator();
    let dir = common::scratch("preview-resync");
    let path = dir.join("intra 30fps.avi");
    intra_clip(&orchestrator, &path, "320x180", 30);
    let player = player(&orchestrator, &path, 0, 0);
    player.command(TransportCommand::Play);
    // The renderer stalls — the ring fills, the decoder blocks, the clock
    // runs on — then asks again.
    std::thread::sleep(Duration::from_millis(2500));
    let first = player
        .next_frame(0, Duration::from_millis(500))
        .expect("a frame");
    let mut later = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = player.next_frame(first.seq, Duration::from_millis(100))
            && seconds_of(&frame) > 2.5
        {
            later = Some(frame);
            break;
        }
    }
    let later = later.expect("playback caught up with the clock");
    assert!(player.stats().resyncs >= 1, "{:?}", player.stats());
    assert!(later.position > first.position);
    player.close();
}

#[test]
fn closing_a_player_terminates_its_decode_processes() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let player = player(&orchestrator, &path, 320, 180);
    player.command(TransportCommand::Play);
    wait_until("a decoder to start", Duration::from_secs(5), || {
        !player.decoder_pids().is_empty()
    });
    let pids = player.decoder_pids();
    assert!(pids.iter().all(|pid| process_exists(*pid)));
    player.close();
    for pid in pids {
        assert!(!process_exists(pid), "decoder {pid} outlived its player");
    }
    assert!(player.decoder_pids().is_empty());
    assert!(player.next_frame(0, Duration::from_millis(50)).is_none());
}

#[test]
fn a_portrait_video_arrives_as_coded_and_turns_upright_by_its_rotation() {
    let orchestrator = orchestrator();
    let path = common::corpus("portrait-phone.mp4");
    let player = player(&orchestrator, &path, 0, 0);
    let shown = player
        .next_frame(0, Duration::from_secs(5))
        .expect("the first frame, paused");
    player.close();
    assert_eq!(shown.rotation, 90);
    let frame = shown.picture.as_ref().expect("a picture");
    assert_eq!(
        (frame.width, frame.height),
        (1280, 720),
        "delivered unrotated"
    );

    // The renderer's rule — rotate counter-clockwise by `rotation` — applied
    // here to the delivered frame must give exactly what FFmpeg's own
    // autorotation gives.
    let upright = rotate_counter_clockwise(frame, shown.rotation);
    let reference = {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&bytes);
        orchestrator
            .run(
                SidecarCommand::ffmpeg()
                    .option("-v", "error")
                    .input(&path)
                    .option("-map", "0:v:0")
                    .option("-vf", "format=rgba")
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
            .expect("autorotated decode");
        bytes.lock().expect("bytes").clone()
    };
    assert_eq!(upright.len(), reference.len());
    assert!(
        upright == reference,
        "rotating the delivered frame does not give the upright picture"
    );
}

/// Rotate an RGBA frame counter-clockwise by a quarter-turn multiple — the
/// same rule the renderer applies with a canvas transform.
fn rotate_counter_clockwise(frame: &VideoFrame, degrees: u32) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let turns = degrees.div_euclid(90) % 4;
    let out_w = if turns % 2 == 1 { h } else { w };
    let mut out = vec![0_u8; frame.pixels.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turns {
                0 => (x, y),
                1 => (y, w - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (h - 1 - y, x),
            };
            let from = (y * w + x) * 4;
            let to = (ny * out_w + nx) * 4;
            out[to..to + 4].copy_from_slice(&frame.pixels[from..from + 4]);
        }
    }
    out
}

#[test]
fn a_source_that_disappears_is_reported_rather_than_panicking() {
    let orchestrator = orchestrator();
    let dir = common::scratch("preview-deleted");
    let path = dir.join("going away.avi");
    intra_clip(&orchestrator, &path, "160x90", 6);

    // Probed and indexed while it exists, as a project would have it.
    let plan = whole(&orchestrator, &path);
    let playing = player_of(&orchestrator, plan.clone(), 0, 0);
    playing.command(TransportCommand::Play);
    let first = playing
        .next_frame(0, Duration::from_secs(2))
        .expect("a frame");
    // While a decoder holds it, Windows refuses the delete, and playback is
    // unaffected.
    assert!(std::fs::remove_file(&path).is_err());
    assert!(
        playing
            .next_frame(first.seq, Duration::from_secs(2))
            .is_some()
    );
    playing.close();

    // Once nothing holds it, it can go; previewing it then says so.
    std::fs::remove_file(&path).expect("deletable once nothing holds it");
    let orphan = player_of(&orchestrator, plan, 0, 0);
    wait_until("the error to surface", Duration::from_secs(5), || {
        let _ = orphan.next_frame(u64::MAX, Duration::from_millis(10));
        orphan.stats().error.is_some()
    });
    let error = orphan.stats().error.expect("an error");
    assert!(error.contains("no longer available"), "{error}");
    orphan.close();
}

/// Ten minutes of 1080p30 at full rate with stable memory (#27).
///
/// Real time by construction — it plays ten minutes — so it runs in the heavy
/// workflow, not the pull-request gate (`CLAUDE.md` section 13).
#[test]
#[ignore = "ten minutes of real-time playback; runs in heavy.yml"]
fn ten_minutes_of_1080p30_plays_at_full_rate_with_stable_memory() {
    let orchestrator = orchestrator();
    let dir = common::scratch("preview-ten-minutes");
    let path = dir.join("1080p30 ten minutes.avi");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=1920x1080:rate=30:duration=610")
                .option("-c:v", "mpeg4")
                .option("-q:v", "6")
                .option("-g", "60")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("source");
    let player = player(&orchestrator, &path, 1920, 1080);
    player.command(TransportCommand::Play);

    let started = Instant::now();
    let mut after = 0;
    let mut memory = Vec::new();
    let mut next_sample = Duration::from_secs(30);
    while started.elapsed() < Duration::from_secs(600) {
        if let Some(frame) = player.next_frame(after, Duration::from_millis(50)) {
            after = frame.seq;
            // What the shell does with it: one copy onto the wire.
            let wire = frame.wire();
            assert_eq!(wire.len(), 48 + 1920 * 1080 * 4);
        }
        if started.elapsed() >= next_sample {
            memory.push(working_set_bytes());
            next_sample += Duration::from_secs(30);
        }
    }
    let stats = player.stats();
    player.close();
    let presented_fps = stats.presented_frames as f64 / 600.0;
    println!("stats {stats:?}; presented {presented_fps:.2} fps; working set {memory:?}");
    assert!(
        presented_fps > 29.0,
        "not full rate: {presented_fps:.2} fps"
    );
    assert_eq!(stats.error, None);
    let first = *memory.first().expect("samples");
    let peak = *memory.iter().max().expect("samples");
    assert!(
        peak < first + 64 * 1024 * 1024,
        "memory grew: {first} to {peak} bytes"
    );
}

/// This process's working set, asked of Windows.
fn working_set_bytes() -> u64 {
    let output = Command::new("tasklist")
        .args([
            "/FI",
            &format!("PID eq {}", std::process::id()),
            "/NH",
            "/FO",
            "CSV",
        ])
        .output()
        .expect("tasklist runs");
    // "name","pid","session","#","12,345 K"
    let text = String::from_utf8_lossy(&output.stdout);
    let kilobytes: String = text
        .rsplit(",\"")
        .next()
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    kilobytes.parse::<u64>().expect("a memory figure") * 1024
}
