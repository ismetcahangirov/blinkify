//! Shared by the engine's integration tests.
//!
//! These tests run the real bundled sidecar, because the behaviour under test
//! is what FFmpeg actually does with a file. They fail — never skip — when it
//! is absent: a test that quietly passes on a machine without the thing it
//! tests is not a test. `pnpm sidecar:fetch` puts it in place.

#![allow(dead_code, clippy::expect_used, unreachable_pub)]

pub mod fixture;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blinkify_engine::Sidecar;
use blinkify_engine::orchestrator::{
    Flow, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};

/// The sidecar as `pnpm sidecar:fetch` leaves it in the source tree, or the
/// directory named by `BLINKIFY_SIDECAR_DIR`.
pub fn sidecar() -> Sidecar {
    let dir = std::env::var_os("BLINKIFY_SIDECAR_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src-tauri/binaries"),
        PathBuf::from,
    );
    Sidecar::in_dir(&dir).expect("the FFmpeg sidecar is missing; run `pnpm sidecar:fetch`")
}

/// A file from the generated test corpus. `pnpm corpus` makes it; the corpus
/// is never committed (`CLAUDE.md` forbidden behaviour 12).
pub fn corpus(name: &str) -> PathBuf {
    let dir = std::env::var_os("BLINKIFY_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus"),
        PathBuf::from,
    );
    let path = dir.join(name);
    assert!(
        path.is_file(),
        "{} is missing from the test corpus; run `pnpm corpus`",
        path.display()
    );
    path
}

/// A scratch directory for one test, emptied first.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("blinkify-engine-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// An orchestrator over the test sidecar.
pub fn orchestrator() -> Orchestrator {
    Orchestrator::new(sidecar(), Limits::for_this_machine())
}

/// One packet as `framemd5` lists it under `-c copy`: its presentation time
/// in its stream's ticks, and the MD5 of its payload — the hash boundary of
/// #45: payload in, container timestamps out.
#[derive(Debug, Clone, PartialEq)]
pub struct Hashed {
    pub pts: i64,
    pub md5: String,
}

pub fn packet_hashes(path: &Path, selector: &str) -> Vec<Hashed> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                // The stream's own timestamps, not shifted to start at zero.
                .flag("-copyts")
                .input(path)
                .option("-map", format!("0:{selector}"))
                .option("-c", "copy")
                .option("-f", "framemd5")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("framemd5");
    let text = String::from_utf8_lossy(&collected.lock().expect("lock")).into_owned();
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            Some(Hashed {
                pts: fields.get(2)?.parse().ok()?,
                md5: (*fields.get(5)?).to_owned(),
            })
        })
        .collect()
}

/// What FFmpeg writes to its error stream decoding every stream of `path`
/// in full: empty for a file that decodes cleanly. The exit code alone is not
/// enough — most decode errors are not fatal.
pub fn decode_errors(path: &Path) -> Vec<String> {
    let output = orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0")
                // Every frame on its own timestamp: the check is of the
                // decoders, not of converting a variable rate to a fixed one
                // for the null output.
                .option("-fps_mode", "passthrough")
                .option("-enc_time_base", "demux")
                .output_null(),
            Priority::Foreground,
        )
        .expect("decodes");
    output.stderr_tail
}

pub fn md5s(hashes: &[Hashed]) -> Vec<String> {
    hashes.iter().map(|h| h.md5.clone()).collect()
}

/// Every packet's presentation time in seconds, sorted: the timeline of a
/// stream, whatever order the packets are stored in.
pub fn packet_times(path: &Path, selector: &str) -> Vec<f64> {
    let output = orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", selector.to_owned())
                .option("-show_entries", "packet=pts_time")
                .option("-of", "csv=p=0")
                .input(path),
            Priority::Foreground,
        )
        .expect("ffprobe");
    let mut times: Vec<f64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').parse().ok())
        .collect();
    times.sort_by(f64::total_cmp);
    times
}

/// Where a stream ends, in seconds: its last packet's time plus duration.
pub fn stream_end(path: &Path, selector: &str) -> f64 {
    let output = orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", selector.to_owned())
                .option("-show_entries", "packet=pts_time,duration_time")
                .option("-of", "csv=p=0")
                .input(path),
            Priority::Foreground,
        )
        .expect("ffprobe");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',');
            let pts: f64 = fields.next()?.parse().ok()?;
            let length: f64 = fields.next()?.parse().ok()?;
            Some(pts + length)
        })
        .fold(0.0, f64::max)
}
