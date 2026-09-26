//! The background export queue (#51): order, cancellation, progress, the
//! snapshot a job exports, and what a relaunch finds after a crash — with a
//! scripted runner where the queue's own behaviour is under test, and with
//! the real executor where what reaches the disk and the process table is.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::missing_panics_doc
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportRequest, export, partial_path,
};
use blinkify_engine::export::plan::plan;
use blinkify_engine::export::queue::{
    ExportJob, ExportQueue, ExportSpec, ExportState, HISTORY_LIMIT, QueueError, Report, RunOutcome,
    Stage,
};
use blinkify_engine::orchestrator::{CancelToken, JobProgress, Orchestrator, Priority};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{Operation, Project, SequenceSettings, SourceRef, Track, TrackKind};
use blinkify_engine::proxy::MediaAsset;
use common::fixture::Source;
use sha2::{Digest, Sha256};

const LONG: Duration = Duration::from_secs(120);

/// A project of one video track over `path`.
fn project(name: &str, path: &Path, settings: SequenceSettings, tracks: Vec<Track>) -> Project {
    let mut project = Project::new(name, settings);
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(path.to_path_buf()).export_source()).expect("ref"),
    );
    project.sequence.tracks = tracks;
    project
}

/// A spec whose project is a stand-in: for runners that do not read it.
fn spec(name: &str, target: &Path) -> ExportSpec {
    ExportSpec {
        project: Project::new(name, SequenceSettings::default()),
        name: name.to_owned(),
        target: target.to_path_buf(),
        overwrite: false,
        audio: AudioTarget::default(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Poll until `condition` holds, or fail after `LONG`.
fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn state_of(queue: &ExportQueue, id: u64) -> ExportState {
    queue
        .jobs()
        .into_iter()
        .find(|job| job.id == id)
        .expect("the job")
        .state
}

/// A runner that holds each job until it is cancelled.
fn held(cancel: &CancelToken) -> Result<RunOutcome, String> {
    while !cancel.is_cancelled() {
        thread::sleep(Duration::from_millis(5));
    }
    Err("cancelled".to_owned())
}

#[test]
fn jobs_run_one_at_a_time_in_the_order_they_were_asked_for() {
    let order = Arc::new(Mutex::new(Vec::new()));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let queue = {
        let (order, active, peak) = (Arc::clone(&order), Arc::clone(&active), Arc::clone(&peak));
        ExportQueue::open(
            None,
            move |spec: &ExportSpec, _: &CancelToken, _: Report| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                lock(&order).push(spec.name.clone());
                thread::sleep(Duration::from_millis(40));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(RunOutcome { bytes: 7 })
            },
            |_: &ExportJob| {},
        )
    };
    let dir = common::scratch("queue-order");
    let ids: Vec<u64> = ["a", "b", "c"]
        .iter()
        .map(|name| {
            queue
                .submit(spec(name, &dir.join(format!("{name}.mp4"))))
                .expect("queued")
                .id
        })
        .collect();
    for &id in &ids {
        let job = queue.wait(id, LONG).expect("the job");
        assert_eq!(job.state, ExportState::Completed { bytes: 7 });
        assert!(job.started.is_some() && job.finished >= job.started);
    }
    assert_eq!(*lock(&order), vec!["a", "b", "c"]);
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}

#[test]
fn a_cancelled_job_stops_and_a_cancelled_queued_job_never_starts() {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let queue = {
        let ran = Arc::clone(&ran);
        ExportQueue::open(
            None,
            move |spec: &ExportSpec, cancel: &CancelToken, _: Report| {
                lock(&ran).push(spec.name.clone());
                held(cancel)
            },
            |_: &ExportJob| {},
        )
    };
    let dir = common::scratch("queue-cancel");
    let first = queue
        .submit(spec("first", &dir.join("1.mp4")))
        .expect("queued");
    let second = queue
        .submit(spec("second", &dir.join("2.mp4")))
        .expect("queued");
    until("the first job to start", || {
        matches!(state_of(&queue, first.id), ExportState::Running { .. })
    });

    queue.cancel(second.id).expect("cancel queued");
    assert_eq!(state_of(&queue, second.id), ExportState::Cancelled);
    queue.cancel(first.id).expect("cancel running");
    assert_eq!(
        queue.wait(first.id, LONG).expect("job").state,
        ExportState::Cancelled
    );
    assert_eq!(*lock(&ran), vec!["first"]);
    // A finished job is left as it is.
    queue.cancel(first.id).expect("no-op");
    assert_eq!(state_of(&queue, first.id), ExportState::Cancelled);
    assert_eq!(queue.cancel(99), Err(QueueError::NoSuchJob(99)));
}

#[test]
fn progress_reaches_the_observer_and_never_moves_backwards() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = {
        let seen = Arc::clone(&seen);
        ExportQueue::open(
            None,
            |_: &ExportSpec, _: &CancelToken, report: Report| {
                report(Stage::Preparing, 0.0);
                for fraction in [0.0, 0.2, 0.5, 0.3, 0.45, 0.9, 1.0] {
                    report(Stage::Exporting, fraction);
                }
                // A late report of the stage before changes nothing.
                report(Stage::Preparing, 0.7);
                Ok(RunOutcome { bytes: 1 })
            },
            move |job: &ExportJob| lock(&seen).push(job.state.clone()),
        )
    };
    let dir = common::scratch("queue-progress");
    let job = queue.submit(spec("p", &dir.join("p.mp4"))).expect("queued");
    queue.wait(job.id, LONG).expect("job");

    let seen = lock(&seen).clone();
    assert_eq!(seen.first(), Some(&ExportState::Queued));
    assert_eq!(seen.last(), Some(&ExportState::Completed { bytes: 1 }));
    let exporting: Vec<f64> = seen
        .iter()
        .filter_map(|state| match state {
            ExportState::Running {
                stage: Stage::Exporting,
                fraction,
                ..
            } => Some(*fraction),
            _ => None,
        })
        .collect();
    assert!(
        exporting.windows(2).all(|pair| pair[0] <= pair[1]),
        "{exporting:?}"
    );
    assert!((exporting.last().copied().unwrap_or_default() - 1.0).abs() < f64::EPSILON);
    // Once exporting, the stage never goes back.
    let first_export = seen
        .iter()
        .position(|s| {
            matches!(
                s,
                ExportState::Running {
                    stage: Stage::Exporting,
                    ..
                }
            )
        })
        .expect("exported");
    assert!(!seen[first_export..].iter().any(|s| matches!(
        s,
        ExportState::Running {
            stage: Stage::Preparing,
            ..
        }
    )));
}

#[test]
fn a_failure_is_recorded_with_its_reason() {
    let queue = ExportQueue::open(
        None,
        |_: &ExportSpec, _: &CancelToken, _: Report| Err("the disk is full".to_owned()),
        |_: &ExportJob| {},
    );
    let dir = common::scratch("queue-failure");
    let job = queue.submit(spec("f", &dir.join("f.mp4"))).expect("queued");
    assert_eq!(
        queue.wait(job.id, LONG).expect("job").state,
        ExportState::Failed {
            message: "the disk is full".to_owned()
        }
    );
}

#[test]
fn a_job_exports_the_project_as_it_was_when_it_was_asked_for() {
    let exported = Arc::new(Mutex::new(None));
    let queue = {
        let exported = Arc::clone(&exported);
        ExportQueue::open(
            None,
            move |spec: &ExportSpec, cancel: &CancelToken, _: Report| {
                *lock(&exported) = Some(spec.project.clone());
                held(cancel)
            },
            |_: &ExportJob| {},
        )
    };
    let dir = common::scratch("queue-snapshot");
    let source = dir.join("clip.mp4");
    std::fs::copy(common::corpus("h264-high-closed-gop.mp4"), &source).expect("copy");
    let mut editing = project("snapshot", &source, SequenceSettings::default(), Vec::new());
    let asked = ExportSpec {
        project: editing.clone(),
        ..spec("snapshot", &dir.join("out.mp4"))
    };
    let job = queue.submit(asked).expect("queued");
    // The user goes on editing while the export runs.
    editing.name = "renamed while exporting".to_owned();
    editing
        .sequence
        .tracks
        .push(Track::new(9, TrackKind::Video, Vec::new()));
    until("the job to start", || lock(&exported).is_some());
    let snapshot = lock(&exported).clone().expect("snapshot");
    assert_eq!(snapshot.name, "snapshot");
    assert!(snapshot.sequence.tracks.is_empty());
    queue.cancel(job.id).expect("cancel");
}

#[test]
fn the_target_is_never_a_source() {
    let dir = common::scratch("queue-target-is-source");
    let source = dir.join("clip.mp4");
    std::fs::copy(common::corpus("h264-high-closed-gop.mp4"), &source).expect("copy");
    let before = sha256(&source);

    // Refused when it is asked for, however the path is spelled.
    let queue = ExportQueue::open(
        None,
        |_: &ExportSpec, _: &CancelToken, _: Report| panic!("nothing may run"),
        |_: &ExportJob| {},
    );
    let project = project("p", &source, SequenceSettings::default(), Vec::new());
    for target in [source.clone(), dir.join(".").join("CLIP.MP4")] {
        let refused = queue.submit(ExportSpec {
            project: project.clone(),
            overwrite: true,
            ..spec("p", &target)
        });
        assert!(
            matches!(refused, Err(QueueError::TargetIsSource(_))),
            "{target:?}: {refused:?}"
        );
    }

    // And by the executor, whoever calls it, even with overwriting confirmed.
    let file = Source::at(source.clone(), None);
    let plan = file.plan_clips(vec![file.clip(1, 0, file.first(), file.end(), &[])]);
    let inputs = file.inputs();
    let result = export(
        &common::orchestrator(),
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &source,
            overwrite: true,
            audio: AudioTarget::default(),
            models: None,
            cancel: CancelToken::default(),
            on_progress: None,
        },
    );
    assert!(matches!(result, Err(ExportError::TargetIsSource(_))));
    assert_eq!(sha256(&source), before);
    assert!(!partial_path(&source).exists());
}

#[test]
fn a_relaunch_offers_an_interrupted_export_again_or_discards_it() {
    let dir = common::scratch("queue-relaunch");
    let store = dir.join("exports.json");
    let (first_target, second_target) = (dir.join("1.mp4"), dir.join("2.mp4"));

    // The first run: one job running and one waiting when the application
    // dies. `forget` is the crash: nothing is shut down, nothing is written.
    let crashed = ExportQueue::open(
        Some(store.clone()),
        |_: &ExportSpec, cancel: &CancelToken, _: Report| held(cancel),
        |_: &ExportJob| {},
    );
    let first = crashed
        .submit(spec("first", &first_target))
        .expect("queued");
    let second = crashed
        .submit(spec("second", &second_target))
        .expect("queued");
    until("the first job to start", || {
        matches!(state_of(&crashed, first.id), ExportState::Running { .. })
    });
    // What the muxer had written when the process died.
    std::fs::write(partial_path(&first_target), b"half an mp4").expect("partial");
    std::mem::forget(crashed);

    // The relaunch: both are offered, and neither runs until asked.
    let ran = Arc::new(Mutex::new(Vec::new()));
    let relaunched = {
        let ran = Arc::clone(&ran);
        ExportQueue::open(
            Some(store.clone()),
            move |spec: &ExportSpec, _: &CancelToken, _: Report| {
                // Exported again from the start: nothing of the last attempt
                // is left beside the target.
                assert!(!partial_path(&spec.target).exists());
                lock(&ran).push(spec.name.clone());
                Ok(RunOutcome { bytes: 3 })
            },
            |_: &ExportJob| {},
        )
    };
    let states: Vec<ExportState> = relaunched.jobs().into_iter().map(|j| j.state).collect();
    assert_eq!(
        states,
        vec![ExportState::Interrupted, ExportState::Interrupted]
    );
    thread::sleep(Duration::from_millis(100));
    assert!(lock(&ran).is_empty());

    relaunched.resume(first.id).expect("resume");
    assert_eq!(
        relaunched.wait(first.id, LONG).expect("job").state,
        ExportState::Completed { bytes: 3 }
    );
    std::fs::write(partial_path(&second_target), b"a quarter").expect("partial");
    let discarded = relaunched.discard(second.id).expect("discard");
    assert_eq!(discarded.state, ExportState::Cancelled);
    assert!(!partial_path(&second_target).exists());
    assert_eq!(*lock(&ran), vec!["first"]);
    assert_eq!(
        relaunched.resume(first.id),
        Err(QueueError::NotInterrupted(first.id))
    );
    drop(relaunched);

    // The history survives, without the projects it no longer needs.
    let again = ExportQueue::open(
        Some(store.clone()),
        |_: &ExportSpec, _: &CancelToken, _: Report| panic!("nothing may run"),
        |_: &ExportJob| {},
    );
    let states: Vec<ExportState> = again.jobs().into_iter().map(|j| j.state).collect();
    assert_eq!(
        states,
        // The resumed job was asked for again, so it is the later one.
        vec![ExportState::Cancelled, ExportState::Completed { bytes: 3 }]
    );
    let stored = std::fs::read_to_string(&store).expect("store");
    assert!(!stored.contains("\"project\""), "{stored}");
}

#[test]
fn an_orderly_shutdown_leaves_the_running_export_to_be_offered_again() {
    let dir = common::scratch("queue-shutdown");
    let store = dir.join("exports.json");
    let queue = ExportQueue::open(
        Some(store.clone()),
        |_: &ExportSpec, cancel: &CancelToken, _: Report| held(cancel),
        |_: &ExportJob| {},
    );
    let job = queue.submit(spec("s", &dir.join("s.mp4"))).expect("queued");
    until("the job to start", || {
        matches!(state_of(&queue, job.id), ExportState::Running { .. })
    });
    queue.shutdown();
    assert_eq!(state_of(&queue, job.id), ExportState::Interrupted);
    drop(queue);
    let relaunched = ExportQueue::open(
        Some(store),
        |_: &ExportSpec, _: &CancelToken, _: Report| Ok(RunOutcome { bytes: 0 }),
        |_: &ExportJob| {},
    );
    assert_eq!(state_of(&relaunched, job.id), ExportState::Interrupted);
}

#[test]
fn an_unreadable_store_is_set_aside_and_the_queue_starts_empty() {
    let dir = common::scratch("queue-unreadable");
    let store = dir.join("exports.json");
    std::fs::write(&store, b"{ not json").expect("write");
    let queue = ExportQueue::open(
        Some(store.clone()),
        |_: &ExportSpec, _: &CancelToken, _: Report| Ok(RunOutcome { bytes: 0 }),
        |_: &ExportJob| {},
    );
    assert!(queue.jobs().is_empty());
    assert_eq!(
        std::fs::read(dir.join("exports.json.unreadable")).expect("set aside"),
        b"{ not json"
    );
}

#[test]
fn the_history_is_bounded_and_can_be_cleared() {
    let queue = ExportQueue::open(
        None,
        |_: &ExportSpec, _: &CancelToken, _: Report| Ok(RunOutcome { bytes: 0 }),
        |_: &ExportJob| {},
    );
    let dir = common::scratch("queue-history");
    let mut last = 0;
    for n in 0..HISTORY_LIMIT + 5 {
        last = queue
            .submit(spec(&n.to_string(), &dir.join(format!("{n}.mp4"))))
            .expect("queued")
            .id;
    }
    queue.wait(last, LONG).expect("job");
    let jobs = queue.jobs();
    assert_eq!(jobs.len(), HISTORY_LIMIT);
    assert_eq!(jobs.last().map(|j| j.id), Some(last));
    assert_eq!(jobs.first().map(|j| j.name.as_str()), Some("5"));
    queue.clear_history();
    assert!(queue.jobs().is_empty());
}

// ── The real executor ────────────────────────────────────────────────────

/// The real tests share the process table, which one of them enumerates.
static REAL: Mutex<()> = Mutex::new(());

/// What the shell's runner does, over one corpus file: plan the job's own
/// project, then execute it with the job's cancel token and progress.
fn real_runner(
    file: Arc<Source>,
    orchestrator: Orchestrator,
) -> impl Fn(&ExportSpec, &CancelToken, Report) -> Result<RunOutcome, String> + Send + Sync {
    move |spec: &ExportSpec, cancel: &CancelToken, report: Report| {
        report(Stage::Preparing, 0.0);
        let timeline = evaluate(&spec.project).map_err(|e| e.to_string())?;
        let plan = plan(
            &timeline,
            &spec.project.sequence.settings,
            &BTreeMap::from([(1, file.facts.clone())]),
        )
        .map_err(|e| e.to_string())?;
        let path = spec.project.sources[&1].path().to_path_buf();
        let inputs = BTreeMap::from([(
            1,
            ExportInput {
                source: MediaAsset::new(path).export_source(),
                info: Arc::clone(&file.info),
            },
        )]);
        report(Stage::Exporting, 0.0);
        let forward = Arc::clone(&report);
        let outcome = export(
            &orchestrator,
            ExportRequest {
                plan: &plan,
                inputs: &inputs,
                target: &spec.target,
                overwrite: spec.overwrite,
                audio: spec.audio,
                models: None,
                cancel: cancel.clone(),
                on_progress: Some(Box::new(move |update: JobProgress| {
                    forward(Stage::Exporting, update.progress.fraction);
                })),
            },
        )
        .map_err(|e| e.to_string())?;
        let bytes = std::fs::metadata(&outcome.path)
            .map_err(|e| e.to_string())?
            .len();
        Ok(RunOutcome { bytes })
    }
}

/// A reversed clip: rendered a chunk at a time, slow enough to stop midway.
fn reversed(file: &Source, dir: &Path) -> (PathBuf, Project) {
    let path = dir.join("source.webm");
    std::fs::copy(&file.path, &path).expect("copy");
    let settings = SequenceSettings::matching(&file.video().geometry).expect("settings");
    let clip = file.clip(1, 0, file.first(), file.end(), &[Operation::Reverse]);
    let project = project(
        "reversed",
        &path,
        settings,
        vec![Track::new(1, TrackKind::Video, vec![clip])],
    );
    (path, project)
}

fn sha256(path: &Path) -> Vec<u8> {
    Sha256::digest(std::fs::read(path).expect("read")).to_vec()
}

/// The sidecar processes this test process has started and not reaped.
fn sidecar_children() -> usize {
    let filter = format!(
        "Get-CimInstance Win32_Process -Filter 'ParentProcessId={}' | \
         Where-Object {{ $_.Name -like 'ff*' }} | Measure-Object | ForEach-Object Count",
        std::process::id()
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &filter])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("a count")
}

#[test]
fn a_real_export_through_the_queue_leaves_the_source_untouched() {
    let _real = lock(&REAL);
    let file = Arc::new(Source::with_encoders("vp9.webm"));
    let dir = common::scratch("queue-real");
    let (source, project) = reversed(&file, &dir);
    let before = sha256(&source);
    let orchestrator = common::orchestrator();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let queue = {
        let seen = Arc::clone(&seen);
        ExportQueue::open(
            None,
            real_runner(Arc::clone(&file), orchestrator.clone()),
            move |job: &ExportJob| lock(&seen).push(job.state.clone()),
        )
    };
    let target = dir.join("out.mkv");
    let job = queue
        .submit(ExportSpec {
            project,
            audio: AudioTarget::Opus { kilobits: 128 },
            ..spec("real", &target)
        })
        .expect("queued");

    // The queue answers at once while the export runs.
    let mut slowest = Duration::ZERO;
    while !state_of(&queue, job.id).is_finished() {
        let asked = Instant::now();
        let _ = queue.jobs();
        slowest = slowest.max(asked.elapsed());
        thread::sleep(Duration::from_millis(2));
    }
    assert!(slowest < Duration::from_millis(100), "{slowest:?}");

    let ExportState::Completed { bytes } = state_of(&queue, job.id) else {
        panic!("{:?}", state_of(&queue, job.id));
    };
    assert_eq!(bytes, std::fs::metadata(&target).expect("output").len());
    assert!(!partial_path(&target).exists());
    assert_eq!(sha256(&source), before);
    let fractions: Vec<f64> = lock(&seen)
        .iter()
        .filter_map(|state| match state {
            ExportState::Running {
                stage: Stage::Exporting,
                fraction,
                ..
            } => Some(*fraction),
            _ => None,
        })
        .collect();
    assert!(fractions.len() > 1, "{fractions:?}");
    assert!(
        fractions.windows(2).all(|pair| pair[0] <= pair[1]),
        "{fractions:?}"
    );
}

#[test]
fn cancelling_a_real_export_removes_the_partial_file_and_every_process() {
    let _real = lock(&REAL);
    let file = Arc::new(Source::with_encoders("vp9.webm"));
    let dir = common::scratch("queue-real-cancel");
    let (source, project) = reversed(&file, &dir);
    let before = sha256(&source);
    let orchestrator = common::orchestrator();
    let queue = ExportQueue::open(
        None,
        real_runner(Arc::clone(&file), orchestrator.clone()),
        |_: &ExportJob| {},
    );
    let target = dir.join("out.mkv");
    let job = queue
        .submit(ExportSpec {
            project,
            audio: AudioTarget::Opus { kilobits: 128 },
            ..spec("cancelled", &target)
        })
        .expect("queued");
    // Midway: the muxer has begun writing.
    until("the export to be writing", || {
        partial_path(&target).exists() || state_of(&queue, job.id).is_finished()
    });
    assert!(
        !state_of(&queue, job.id).is_finished(),
        "the export finished before it could be cancelled"
    );
    queue.cancel(job.id).expect("cancel");

    assert_eq!(
        queue.wait(job.id, LONG).expect("job").state,
        ExportState::Cancelled
    );
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());
    until("the export's jobs to end", || {
        orchestrator.running(Priority::Export) == 0
    });
    until("every sidecar process to exit", || sidecar_children() == 0);
    assert_eq!(sha256(&source), before);
}

#[test]
fn a_real_export_whose_source_or_folder_is_gone_fails_and_leaves_nothing() {
    let _real = lock(&REAL);
    let file = Arc::new(Source::with_encoders("vp9.webm"));
    let dir = common::scratch("queue-real-gone");
    let queue = ExportQueue::open(
        None,
        real_runner(Arc::clone(&file), common::orchestrator()),
        |_: &ExportJob| {},
    );

    // The source is removed after the export was asked for.
    let (source, project) = reversed(&file, &dir);
    std::fs::remove_file(&source).expect("remove");
    let target = dir.join("from-gone.mkv");
    let job = queue
        .submit(ExportSpec {
            project,
            ..spec("gone", &target)
        })
        .expect("queued");
    let state = queue.wait(job.id, LONG).expect("job").state;
    assert!(matches!(state, ExportState::Failed { .. }), "{state:?}");
    assert!(!target.exists());
    assert!(!partial_path(&target).exists());

    // The folder the export was to be written to is removed.
    let (_, project) = reversed(&file, &dir);
    let folder = dir.join("removed");
    std::fs::create_dir_all(&folder).expect("folder");
    let target = folder.join("out.mkv");
    std::fs::remove_dir_all(&folder).expect("remove");
    let job = queue
        .submit(ExportSpec {
            project,
            ..spec("no folder", &target)
        })
        .expect("queued");
    assert!(matches!(
        queue.wait(job.id, LONG).expect("job").state,
        ExportState::Failed { .. }
    ));
    assert!(!folder.exists());
}
