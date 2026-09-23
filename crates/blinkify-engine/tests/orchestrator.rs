//! The orchestrator against the real sidecar: progress, cancellation, stderr
//! floods, hostile file names, concurrency, and processes that must not
//! survive.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::orchestrator::{
    JobError, JobOptions, Limits, Orchestrator, Priority, Progress, SidecarCommand,
};

fn orchestrator(limits: Limits) -> Orchestrator {
    Orchestrator::new(common::sidecar(), limits)
}

const ROOMY: Limits = Limits {
    interactive: 4,
    shared: 4,
    background: 3,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("blinkify-orchestrator-tests")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A short real media file, written by the sidecar itself.
fn make_clip(orchestrator: &Orchestrator, path: &Path, seconds: u32) {
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!("testsrc2=size=320x240:rate=25:duration={seconds}"))
                .option("-c:v", "mjpeg")
                .output_file(path),
            Priority::Foreground,
        )
        .expect("clip written");
}

/// Whether a process with this id exists, asked of Windows rather than of
/// anything in this process.
fn process_exists(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn progress_arrives_throughout_and_reaches_one_hundred_percent_exactly_once() {
    let orchestrator = orchestrator(ROOMY);
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = Arc::clone(&seen);
    // Real-time input (`-re`) so the job lasts long enough to report often.
    let job = orchestrator.run(
        SidecarCommand::ffmpeg()
            .flag("-re")
            .lavfi_input("testsrc2=size=320x240:rate=25:duration=3")
            .output_null()
            .report_progress(Duration::from_secs(3)),
        Priority::Foreground,
        JobOptions::default().on_progress(move |p| sink.lock().expect("lock").push(p.progress)),
    );
    job.wait().expect("the job succeeds");

    let seen = seen.lock().expect("lock");
    let fractions: Vec<f64> = seen.iter().map(|p| p.fraction).collect();
    assert!(fractions.len() >= 4, "too few updates: {fractions:?}");
    assert!(
        fractions.windows(2).all(|w| w[0] <= w[1]),
        "went backwards: {fractions:?}"
    );
    let complete = fractions
        .iter()
        .filter(|&&f| (f - 1.0).abs() < f64::EPSILON)
        .count();
    assert_eq!(complete, 1, "{fractions:?}");
    assert!((fractions.last().copied().unwrap_or_default() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn a_failed_job_never_reports_one_hundred_percent() {
    let orchestrator = orchestrator(ROOMY);
    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let job = orchestrator.run(
        SidecarCommand::ffmpeg()
            .input(Path::new("C:/definitely/not/here.mp4"))
            .output_null()
            .report_progress(Duration::from_secs(3)),
        Priority::Foreground,
        JobOptions::default().on_progress(move |p| sink.lock().expect("lock").push(p.progress)),
    );
    assert!(job.wait().is_err());
    assert!(seen.lock().expect("lock").iter().all(|p| p.fraction < 1.0));
}

#[test]
fn cancelling_kills_the_process_within_a_second_and_releases_the_file() {
    let dir = scratch("cancel");
    let orchestrator = orchestrator(ROOMY);
    let input = dir.join("long input.avi");
    make_clip(&orchestrator, &input, 60);

    // `-re` reads at real time: this job would take a minute.
    let job = orchestrator.run(
        SidecarCommand::ffmpeg()
            .flag("-re")
            .input(&input)
            .output_null(),
        Priority::Background,
        JobOptions::default(),
    );
    let started = Instant::now();
    while job.pid().is_none() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    let pid = job.pid().expect("the process started");
    assert!(process_exists(pid));

    let cancelled_at = Instant::now();
    job.cancel();
    let result = job.wait();
    let took = cancelled_at.elapsed();

    assert!(matches!(result, Err(JobError::Cancelled)), "{result:?}");
    assert!(took < Duration::from_secs(1), "cancellation took {took:?}");
    assert!(!process_exists(pid), "ffmpeg {pid} survived cancellation");
    // On Windows a file open in another process cannot be deleted. If this
    // succeeds, nothing holds it.
    std::fs::remove_file(&input).expect("the input is no longer held open");
}

#[test]
fn cancelling_removes_the_partial_output() {
    let dir = scratch("partial");
    let orchestrator = orchestrator(ROOMY);
    let output = dir.join("partial.avi");
    let job = orchestrator.run(
        SidecarCommand::ffmpeg()
            .flag("-re")
            .lavfi_input("testsrc2=size=320x240:rate=25:duration=60")
            .option("-c:v", "mjpeg")
            .output_file(&output),
        Priority::Foreground,
        JobOptions::default().remove_on_failure(output.clone()),
    );
    let started = Instant::now();
    while !output.exists() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(output.exists(), "the job began writing");
    job.cancel();
    assert!(matches!(job.wait(), Err(JobError::Cancelled)));
    assert!(!output.exists(), "the partial output was left behind");
}

#[test]
fn a_flood_of_stderr_does_not_deadlock() {
    let orchestrator = orchestrator(ROOMY);
    // `showinfo` logs a long line per frame: 5 000 frames is megabytes of
    // stderr, far beyond a pipe buffer. Undrained, FFmpeg blocks forever.
    let started = Instant::now();
    let output = orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=160x120:rate=100:duration=50")
                .option("-vf", "showinfo")
                .output_null(),
            Priority::Foreground,
        )
        .expect("finishes");
    assert!(started.elapsed() < Duration::from_secs(120));
    assert!(
        output
            .stderr_tail
            .iter()
            .any(|line| line.contains("showinfo")),
        "{:?}",
        output.stderr_tail
    );
}

#[test]
fn file_names_with_quotes_spaces_unicode_and_dashes_round_trip() {
    let dir = scratch("names");
    let orchestrator = orchestrator(ROOMY);
    let names = [
        "plain.avi",
        "with space.avi",
        "it's a 'quoted' name.avi",
        "Işıq ğöç şəkil — ünicode.avi",
        "emoji 🎬🎞️.avi",
        "-starts-with-dash.avi",
        "semi;colon & ampersand %PATH% $HOME.avi",
    ];
    for name in names {
        let path = dir.join(name);
        make_clip(&orchestrator, &path, 1);
        let probe = orchestrator
            .run_to_end(
                SidecarCommand::ffprobe()
                    .option("-v", "error")
                    .option("-show_entries", "format=nb_streams")
                    .option("-of", "csv=p=0")
                    .input(&path),
                Priority::Interactive,
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "1", "{name}");
    }
}

#[test]
fn a_path_longer_than_max_path_opens() {
    let dir = scratch("long");
    let orchestrator = orchestrator(ROOMY);
    let mut deep = dir.clone();
    while deep.as_os_str().len() < 300 {
        deep.push("a-rather-long-directory-name-segment");
    }
    // Rust's std uses `\\?\` itself, so this creates the tree even past
    // MAX_PATH. The question is whether FFmpeg opens it.
    std::fs::create_dir_all(&deep).expect("deep tree");
    let path = deep.join("clip in a deep folder.avi");
    assert!(path.as_os_str().len() > 260);
    make_clip(&orchestrator, &path, 1);
    assert!(path.exists());
    orchestrator
        .run_to_end(
            SidecarCommand::ffprobe().option("-v", "error").input(&path),
            Priority::Interactive,
        )
        .expect("a long path opens");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_invalid_command_is_a_structured_error_not_a_panic() {
    let orchestrator = orchestrator(ROOMY);
    let error = orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .input(Path::new("C:/no/such/file.mp4"))
                .output_null(),
            Priority::Foreground,
        )
        .expect_err("fails");
    match error {
        JobError::Failed {
            command,
            exit_code,
            stderr_tail,
        } => {
            assert!(command.contains("file:C:/no/such/file.mp4"), "{command}");
            assert!(exit_code.is_some_and(|code| code != 0));
            assert!(
                stderr_tail.iter().any(|line| line.contains("No such file")),
                "{stderr_tail:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn the_concurrency_limits_hold_under_load() {
    let limits = Limits {
        interactive: 1,
        shared: 2,
        background: 1,
    };
    let orchestrator = orchestrator(limits);
    let job = |priority| {
        orchestrator.run(
            SidecarCommand::ffmpeg()
                .flag("-re")
                .lavfi_input("testsrc2=size=320x240:rate=25:duration=2")
                .output_null(),
            priority,
            JobOptions::default(),
        )
    };
    let jobs: Vec<_> = (0..4)
        .map(|_| job(Priority::Background))
        .chain((0..3).map(|_| job(Priority::Foreground)))
        .chain((0..2).map(|_| job(Priority::Interactive)))
        .collect();
    for job in jobs {
        job.wait().expect("each job finishes");
    }
    assert!(orchestrator.peak(Priority::Background) <= 1);
    assert!(orchestrator.peak(Priority::Foreground) <= 2);
    assert!(orchestrator.peak(Priority::Interactive) <= 1);
    assert_eq!(orchestrator.peak(Priority::Interactive), 1);
}

#[test]
fn shutdown_leaves_no_process_behind() {
    let orchestrator = orchestrator(ROOMY);
    let jobs: Vec<_> = (0..3)
        .map(|_| {
            orchestrator.run(
                SidecarCommand::ffmpeg()
                    .flag("-re")
                    .lavfi_input("testsrc2=size=320x240:rate=25:duration=60")
                    .output_null(),
                Priority::Background,
                JobOptions::default(),
            )
        })
        .collect();
    let started = Instant::now();
    while jobs.iter().filter(|job| job.pid().is_some()).count() < 3
        && started.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let pids: Vec<u32> = jobs
        .iter()
        .filter_map(blinkify_engine::orchestrator::Job::pid)
        .collect();
    assert_eq!(pids.len(), 3);

    orchestrator.shutdown(Duration::from_secs(5));
    for pid in pids {
        assert!(!process_exists(pid), "ffmpeg {pid} survived shutdown");
    }
    let refused = orchestrator.run_to_end(
        SidecarCommand::ffmpeg()
            .lavfi_input("testsrc2=duration=1")
            .output_null(),
        Priority::Interactive,
    );
    assert!(matches!(refused, Err(JobError::ShuttingDown)));
}

/// The child half of the unclean-shutdown test. Ignored in a normal run; the
/// test below starts this test binary again with only this test selected, and
/// then kills it the way Task Manager would.
#[test]
#[ignore = "run only as the child of an_unclean_exit_leaves_no_process_behind"]
fn unclean_exit_child() {
    let orchestrator = orchestrator(ROOMY);
    let job = orchestrator.run(
        SidecarCommand::ffmpeg()
            .flag("-re")
            .lavfi_input("testsrc2=size=320x240:rate=25:duration=120")
            .output_null(),
        Priority::Background,
        JobOptions::default(),
    );
    let started = Instant::now();
    while job.pid().is_none() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    // Through a file rather than stdout: the test harness decides when its
    // stdout is flushed, and the parent needs this line now.
    if let Some(report) = std::env::var_os("BLINKIFY_CHILD_PID_FILE") {
        std::fs::write(report, job.pid().expect("started").to_string()).expect("report pid");
    }
    // Block until killed.
    let _ = job.wait();
}

#[test]
fn an_unclean_exit_leaves_no_process_behind() {
    let report = scratch("unclean").join("pid.txt");
    let mut child = Command::new(std::env::current_exe().expect("test binary"))
        .args([
            "unclean_exit_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env("BLINKIFY_CHILD_PID_FILE", &report)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("child test process");
    let deadline = Instant::now() + Duration::from_secs(30);
    let pid = loop {
        if let Some(pid) = std::fs::read_to_string(&report)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "the child never reported its ffmpeg"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(process_exists(pid));

    // TerminateProcess on the parent: no destructor, no shutdown hook, no
    // cleanup code of ours runs. Only the job object can save us.
    child.kill().expect("kill the parent");
    let _ = child.wait();

    let deadline = Instant::now() + Duration::from_secs(3);
    while process_exists(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!process_exists(pid), "ffmpeg {pid} outlived its parent");
}

/// #22: "background work yields to interactive work — asserted by measuring
/// playback frame delivery while a full-file keyframe index runs".
///
/// Decode time of an interactive 1080p decode is measured on an idle machine,
/// then again while far more background indexing jobs are queued than there
/// are slots — each enumerating every packet of a two-hour file, which is what
/// the keyframe indexer (#24) does. The interactive decode must neither queue
/// behind them nor slow by more than 35 percent.
///
/// What this measured on an eight-core laptop, to keep the threshold honest:
/// 1.14x to 1.33x, and the same range with the background processes at normal
/// priority and with them confined to half the cores. The residue is memory
/// and cache contention that scheduling does not arbitrate; what the
/// orchestrator controls — admission and slots — is what keeps it at that and
/// not at "waits for the index to finish".
///
/// Timing-sensitive, so it runs in the heavy workflow rather than the
/// pull-request gate (`CLAUDE.md` section 13).
#[test]
#[ignore = "timing benchmark; runs in heavy.yml"]
fn background_work_yields_to_interactive_decode() {
    let dir = scratch("yield");
    let orchestrator = orchestrator(Limits::for_this_machine());

    let playback = dir.join("playback 1080p.avi");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=1920x1080:rate=30:duration=20")
                .option("-c:v", "mjpeg")
                .option("-q:v", "3")
                .output_file(&playback),
            Priority::Foreground,
        )
        .expect("playback clip");
    let long = dir.join("two hours.avi");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=160x90:rate=30:duration=7200")
                .option("-c:v", "mjpeg")
                .output_file(&long),
            Priority::Foreground,
        )
        .expect("long clip");

    // The fastest of three runs: a laptop's clock and thermal state move a
    // single measurement by more than the effect being measured, and the
    // minimum is the run least disturbed by anything but the load under test.
    let decode = || {
        (0..3)
            .map(|_| {
                let started = Instant::now();
                orchestrator
                    .run_to_end(
                        SidecarCommand::ffmpeg().input(&playback).output_null(),
                        Priority::Interactive,
                    )
                    .expect("decode");
                started.elapsed()
            })
            .min()
            .unwrap_or_default()
    };
    let idle = decode();

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let limits = Limits::for_this_machine();
    let queued = 3 * (limits.shared + limits.interactive);
    let indexers: Vec<_> = (0..queued)
        .map(|_| {
            let orchestrator = orchestrator.clone();
            let long = long.clone();
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    let _ = orchestrator.run_to_end(
                        SidecarCommand::ffprobe()
                            .option("-v", "error")
                            .option("-select_streams", "v:0")
                            .option("-show_entries", "packet=pts,dts,pos,flags")
                            .option("-of", "csv=p=0")
                            .input(&long),
                        Priority::Background,
                    );
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(1));
    let busy = decode();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    for indexer in indexers {
        let _ = indexer.join();
    }

    let ratio = busy.as_secs_f64() / idle.as_secs_f64();
    println!("interactive decode: idle {idle:?}, while indexing {busy:?} ({ratio:.2}x)");
    assert!(
        ratio < 1.35,
        "interactive decode slowed {ratio:.2}x while a keyframe index ran"
    );
}
