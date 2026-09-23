//! The keyframe index against `ffprobe`'s own packet listing, and against the
//! behaviours the issue names: laziness, persistence, open GOPs, VFR,
//! concurrency, and a two-hour file that must not block anything.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use blinkify_engine::cache::Cache;
use blinkify_engine::keyframes::{GopKind, KeyframeIndex, PictureKind};
use blinkify_engine::orchestrator::{Limits, Orchestrator, Priority, SidecarCommand};
use blinkify_engine::probe::Prober;

const CORPUS_VIDEO: [&str; 11] = [
    "h264-high-closed-gop.mp4",
    "h264-open-gop.mp4",
    "hevc-open-gop.mp4",
    "hevc-closed-gop-radl.mp4",
    "hevc-hdr10.mp4",
    "portrait-phone.mp4",
    "vfr-screen.mp4",
    "edit-list.mp4",
    "multi-audio.mkv",
    "vp9.webm",
    "av1.mp4",
];

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

fn open(path: &Path, cache: Option<Cache>) -> KeyframeIndex {
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    KeyframeIndex::open(path, &info, orchestrator, cache).expect("open")
}

fn video_stream(index: &KeyframeIndex) -> u32 {
    index.streams().next().expect("a video stream")
}

/// `(pts, dts, pos)` of every keyframe, straight from a full `ffprobe` pass —
/// the reference the index must match exactly.
fn reference(path: &Path, stream: u32) -> Vec<(i64, Option<i64>, Option<u64>)> {
    let output = orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", stream.to_string())
                .option("-show_entries", "packet=pts,dts,pos,flags")
                .option("-of", "compact=p=0")
                .input(path),
            Priority::Foreground,
        )
        .expect("ffprobe");
    let mut keyframes: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains("flags=K"))
        .map(|line| {
            let field = |name: &str| {
                line.split('|')
                    .find_map(|f| f.strip_prefix(&format!("{name}=")))
                    .and_then(|v| v.parse::<i64>().ok())
            };
            (
                field("pts").expect("pts"),
                field("dts"),
                field("pos").and_then(|p| u64::try_from(p).ok()),
            )
        })
        .collect();
    keyframes.sort_unstable();
    keyframes.dedup();
    keyframes
}

fn indexed(index: &KeyframeIndex, stream: u32) -> Vec<(i64, Option<i64>, Option<u64>)> {
    index
        .keyframes(stream)
        .expect("keyframes")
        .iter()
        .map(|k| (k.pts, k.dts, k.pos))
        .collect()
}

#[test]
fn the_index_matches_ffprobe_exactly_across_the_corpus() {
    for name in CORPUS_VIDEO {
        let path = common::corpus(name);
        // Quarter-second regions: dozens of region boundaries per file, each
        // one a chance to drop or duplicate a keyframe.
        let index = open(&path, None).with_chunk_seconds(0.25);
        let stream = video_stream(&index);
        index.complete_in_background(|_| {}).expect("index");
        assert!(index.is_complete(), "{name}");
        assert_eq!(indexed(&index, stream), reference(&path, stream), "{name}");
    }
}

#[test]
fn an_index_built_from_scattered_queries_matches_one_built_in_order() {
    let path = common::corpus("h264-high-closed-gop.mp4");
    let index = open(&path, None).with_chunk_seconds(0.3);
    let stream = video_stream(&index);
    // Out of order, overlapping, and past the end.
    for seconds in [3.1, 0.2, 2.25, 1.0, 3.9, 9.0, 0.71] {
        let ticks = index.ticks(stream, seconds).expect("ticks");
        let _ = index.at_or_before(stream, ticks).expect("query");
    }
    index.complete_in_background(|_| {}).expect("index");
    assert_eq!(indexed(&index, stream), reference(&path, stream));
}

#[test]
fn queries_answer_at_or_before_at_or_after_and_exactly() {
    // Keyframes forced at 0, 0.7, 1.9, 2.2 and 3.5 s.
    let index = open(&common::corpus("h264-high-closed-gop.mp4"), None);
    let stream = video_stream(&index);
    let t = |seconds| index.ticks(stream, seconds).expect("ticks");

    let before = index
        .at_or_before(stream, t(2.0))
        .expect("q")
        .expect("some");
    assert_eq!(before.pts, t(1.9));
    let after = index.at_or_after(stream, t(2.0)).expect("q").expect("some");
    assert_eq!(after.pts, t(2.2));
    assert!(index.is_keyframe(stream, t(0.7)).expect("q"));
    assert!(!index.is_keyframe(stream, t(0.8)).expect("q"));
    assert_eq!(
        index.at_or_after(stream, t(3.6)).expect("q"),
        None,
        "none after the last"
    );
}

#[test]
fn a_vfr_source_keeps_its_real_non_uniform_timestamps() {
    let index = open(&common::corpus("vfr-screen.mp4"), None);
    let stream = video_stream(&index);
    index.complete_in_background(|_| {}).expect("index");
    // 90 kHz ticks, keyframes forced at 0, 1, 2.5 and 3.2 s — across a
    // 30 fps to 10 fps change. A frame-number model would put the last two
    // somewhere else entirely.
    let pts: Vec<i64> = index
        .keyframes(stream)
        .expect("k")
        .iter()
        .map(|k| k.pts)
        .collect();
    assert_eq!(pts, vec![0, 90_000, 225_000, 288_000]);
}

#[test]
fn open_and_closed_gops_are_told_apart() {
    let gops = |name: &str| {
        let index = open(&common::corpus(name), None);
        let stream = video_stream(&index);
        index.complete_in_background(|_| {}).expect("index");
        index.keyframes(stream).expect("keyframes")
    };

    let closed = gops("h264-high-closed-gop.mp4");
    assert!(closed.iter().all(|k| k.gop == GopKind::Closed));
    assert!(closed.iter().all(|k| k.picture == Some(PictureKind::Idr)));

    let h264_open = gops("h264-open-gop.mp4");
    assert!(
        h264_open
            .iter()
            .any(|k| k.gop == GopKind::Open && k.picture == Some(PictureKind::RecoveryPoint)),
        "{h264_open:#?}"
    );
    assert_eq!(
        h264_open[0].gop,
        GopKind::Closed,
        "the first picture is an IDR"
    );

    let hevc_open = gops("hevc-open-gop.mp4");
    assert!(
        hevc_open
            .iter()
            .any(|k| k.gop == GopKind::Open && k.picture == Some(PictureKind::Cra)),
        "{hevc_open:#?}"
    );

    // The hard case: IDR_W_RADL keyframes *do* have leading pictures, and are
    // still closed. Leading pictures alone would call these open and cost a
    // needless smart-cut at every one.
    let radl = gops("hevc-closed-gop-radl.mp4");
    assert!(radl.iter().any(|k| k.has_leading_pictures), "{radl:#?}");
    assert!(radl.iter().all(|k| k.gop == GopKind::Closed), "{radl:#?}");
}

#[test]
fn the_index_survives_a_restart_and_is_rebuilt_when_the_file_changes() {
    let dir = common::scratch("keyframes-cache");
    let cache = Cache::new(dir.join("cache"), 64 * 1024 * 1024);
    let path = dir.join("clip.mp4");
    std::fs::copy(common::corpus("h264-high-closed-gop.mp4"), &path).expect("copy");

    let first = open(&path, Some(cache.clone()));
    let stream = video_stream(&first);
    first.complete_in_background(|_| {}).expect("index");
    let before = indexed(&first, stream);
    drop(first);

    // "Restart": a new index, from the cache, reads nothing.
    let restored = open(&path, Some(cache.clone()));
    assert!(restored.is_complete());
    assert_eq!(indexed(&restored, stream), before);
    let _ = restored.at_or_before(stream, 12_345).expect("query");
    assert_eq!(
        restored.reads(),
        0,
        "a restored index must not re-read the file"
    );

    // A different file under the same name is indexed afresh.
    std::fs::copy(common::corpus("h264-open-gop.mp4"), &path).expect("replace");
    let rebuilt = open(&path, Some(cache));
    assert!(!rebuilt.is_complete());
    rebuilt.complete_in_background(|_| {}).expect("index");
    assert!(rebuilt.reads() > 0);
    assert_eq!(indexed(&rebuilt, stream), reference(&path, stream));
}

/// A two-hour file, generated once per test run: one keyframe every second
/// (all-intra, 1 fps, a 1/1 time base), so the answer to any query is known
/// in advance. Queries use fractions below one half, which the index rounds
/// down to the second they fall in.
fn two_hours() -> &'static PathBuf {
    static FILE: OnceLock<PathBuf> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = common::scratch("keyframes-two-hours").join("two hours.avi");
        orchestrator()
            .run_to_end(
                SidecarCommand::ffmpeg()
                    .lavfi_input("color=c=gray:size=64x36:rate=1:duration=7200")
                    .option("-c:v", "mjpeg")
                    .output_file(&path),
                Priority::Foreground,
            )
            .expect("two-hour file");
        path
    })
}

#[test]
fn querying_an_unindexed_region_reads_only_that_region() {
    let index = open(two_hours(), None);
    let stream = video_stream(&index);
    let started = Instant::now();
    let at = index.ticks(stream, 3600.3).expect("ticks");
    let keyframe = index.at_or_before(stream, at).expect("q").expect("some");
    let took = started.elapsed();

    assert_eq!(keyframe.pts, index.ticks(stream, 3600.0).expect("ticks"));
    assert_eq!(index.reads(), 1, "one region, not a full-file pass");
    assert!(
        index.fraction() < 0.05,
        "only the region was indexed: {}",
        index.fraction()
    );
    assert!(
        took < Duration::from_secs(5),
        "a region query took {took:?}"
    );
}

#[test]
fn two_simultaneous_queries_into_different_regions_both_answer() {
    let index = Arc::new(open(two_hours(), None));
    let stream = video_stream(&index);
    let query = |seconds: f64| {
        let index = Arc::clone(&index);
        std::thread::spawn(move || {
            let at = index.ticks(stream, seconds).expect("ticks");
            index
                .at_or_before(stream, at)
                .expect("q")
                .expect("some")
                .pts
        })
    };
    let (early, late) = (query(1000.4), query(6000.2));
    let early = early.join().expect("thread");
    let late = late.join().expect("thread");
    assert_eq!(early, index.ticks(stream, 1000.0).expect("ticks"));
    assert_eq!(late, index.ticks(stream, 6000.0).expect("ticks"));
}

#[test]
fn indexing_two_hours_in_the_background_never_blocks_a_query() {
    let index = Arc::new(open(two_hours(), None));
    let stream = video_stream(&index);
    let progress: Arc<std::sync::Mutex<Vec<f64>>> = Arc::default();
    let sink = Arc::clone(&progress);
    let background = blinkify_engine::keyframes::spawn_background(Arc::clone(&index), move |p| {
        sink.lock().expect("lock").push(p.fraction);
    });

    // While the whole file is being indexed, queries anywhere still answer
    // promptly: the background pass holds no lock while ffprobe runs.
    let mut slowest = Duration::ZERO;
    for seconds in [5000.2, 150.3, 7100.1, 2400.4] {
        let started = Instant::now();
        let at = index.ticks(stream, seconds).expect("ticks");
        let keyframe = index.at_or_before(stream, at).expect("q").expect("some");
        slowest = slowest.max(started.elapsed());
        assert_eq!(
            keyframe.pts,
            index.ticks(stream, seconds.floor()).expect("ticks")
        );
    }
    assert!(
        slowest < Duration::from_secs(5),
        "a query waited {slowest:?}"
    );

    background
        .join()
        .expect("thread")
        .expect("background index");
    assert!(index.is_complete());
    let progress = progress.lock().expect("lock");
    assert!(
        progress.len() > 10,
        "progress is reported as it goes: {}",
        progress.len()
    );
    assert!(progress.windows(2).all(|w| w[0] <= w[1]), "never backwards");
    assert!((progress.last().copied().unwrap_or_default() - 1.0).abs() < f64::EPSILON);
    assert_eq!(index.keyframes(stream).expect("k").len(), 7200);
}

/// Every shown frame's presentation timestamp, as `ffprobe` decodes them.
fn reference_frames(path: &Path) -> Vec<i64> {
    let output = orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", "v:0")
                .option("-show_entries", "frame=pts")
                .option("-of", "csv=p=0")
                .input(path),
            Priority::Foreground,
        )
        .expect("ffprobe frames");
    let mut pts: Vec<i64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
        .collect();
    pts.sort_unstable();
    pts
}

#[test]
fn the_frame_table_walks_every_shown_frame_in_both_directions() {
    // One-second regions, so the walk crosses region boundaries — where a
    // frame shown before the limit but decoded after it would go missing.
    for name in [
        "h264-high-closed-gop.mp4",
        "hevc-open-gop.mp4",
        "vfr-screen.mp4",
        "edit-list.mp4",
    ] {
        let path = common::corpus(name);
        let index = open(&path, None).with_chunk_seconds(1.0);
        let stream = video_stream(&index);
        let expected = reference_frames(&path);

        let mut forwards = vec![
            index
                .frame_at_or_before(stream, expected[0])
                .expect("index")
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: the first frame {} is not in the table",
                        expected[0]
                    )
                }),
        ];
        while let Some(next) = index
            .frame_after(stream, *forwards.last().expect("a frame"))
            .expect("index")
        {
            forwards.push(next);
        }
        assert_eq!(forwards, expected, "{name}: forwards");

        let mut backwards = vec![*expected.last().expect("frames")];
        while let Some(previous) = index
            .frame_before(stream, *backwards.last().expect("a frame"))
            .expect("index")
        {
            backwards.push(previous);
        }
        backwards.reverse();
        assert_eq!(backwards, expected, "{name}: backwards");

        // Between two frames, the frame shown is the earlier one.
        let between = expected[10] + (expected[11] - expected[10]).div_euclid(2);
        assert_eq!(
            index.frame_at_or_before(stream, between).expect("index"),
            Some(expected[10]),
            "{name}: between frames"
        );
    }
}
