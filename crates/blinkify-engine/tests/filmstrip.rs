//! Filmstrips against decoded frames at known timestamps, progressive sheets,
//! the cache, and cancellation.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::cache::Cache;
use blinkify_engine::filmstrip::{Filmstrips, SheetReady};
use blinkify_engine::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::probe::Prober;

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

/// A clip whose colour changes every second, predictably: second `k` has
/// red `50k mod 256`, green 128, blue `255 − 40k mod 256`.
fn stepped_clip(dir: &Path, name: &str, seconds: u32) -> PathBuf {
    let path = dir.join(name);
    orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!(
                    "color=c=black:s=320x180:r=25:d={seconds},geq=r='mod(floor(T)*50\\,256)':g='128':b='mod(255-floor(T)*40\\,256)'"
                ))
                .option("-c:v", "mjpeg")
                .option("-q:v", "2")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("clip");
    path
}

fn expected_colour(second: u32) -> [i32; 3] {
    let k = i32::try_from(second).expect("small");
    [
        (k * 50).rem_euclid(256),
        128,
        (255 - k * 40).rem_euclid(256),
    ]
}

/// Decode a sheet to RGB with the sidecar, and read one pixel.
fn pixel(sheet: &Path, width: u32, x: u32, y: u32) -> [i32; 3] {
    let decoded: Arc<Mutex<Vec<u8>>> = Arc::default();
    let sink = Arc::clone(&decoded);
    orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .input(sheet)
                .option("-f", "rawvideo")
                .option("-pix_fmt", "rgb24")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decode sheet");
    let bytes = decoded.lock().expect("lock");
    let at = ((y * width + x) * 3) as usize;
    [
        i32::from(bytes[at]),
        i32::from(bytes[at + 1]),
        i32::from(bytes[at + 2]),
    ]
}

#[test]
fn each_thumbnail_is_the_frame_at_its_timestamp() {
    let dir = common::scratch("filmstrip-frames");
    let clip = stepped_clip(&dir, "steps.avi", 8);
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&clip)
        .expect("probe");
    let strips = Filmstrips::new(orchestrator, Cache::new(dir.join("cache"), 1 << 30));
    let strip = strips
        .filmstrip(&clip, &info, 72, 1.0, None, |_| {})
        .expect("filmstrip");

    assert_eq!(strip.count, 8);
    assert_eq!((strip.tile_width, strip.tile_height), (128, 72));
    assert_eq!(
        strip.sheets.len(),
        1,
        "eight thumbnails fit one 10x10 sheet"
    );
    let sheet_width = strip.tile_width * strip.columns;
    for second in 0..8 {
        let (sheet, x, y) = strip.locate(second).expect("located");
        let actual = pixel(sheet, sheet_width, x + 64, y + 36);
        let expected = expected_colour(second);
        for channel in 0..3 {
            assert!(
                (actual[channel] - expected[channel]).abs() <= 12,
                "thumbnail {second}: {actual:?}, frame at {second}s is {expected:?}"
            );
        }
    }
}

#[test]
fn sheets_arrive_one_by_one_while_generation_runs() {
    let dir = common::scratch("filmstrip-progressive");
    let clip = stepped_clip(&dir, "long.avi", 250);
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&clip)
        .expect("probe");
    let strips = Filmstrips::new(orchestrator, Cache::new(dir.join("cache"), 1 << 30));
    let seen: Arc<Mutex<Vec<SheetReady>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let strip = strips
        .filmstrip(&clip, &info, 72, 1.0, None, move |sheet| {
            assert!(sheet.path.is_file(), "reported before it was written");
            sink.lock().expect("lock").push(sheet);
        })
        .expect("filmstrip");

    let seen = seen.lock().expect("lock");
    assert_eq!(strip.count, 250);
    assert_eq!(seen.len(), 3, "100 + 100 + 50 thumbnails");
    assert_eq!(
        seen.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        seen.iter().map(|s| s.thumbnails).collect::<Vec<_>>(),
        vec![100, 200, 250]
    );
}

#[test]
fn a_filmstrip_is_cached_and_regenerated_when_the_source_changes() {
    let dir = common::scratch("filmstrip-cache");
    let cache = Cache::new(dir.join("cache"), 1 << 30);
    let clip = dir.join("clip.avi");
    std::fs::copy(stepped_clip(&dir, "a.avi", 5), &clip).expect("copy");
    let orchestrator = orchestrator();
    let prober = Prober::new(orchestrator.clone());
    let strips = Filmstrips::new(orchestrator, cache.clone());

    let first = strips
        .filmstrip(
            &clip,
            &prober.probe(&clip).expect("probe"),
            72,
            1.0,
            None,
            |_| {},
        )
        .expect("filmstrip");
    let reported = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&reported);
    let again = strips
        .filmstrip(
            &clip,
            &prober.probe(&clip).expect("probe"),
            72,
            1.0,
            None,
            move |_| {
                *counter.lock().expect("lock") += 1;
            },
        )
        .expect("filmstrip");
    assert_eq!(first, again);
    assert_eq!(*reported.lock().expect("lock"), 0, "served from the cache");
    assert!(cache.size_on_disk() > 0);

    std::fs::copy(stepped_clip(&dir, "b.avi", 7), &clip).expect("replace");
    let changed = strips
        .filmstrip(
            &clip,
            &prober.probe(&clip).expect("probe"),
            72,
            1.0,
            None,
            |_| {},
        )
        .expect("filmstrip");
    assert_eq!(changed.count, 7);
    assert_ne!(changed.sheets, first.sheets);
}

#[test]
fn cancelling_a_filmstrip_stops_its_process() {
    let dir = common::scratch("filmstrip-cancel");
    // Ten minutes at a quarter-second interval: 2 400 thumbnails, 24 sheets,
    // so there is plenty left to cancel after the first.
    let clip = dir.join("long.avi");
    orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=160x90:rate=25:duration=600")
                .option("-c:v", "mjpeg")
                .output_file(&clip),
            Priority::Foreground,
        )
        .expect("clip");
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&clip)
        .expect("probe");
    let strips = Filmstrips::new(orchestrator, Cache::new(dir.join("cache"), 1 << 30));
    let cancel = CancelToken::default();
    let trigger = cancel.clone();
    let started = Instant::now();
    let result = strips.filmstrip(&clip, &info, 72, 0.25, Some(&cancel), move |_| {
        trigger.cancel();
    });
    assert!(
        matches!(
            result,
            Err(blinkify_engine::filmstrip::FilmstripError::Engine(
                JobError::Cancelled
            ))
        ),
        "{result:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(120));
    // The input is released: nothing holds it open.
    std::fs::remove_file(&clip).expect("the source is no longer open");
}
