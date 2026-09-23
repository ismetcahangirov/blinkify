//! Frame-accurate seek and scrub against the real sidecar (#29): the frame
//! shown for a timestamp is the right one by content — on B-frame, open-GOP,
//! variable-frame-rate and edit-list sources — a storm of scrub positions is
//! coalesced, prefetch stays inside its bound, and a proxy shows the same
//! source frame the original would.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::similar_names,
    clippy::integer_division,
    clippy::needless_range_loop
)]

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::cache::Cache;
use blinkify_engine::decode::FrameSize;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{
    CancelToken, Flow, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, Player, PlayerOptions, ShownFrame, SourceMedia,
    TransportCommand,
};
use blinkify_engine::probe::{MediaInfo, Prober};
use blinkify_engine::proxy::Proxies;
use blinkify_engine::seek;
use blinkify_engine::time::{self, MICROSECONDS, Rounding};

const SMALL: FrameSize = FrameSize {
    width: 160,
    height: 90,
};

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

fn source(orchestrator: &Orchestrator, path: &Path) -> SourceMedia {
    let info = probe(orchestrator, path);
    let index = index(orchestrator, path, &info);
    SourceMedia::new(path, info, index).expect("playable")
}

fn player(orchestrator: &Orchestrator, source: SourceMedia) -> Player {
    Player::new(
        orchestrator.clone(),
        PlaybackPlan::whole(Arc::new(source)).expect("plan"),
        &PlayerOptions {
            audio: AudioChoice::Silent,
            max_width: 160,
            max_height: 90,
            default_device: DefaultDevice::System,
        },
    )
}

/// Every shown frame's presentation timestamp, as `ffprobe` decodes them.
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

/// The frame at `pts`, decoded independently from the start of the file:
/// every frame, no seek, the same scaling.
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

/// Timestamps to seek to: on frames, between frames, on keyframes, before
/// the first frame and in the last GOP — spread over the file.
fn targets(frames: &[i64]) -> Vec<i64> {
    let n = frames.len();
    let mut targets = vec![frames[0] - 1, frames[0], frames[n - 1]];
    for k in [3, n / 5, n / 3, n / 2, 2 * n / 3, n - 7] {
        targets.push(frames[k]);
        // Halfway to the next frame: the frame on screen is still this one.
        targets.push(frames[k] + (frames[k + 1] - frames[k]) / 2);
    }
    targets
}

/// The frame on screen at `pts`: the newest at or before it, or the first.
fn expected(frames: &[i64], pts: i64) -> i64 {
    frames
        .iter()
        .rev()
        .find(|frame| **frame <= pts)
        .copied()
        .unwrap_or(frames[0])
}

#[test]
fn every_timestamp_seeks_to_the_frame_it_shows_by_content() {
    let orchestrator = orchestrator();
    for name in [
        "h264-high-closed-gop.mp4",
        "hevc-open-gop.mp4",
        "hevc-closed-gop-radl.mp4",
        "vfr-screen.mp4",
        "edit-list.mp4",
        "vp9.webm",
    ] {
        let path = common::corpus(name);
        let info = probe(&orchestrator, &path);
        let index = index(&orchestrator, &path, &info);
        let stream = index.streams().next().expect("a video stream");
        let frames = reference_pts(&orchestrator, &path);
        for target in targets(&frames) {
            let want = expected(&frames, target);
            let plan = seek::plan(&index, stream, target).expect("plan");
            assert_eq!(plan.frame, want, "{name}: the plan for {target}");
            assert!(plan.keyframe.pts <= want || plan.from_start, "{name}");
            let frame = seek::decode_frame(&orchestrator, &index, &path, stream, target, SMALL)
                .expect("decode");
            assert_eq!(frame.pts, want, "{name}: seek to {target}");
            assert!(
                frame.pixels == reference_frame(&orchestrator, &path, want),
                "{name}: the frame shown for {target} is not the frame at {want}"
            );
        }
    }
}

/// The frame a seek, step or scrub landed on: once the player says it is no
/// longer resolving, the newest frame shown.
fn landed(player: &Player) -> Arc<ShownFrame> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.status().resolving {
        assert!(Instant::now() < deadline, "the frame never arrived");
        let _ = player.next_frame(u64::MAX, Duration::from_millis(10));
    }
    settle(player)
}

/// The newest frame shown, once nothing newer arrives for a moment.
fn settle(player: &Player) -> Arc<ShownFrame> {
    let mut last = player
        .next_frame(0, Duration::from_secs(10))
        .expect("a frame");
    while let Some(newer) = player.next_frame(last.seq, Duration::from_millis(200)) {
        last = newer;
    }
    last
}

#[test]
fn the_player_seeks_to_the_exact_frame_and_past_the_end_to_the_last() {
    let orchestrator = orchestrator();
    let path = common::corpus("hevc-open-gop.mp4");
    let source = source(&orchestrator, &path);
    let tb = source.time_base();
    let frames = reference_pts(&orchestrator, &path);
    let player = player(&orchestrator, source);
    for k in [70, 11, 45, 100] {
        let position = time::rescale(frames[k], tb, MICROSECONDS, Rounding::Up).expect("µs");
        player.command(TransportCommand::Seek { position });
        let shown = landed(&player);
        assert_eq!(shown.picture.as_ref().expect("picture").pts, frames[k]);
    }
    let status = player.command(TransportCommand::Seek {
        position: 3_600_000_000,
    });
    assert!(status.position < status.duration);
    let shown = landed(&player);
    assert_eq!(
        shown.picture.as_ref().expect("picture").pts,
        *frames.last().expect("frames"),
        "a seek past the end shows the last frame"
    );
    assert!(!player.status().resolving);
    player.close();
}

#[test]
fn a_storm_of_scrub_positions_is_coalesced_and_ends_on_the_last() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let frames = reference_pts(&orchestrator, &path);
    let source = source(&orchestrator, &path);
    let tb = source.time_base();
    let player = player(&orchestrator, source);
    // A drag across the whole clip and back: 400 positions in about 400 ms.
    let positions: Vec<i64> = (0..400)
        .map(|i| {
            let fraction = if i < 200 { i } else { 400 - i };
            fraction * 3_900_000 / 200
        })
        .collect();
    for position in &positions {
        player.command(TransportCommand::Scrub {
            position: *position,
        });
        std::thread::sleep(Duration::from_millis(1));
    }
    let last = *positions.last().expect("positions");
    let deadline = Instant::now() + Duration::from_secs(10);
    while player.status().resolving {
        assert!(
            Instant::now() < deadline,
            "the last position never resolved"
        );
        let _ = player.next_frame(u64::MAX, Duration::from_millis(20));
    }
    let shown = settle(&player);
    let want = expected(
        &frames,
        time::rescale(last, MICROSECONDS, tb, Rounding::Down).expect("ticks"),
    );
    assert_eq!(shown.picture.as_ref().expect("picture").pts, want);
    let (requests, decodes) = player.scrub_counts();
    assert_eq!(requests, 400);
    assert!(
        decodes * 5 < requests,
        "{decodes} windows decoded for {requests} positions: not coalesced"
    );
    player.close();
}

#[test]
fn prefetch_while_scrubbing_stays_inside_its_bound() {
    let orchestrator = orchestrator();
    let dir = common::scratch("seek-prefetch-bound");
    let path = dir.join("1080p intra.avi");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=1920x1080:rate=30:duration=20")
                .option("-c:v", "mjpeg")
                .option("-q:v", "8")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("clip");
    let player = Player::new(
        orchestrator.clone(),
        PlaybackPlan::whole(Arc::new(source(&orchestrator, &path))).expect("plan"),
        &PlayerOptions {
            audio: AudioChoice::Silent,
            max_width: 1920,
            max_height: 1080,
            default_device: DefaultDevice::System,
        },
    );
    let mut worst = 0;
    // Forwards then backwards, slowly enough that windows complete.
    for step in (0..60).chain((0..60).rev()) {
        player.command(TransportCommand::Scrub {
            position: step * 300_000,
        });
        std::thread::sleep(Duration::from_millis(40));
        let (bytes, budget) = player.scrub_cache_bytes();
        assert!(bytes <= budget, "{bytes} bytes cached against {budget}");
        worst = worst.max(bytes);
    }
    let (_, budget) = player.scrub_cache_bytes();
    println!("scrub cache peaked at {worst} of {budget} bytes");
    assert!(worst > 0, "nothing was prefetched");
    player.close();
}

#[test]
fn a_proxy_shows_the_same_source_frame_the_original_would() {
    let orchestrator = orchestrator();
    let path = common::corpus("h264-high-closed-gop.mp4");
    let dir = common::scratch("seek-proxy");
    let info = probe(&orchestrator, &path);
    let proxy = Proxies::new(orchestrator.clone(), Cache::new(dir, u64::MAX))
        .generate(&path, &info, &CancelToken::default(), |_| {})
        .expect("proxy");
    let frames = reference_pts(&orchestrator, &path);
    let source = source(&orchestrator, &path).with_proxy(Some(proxy));
    let tb = source.time_base();
    let player = player(&orchestrator, source);
    assert!(
        player.status().proxy,
        "the status says the picture is a proxy"
    );
    for k in [0, 7, 21, 60, 95, 118] {
        let position = time::rescale(frames[k], tb, MICROSECONDS, Rounding::Up).expect("µs");
        player.command(TransportCommand::Seek { position });
        let shown = landed(&player);
        assert_eq!(
            shown.picture.as_ref().expect("picture").pts,
            frames[k],
            "the proxy showed a different source frame"
        );
        assert_eq!(shown.rotation, 0, "a proxy is made upright");
    }
    // Stepping through the proxy lands on every source frame in turn.
    player.command(TransportCommand::JumpToStart);
    let _ = landed(&player);
    for k in 1..=10 {
        player.command(TransportCommand::Step { frames: 1 });
        assert_eq!(
            landed(&player).picture.as_ref().expect("picture").pts,
            frames[k]
        );
    }
    player.close();
}

#[test]
fn a_seek_on_a_sparse_source_says_it_is_resolving_until_its_frame_is_up() {
    let orchestrator = orchestrator();
    let dir = common::scratch("seek-sparse");
    let path = dir.join("one gop.mkv");
    // One keyframe for the whole clip: a seek near the end decodes it all.
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=1280x720:rate=30:duration=12")
                .option("-c:v", "mpeg4")
                .option("-g", "1000")
                .option("-q:v", "4")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("clip");
    let player = player(&orchestrator, source(&orchestrator, &path));
    let _ = settle(&player);
    let started = Instant::now();
    let status = player.command(TransportCommand::Seek {
        position: 11_500_000,
    });
    assert!(status.resolving, "the seek did not say it was resolving");
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.status().resolving {
        assert!(Instant::now() < deadline, "it never resolved");
        let _ = player.next_frame(u64::MAX, Duration::from_millis(20));
    }
    let latency = started.elapsed();
    let shown = settle(&player);
    println!("sparse seek resolved in {latency:?}");
    assert!(shown.position >= 11_466_000 && shown.position <= 11_500_000);
    player.close();
}

/// Seek latency (#29): imperceptible on a keyframe-dense source, bounded on
/// a sparse one. Timing, so it runs in the heavy workflow.
#[test]
#[ignore = "timing benchmark; runs in heavy.yml"]
fn seek_latency_is_imperceptible_on_dense_keyframes_and_bounded_on_sparse() {
    let orchestrator = orchestrator();
    let dir = common::scratch("seek-latency");
    let dense = dir.join("dense.avi");
    let sparse = dir.join("sparse.mkv");
    for (path, codec, gop) in [(&dense, "mjpeg", "1"), (&sparse, "mpeg4", "300")] {
        orchestrator
            .run_to_end(
                SidecarCommand::ffmpeg()
                    .lavfi_input("testsrc2=size=1920x1080:rate=30:duration=30")
                    .option("-c:v", codec)
                    .option("-g", gop)
                    .option("-q:v", "5")
                    .output_file(path),
                Priority::Foreground,
            )
            .expect("clip");
    }
    let measure = |path: &Path| {
        let player = Player::new(
            orchestrator.clone(),
            PlaybackPlan::whole(Arc::new(source(&orchestrator, path))).expect("plan"),
            &PlayerOptions {
                audio: AudioChoice::Silent,
                max_width: 960,
                max_height: 540,
                default_device: DefaultDevice::System,
            },
        );
        let _ = settle(&player);
        let mut latencies: Vec<Duration> = [23.4, 3.1, 17.9, 8.2, 28.8, 12.6, 1.4]
            .iter()
            .map(|seconds| {
                let started = Instant::now();
                player.command(TransportCommand::Seek {
                    position: (seconds * 1_000_000.0) as i64,
                });
                while player.status().resolving {
                    let _ = player.next_frame(u64::MAX, Duration::from_millis(5));
                }
                started.elapsed()
            })
            .collect();
        player.close();
        latencies.sort();
        latencies[latencies.len() / 2]
    };
    let dense_median = measure(&dense);
    let sparse_median = measure(&sparse);
    println!("median seek: dense {dense_median:?}, sparse {sparse_median:?}");
    assert!(
        dense_median < Duration::from_millis(250),
        "{dense_median:?}"
    );
    assert!(sparse_median < Duration::from_secs(5), "{sparse_median:?}");
}
