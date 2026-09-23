//! Preview proxies: when they are offered, what they are, how cancelling and
//! resuming behave, and — the rule that matters — that export never reads one.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use blinkify_engine::cache::Cache;
use blinkify_engine::orchestrator::{
    CancelToken, JobError, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::proxy::{
    MediaAsset, PROXY_HEIGHT, Proxies, ProxyError, ProxyReason, proxy_advice,
};

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

fn clip(dir: &Path, name: &str, size: &str, seconds: u32) -> PathBuf {
    let path = dir.join(name);
    orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!("testsrc2=size={size}:rate=25:duration={seconds}"))
                .option("-c:v", "mjpeg")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("clip");
    path
}

#[test]
fn a_proxy_is_offered_for_4k_and_not_for_720p() {
    let dir = common::scratch("proxy-advice");
    let prober = Prober::new(orchestrator());
    let uhd = prober
        .probe(&clip(&dir, "uhd.avi", "3840x2160", 1))
        .expect("probe");
    assert!(matches!(
        proxy_advice(&uhd).as_slice(),
        [
            ProxyReason::Resolution {
                width: 3840,
                height: 2160
            },
            ..
        ]
    ));
    let hd = prober
        .probe(&clip(&dir, "hd.avi", "1280x720", 1))
        .expect("probe");
    assert!(proxy_advice(&hd).is_empty());
}

#[test]
fn a_proxy_is_540_line_all_intra_segments_covering_the_source() {
    let dir = common::scratch("proxy-make");
    let source = clip(&dir, "source.avi", "1920x1080", 25);
    let orchestrator = orchestrator();
    let prober = Prober::new(orchestrator.clone());
    let info = prober.probe(&source).expect("probe");
    let proxies = Proxies::new(orchestrator, Cache::new(dir.join("proxies"), u64::MAX));
    let proxy = proxies
        .generate(&source, &info, &CancelToken::default(), |_| {})
        .expect("proxy");

    assert_eq!(proxy.segments.len(), 3, "10 + 10 + 5 seconds");
    assert!(proxy.segments[0].start.abs() < 0.05);
    assert!((proxy.segments[2].end - 25.0).abs() < 0.1);
    for pair in proxy.segments.windows(2) {
        assert!(
            (pair[1].start - pair[0].end).abs() < 0.05,
            "segments are contiguous"
        );
    }
    let segment = prober.probe(&proxy.segments[0].file).expect("probe proxy");
    let (stream, video) = segment.video().next().expect("video");
    assert_eq!(video.height, PROXY_HEIGHT);
    assert_eq!(video.width, 960);
    assert_eq!(stream.codec.as_deref(), Some("mjpeg"), "all-intra");
}

#[test]
fn cancelling_leaves_only_complete_segments_and_resuming_finishes_the_job() {
    let dir = common::scratch("proxy-cancel");
    let source = clip(&dir, "source.avi", "1920x1080", 90);
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&source)
        .expect("probe");
    let proxies = Proxies::new(orchestrator, Cache::new(dir.join("proxies"), u64::MAX));
    let proxy_dir = proxies.directory(&source).expect("dir");

    // Cancel as soon as the first segment is finished.
    let cancel = CancelToken::default();
    let watcher = {
        let cancel = cancel.clone();
        let dir = proxy_dir.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(120);
            while Instant::now() < deadline {
                let listed = std::fs::read_to_string(dir.join("list-00000.csv"))
                    .map_or(0, |text| text.lines().count());
                if listed >= 1 {
                    cancel.cancel();
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let result = proxies.generate(&source, &info, &cancel, |_| {});
    watcher.join().expect("watcher");
    assert!(
        matches!(result, Err(ProxyError::Engine(JobError::Cancelled))),
        "{result:?}"
    );

    // Every segment file left is one FFmpeg finished; the one it was writing
    // is gone.
    let listed = std::fs::read_to_string(proxy_dir.join("list-00000.csv")).expect("list");
    let segments: Vec<String> = std::fs::read_dir(&proxy_dir)
        .expect("dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("seg_"))
        .collect();
    assert!(
        !segments.is_empty(),
        "the finished segment is kept for resuming"
    );
    for segment in &segments {
        assert!(
            listed.contains(segment.as_str()),
            "{segment} is a partial file"
        );
    }

    // Resume: continues from the next segment, and covers the whole source.
    let proxy = proxies
        .generate(&source, &info, &CancelToken::default(), |_| {})
        .expect("resumed");
    assert_eq!(proxy.segments.len(), 9);
    for pair in proxy.segments.windows(2) {
        assert!((pair[1].start - pair[0].end).abs() < 0.05, "{pair:?}");
    }
    assert!((proxy.segments[8].end - 90.0).abs() < 0.1);
    std::fs::remove_file(&source).expect("nothing holds the source open");
}

/// #26: "export from a project using a proxy reads the original file —
/// asserted by a test". The export executor is Epic #6; what this proves is
/// that the only path an export can be given, `export_source()`, is the
/// original — by file access, not by comparing strings: every proxy file is
/// held open exclusively, so anything that tried to read one would fail.
#[test]
#[cfg(windows)]
fn the_export_path_never_opens_a_proxy_file() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = common::scratch("proxy-export");
    let source = clip(&dir, "footage.avi", "1920x1080", 12);
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&source)
        .expect("probe");
    let proxy = Proxies::new(
        orchestrator.clone(),
        Cache::new(dir.join("proxies"), u64::MAX),
    )
    .generate(&source, &info, &CancelToken::default(), |_| {})
    .expect("proxy");

    let mut asset = MediaAsset::new(source.clone());
    asset.attach_proxy(proxy.clone());
    assert!(asset.preview_source().is_proxy());

    // Lock every proxy file: share mode 0, so no other open succeeds.
    let locks: Vec<std::fs::File> = proxy
        .segments
        .iter()
        .map(|segment| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(&segment.file)
                .expect("lock")
        })
        .collect();
    // The lock is real: reading a proxy now fails.
    let blocked = orchestrator.run_to_end(
        SidecarCommand::ffprobe().input(&proxy.segments[0].file),
        Priority::Interactive,
    );
    assert!(blocked.is_err(), "the proxy lock does not hold");

    // A stream copy of the export source — what tier 1 is — succeeds, so it
    // touched no proxy.
    let export = asset.export_source();
    assert_eq!(export.path(), source.as_path());
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .input(export.path())
                .option("-c", "copy")
                .output_null(),
            Priority::Foreground,
        )
        .expect("the export source is readable while every proxy is locked");
    drop(locks);
}

/// #26: "a 4K source with a proxy scrubs smoothly; the same source without one
/// does not, demonstrably". A scrub is a seek and one decoded frame; this
/// measures it at scattered points in a long-GOP 4K source and in its proxy.
/// Timing, so it runs in the heavy workflow.
#[test]
#[ignore = "timing benchmark; runs in heavy.yml"]
fn a_proxy_makes_a_4k_source_scrubbable() {
    let dir = common::scratch("proxy-scrub");
    let source = dir.join("uhd-long-gop.mkv");
    // One GOP for the whole clip: every seek decodes from the start, which is
    // what a camera's long GOP does to a scrub.
    orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=3840x2160:rate=30:duration=8")
                .option("-c:v", "libsvtav1")
                .option("-preset", "12")
                .option("-g", "240")
                .output_file(&source),
            Priority::Foreground,
        )
        .expect("4K source");
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&source)
        .expect("probe");
    let proxy = Proxies::new(
        orchestrator.clone(),
        Cache::new(dir.join("proxies"), u64::MAX),
    )
    .generate(&source, &info, &CancelToken::default(), |_| {})
    .expect("proxy");

    let scrub = |path: &Path, at: f64| {
        let started = Instant::now();
        orchestrator
            .run_to_end(
                SidecarCommand::ffmpeg()
                    .option("-ss", format!("{at:.3}"))
                    .input(path)
                    .option("-frames:v", "1")
                    .output_null(),
                Priority::Interactive,
            )
            .expect("scrub");
        started.elapsed()
    };
    let points = [7.3, 2.1, 5.8, 0.9, 6.6];
    let median = |mut times: Vec<Duration>| {
        times.sort();
        times[times.len() >> 1]
    };
    let original = median(points.iter().map(|&t| scrub(&source, t)).collect());
    let proxied = median(
        points
            .iter()
            .map(|&t| scrub(&proxy.segments[0].file, t))
            .collect(),
    );
    println!("scrub, median of 5: original {original:?}, proxy {proxied:?}");
    assert!(
        original >= proxied * 3,
        "the proxy should scrub at least 3x faster: {original:?} vs {proxied:?}"
    );
}
