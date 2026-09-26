//! The background export queue (#51): exports run one after another on a
//! thread of their own, report real progress, can be cancelled, and survive
//! the application stopping underneath them.
//!
//! What it holds to:
//!
//! - **A job is a snapshot.** It carries a copy of the project as it was when
//!   the export was asked for, and plans from that copy when it starts. The
//!   user goes on editing; the running export cannot see it.
//! - **One export at a time, in order.** An export already runs a reader per
//!   stream, a muxer and possibly encoders at once, and two 4K exports side by
//!   side saturate the machine; the orchestrator's export slots are sized for
//!   one. Later jobs wait in the order they were asked for.
//! - **Progress only moves forward**, and the time left is derived from the
//!   throughput measured so far, never from a timer.
//! - **Cancelling is real.** The job's [`CancelToken`] stops every process
//!   of the export, and the executor removes the partial file.
//! - **The queue is on disk.** Every change of state is written before it is
//!   announced. A job found queued or running when the queue is opened was
//!   interrupted — the application stopped under it — and is offered to be
//!   exported again from the start or discarded. It is never "continued":
//!   an export written through pipes into one muxer has no point it can
//!   honestly resume from.
//!
//! The queue knows nothing about how an export is made: a [`Runner`] does
//! that. The shell's runner resolves loudness, plans and executes; a test's
//! runner can be anything.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::audio::AudioTarget;
use super::execute::partial_path;
use crate::orchestrator::CancelToken;
use crate::project::Project;

/// Finished jobs kept in the history; the oldest are forgotten first.
pub const HISTORY_LIMIT: usize = 50;

/// The version of the file the queue is kept in.
const STORE_VERSION: u32 = 1;

/// Below this fraction a rate is noise, and no time left is offered.
const ESTIMATE_FROM_FRACTION: f64 = 0.01;

/// Nor before this much has been measured.
const ESTIMATE_AFTER: Duration = Duration::from_secs(1);

/// What is to be exported, fixed when the export is asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSpec {
    /// The project as it was: planned from when the job starts.
    pub project: Project,
    /// What the job is called in the queue: the project's name.
    pub name: String,
    pub target: PathBuf,
    /// The user confirmed replacing an existing target.
    pub overwrite: bool,
    pub audio: AudioTarget,
}

/// The two stages of a running export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Stage {
    /// Measuring loudness and planning, before anything is written. Its
    /// length is not known in advance.
    Preparing,
    /// Writing the output file, with progress from the muxer.
    Exporting,
}

/// Where a job is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ExportState {
    /// Waiting for the jobs before it.
    Queued,
    Running {
        stage: Stage,
        /// Of the stage, from 0 to 1; never less than the last reported.
        fraction: f64,
        /// Derived from the throughput so far, once there is enough of it.
        #[ts(type = "number | null")]
        remaining_seconds: Option<f64>,
    },
    /// The output file is complete at the target.
    Completed {
        #[ts(type = "number")]
        bytes: u64,
    },
    /// Nothing was left at the target.
    Failed { message: String },
    /// Cancelled, or discarded after an interruption. Nothing was left at the
    /// target.
    Cancelled,
    /// Queued or running when the application stopped: offered to be
    /// exported again or discarded.
    Interrupted,
}

impl ExportState {
    /// Whether the job has reached an end, and belongs to the history.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled
        )
    }
}

/// One export, as the queue and its history show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportJob {
    #[ts(type = "number")]
    pub id: u64,
    pub name: String,
    #[ts(type = "string")]
    pub target: PathBuf,
    pub audio: AudioTarget,
    /// Milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub submitted: u64,
    #[ts(type = "number | null")]
    pub started: Option<u64>,
    #[ts(type = "number | null")]
    pub finished: Option<u64>,
    pub state: ExportState,
}

/// What a finished export produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOutcome {
    /// The size of the output file.
    pub bytes: u64,
}

/// A running export's progress: its stage, and how far through it.
pub type Report = Arc<dyn Fn(Stage, f64) + Send + Sync>;

/// How an export is made.
pub trait Runner: Send + Sync {
    /// Export `spec`, reporting progress through `report`, stopping when
    /// `cancel` is.
    ///
    /// # Errors
    ///
    /// Why nothing was exported, in words the user reads.
    fn run(
        &self,
        spec: &ExportSpec,
        cancel: &CancelToken,
        report: Report,
    ) -> Result<RunOutcome, String>;
}

impl<F> Runner for F
where
    F: Fn(&ExportSpec, &CancelToken, Report) -> Result<RunOutcome, String> + Send + Sync,
{
    fn run(
        &self,
        spec: &ExportSpec,
        cancel: &CancelToken,
        report: Report,
    ) -> Result<RunOutcome, String> {
        self(spec, cancel, report)
    }
}

/// Why the queue refused a request.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum QueueError {
    #[error("there is no export {0}")]
    NoSuchJob(u64),
    #[error("export {0} was not interrupted")]
    NotInterrupted(u64),
    #[error("{} is one of the project's sources, and a source is never written to", .0.display())]
    TargetIsSource(PathBuf),
}

/// A job, and what it needs to run while it may still run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    job: ExportJob,
    /// Dropped when the job finishes: the history needs the outcome, not a
    /// copy of the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spec: Option<ExportSpec>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    version: u32,
    next: u64,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Inner {
    next: u64,
    entries: Vec<Entry>,
    /// The running job, and how to stop it.
    running: Option<Running>,
    stopping: bool,
}

/// The job being exported.
#[derive(Debug)]
struct Running {
    id: u64,
    cancel: CancelToken,
    /// The user asked for it to stop. Kept apart from the token, which the
    /// executor also trips to stop its own processes when the export fails.
    cancelled: bool,
}

type Observer = Box<dyn Fn(&ExportJob) + Send + Sync>;

struct Shared {
    inner: Mutex<Inner>,
    /// The worker waits here for a job.
    wake: Condvar,
    /// Callers of [`ExportQueue::wait`] wait here for a job to finish.
    changed: Condvar,
    store: Option<PathBuf>,
    runner: Box<dyn Runner>,
    observer: Observer,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

/// The export queue: submit, watch, cancel, and pick up after a crash.
#[derive(Debug)]
pub struct ExportQueue {
    shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl ExportQueue {
    /// Open the queue kept at `store` — or kept nowhere, with `None` — and
    /// start its worker. Jobs the file records as queued or running were
    /// interrupted, and are left for the user to resume or discard.
    /// `observer` is told of every change of every job; it may be called
    /// with the queue locked, so it must not call back into the queue.
    ///
    /// A store that cannot be read is set aside beside itself, as
    /// `<store>.unreadable`, and the queue starts empty: losing a history is
    /// better than not being able to export.
    pub fn open(
        store: Option<PathBuf>,
        runner: impl Runner + 'static,
        observer: impl Fn(&ExportJob) + Send + Sync + 'static,
    ) -> Self {
        let mut stored = store.as_deref().map_or(
            Stored {
                version: STORE_VERSION,
                next: 1,
                entries: Vec::new(),
            },
            load,
        );
        for entry in &mut stored.entries {
            if matches!(
                entry.job.state,
                ExportState::Queued | ExportState::Running { .. }
            ) {
                entry.job.state = ExportState::Interrupted;
            }
        }
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                next: stored.next,
                entries: stored.entries,
                running: None,
                stopping: false,
            }),
            wake: Condvar::new(),
            changed: Condvar::new(),
            store,
            runner: Box::new(runner),
            observer: Box::new(observer),
        });
        shared.persist(&shared.lock());
        let worker = {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name("export-queue".to_owned())
                .spawn(move || work(&shared))
                .ok()
        };
        Self {
            shared,
            worker: Mutex::new(worker),
        }
    }

    /// Queue `spec` behind every job already waiting.
    ///
    /// # Errors
    ///
    /// The target is one of the project's sources.
    pub fn submit(&self, spec: ExportSpec) -> Result<ExportJob, QueueError> {
        if let Some(source) = spec
            .project
            .sources
            .values()
            .map(crate::project::SourceRef::path)
            .find(|path| same_file(path, &spec.target))
        {
            return Err(QueueError::TargetIsSource(source.to_path_buf()));
        }
        let mut inner = self.shared.lock();
        let id = inner.next;
        inner.next = id.saturating_add(1);
        let job = ExportJob {
            id,
            name: spec.name.clone(),
            target: spec.target.clone(),
            audio: spec.audio,
            submitted: now(),
            started: None,
            finished: None,
            state: ExportState::Queued,
        };
        inner.entries.push(Entry {
            job: job.clone(),
            spec: Some(spec),
        });
        self.shared.persist(&inner);
        drop(inner);
        (self.shared.observer)(&job);
        self.shared.wake.notify_all();
        Ok(job)
    }

    /// Cancel job `id`: a queued job never starts, and a running one is
    /// stopped — every process of it — and its partial file removed. A job
    /// that has finished is left as it is.
    ///
    /// # Errors
    ///
    /// There is no such job.
    pub fn cancel(&self, id: u64) -> Result<(), QueueError> {
        let mut inner = self.shared.lock();
        if let Some(running) = inner.running.as_mut()
            && running.id == id
        {
            // The worker records the outcome when the export has stopped.
            running.cancelled = true;
            running.cancel.cancel();
            return Ok(());
        }
        let entry = inner.entry(id)?;
        if matches!(
            entry.job.state,
            ExportState::Queued | ExportState::Interrupted
        ) {
            entry.job.state = ExportState::Cancelled;
            entry.job.finished = Some(now());
            entry.spec = None;
            let job = entry.job.clone();
            self.shared.settle(&mut inner, &job);
        }
        Ok(())
    }

    /// Export interrupted job `id` again, from the start, behind the jobs
    /// already waiting. Whatever the interrupted export left beside its
    /// target is removed first.
    ///
    /// # Errors
    ///
    /// There is no such job, or it was not interrupted.
    pub fn resume(&self, id: u64) -> Result<ExportJob, QueueError> {
        let mut inner = self.shared.lock();
        let entry = inner.entry(id)?;
        if entry.job.state != ExportState::Interrupted {
            return Err(QueueError::NotInterrupted(id));
        }
        remove_partial(&entry.job.target);
        entry.job.state = ExportState::Queued;
        entry.job.started = None;
        let job = entry.job.clone();
        // Behind the jobs already waiting: interrupted jobs keep their place
        // in the file, but a resumed one is asked for now.
        if let Some(at) = inner.entries.iter().position(|e| e.job.id == id) {
            let moved = inner.entries.remove(at);
            inner.entries.push(moved);
        }
        self.shared.persist(&inner);
        drop(inner);
        (self.shared.observer)(&job);
        self.shared.wake.notify_all();
        Ok(job)
    }

    /// Give up interrupted job `id`, removing what it left beside its target.
    ///
    /// # Errors
    ///
    /// There is no such job, or it was not interrupted.
    pub fn discard(&self, id: u64) -> Result<ExportJob, QueueError> {
        let mut inner = self.shared.lock();
        let entry = inner.entry(id)?;
        if entry.job.state != ExportState::Interrupted {
            return Err(QueueError::NotInterrupted(id));
        }
        remove_partial(&entry.job.target);
        entry.job.state = ExportState::Cancelled;
        entry.job.finished = Some(now());
        entry.spec = None;
        let job = entry.job.clone();
        self.shared.settle(&mut inner, &job);
        Ok(job)
    }

    /// Every job: the queue in order, then the history, oldest first.
    #[must_use]
    pub fn jobs(&self) -> Vec<ExportJob> {
        self.shared
            .lock()
            .entries
            .iter()
            .map(|entry| entry.job.clone())
            .collect()
    }

    /// Forget every finished job.
    pub fn clear_history(&self) {
        let mut inner = self.shared.lock();
        inner.entries.retain(|entry| !entry.job.state.is_finished());
        self.shared.persist(&inner);
    }

    /// Wait up to `timeout` for job `id` to finish; the job as it then is.
    pub fn wait(&self, id: u64, timeout: Duration) -> Option<ExportJob> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.shared.lock();
        loop {
            let job = inner.entries.iter().find(|e| e.job.id == id)?.job.clone();
            let left = deadline.saturating_duration_since(Instant::now());
            if job.state.is_finished() || job.state == ExportState::Interrupted || left.is_zero() {
                return Some(job);
            }
            inner = self
                .shared
                .changed
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Stop the worker. A running export is stopped and left interrupted,
    /// so the next launch offers it again; queued jobs stay queued on disk
    /// and are offered the same way.
    pub fn shutdown(&self) {
        {
            let mut inner = self.shared.lock();
            inner.stopping = true;
            if let Some(running) = &inner.running {
                running.cancel.cancel();
            }
        }
        self.shared.wake.notify_all();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

impl Drop for ExportQueue {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Inner {
    fn entry(&mut self, id: u64) -> Result<&mut Entry, QueueError> {
        self.entries
            .iter_mut()
            .find(|entry| entry.job.id == id)
            .ok_or(QueueError::NoSuchJob(id))
    }

    /// Keep at most [`HISTORY_LIMIT`] finished jobs.
    fn trim(&mut self) {
        let finished = self
            .entries
            .iter()
            .filter(|entry| entry.job.state.is_finished())
            .count();
        let mut excess = finished.saturating_sub(HISTORY_LIMIT);
        self.entries.retain(|entry| {
            if excess > 0 && entry.job.state.is_finished() {
                excess -= 1;
                false
            } else {
                true
            }
        });
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write the queue to its store: to a file beside it, then renamed over
    /// it, so a crash mid-write leaves the previous state rather than half
    /// of one.
    fn persist(&self, inner: &Inner) {
        let Some(store) = &self.store else { return };
        let stored = Stored {
            version: STORE_VERSION,
            next: inner.next,
            entries: inner.entries.clone(),
        };
        // A queue that cannot be written still exports; what is lost is the
        // offer to resume after a crash, which is the lesser failure.
        let _ = write_atomically(store, &stored);
    }

    /// Record a finished or changed job: trim the history, persist, and tell
    /// everyone waiting.
    fn settle(&self, inner: &mut MutexGuard<'_, Inner>, job: &ExportJob) {
        inner.trim();
        self.persist(inner);
        (self.observer)(job);
        self.changed.notify_all();
    }

    /// A running job's progress, as the observer sees it. Not persisted: a
    /// crash leaves the job running on disk, which is all that matters.
    fn progress(&self, id: u64, state: ExportState) {
        let mut inner = self.lock();
        if let Ok(entry) = inner.entry(id)
            && matches!(entry.job.state, ExportState::Running { .. })
        {
            entry.job.state = state;
            let job = entry.job.clone();
            drop(inner);
            (self.observer)(&job);
        }
    }
}

/// The worker: take the first queued job, run it, record how it ended.
fn work(shared: &Arc<Shared>) {
    while let Some((id, spec, cancel)) = next_job(shared) {
        let meter = Arc::new(Mutex::new(Meter::default()));
        let report: Report = {
            let shared = Arc::clone(shared);
            Arc::new(move |stage, fraction| {
                let state = meter.lock().unwrap_or_else(PoisonError::into_inner).update(
                    stage,
                    fraction,
                    Instant::now(),
                );
                shared.progress(id, state);
            })
        };
        let result = shared.runner.run(&spec, &cancel, report);

        let mut inner = shared.lock();
        let cancelled = inner.running.take().is_some_and(|r| r.cancelled);
        let stopping = inner.stopping;
        let Ok(entry) = inner.entry(id) else { continue };
        // A file that was completed is reported as completed, however late
        // the request to stop it came.
        entry.job.state = match result {
            Ok(outcome) => ExportState::Completed {
                bytes: outcome.bytes,
            },
            Err(_) if stopping => ExportState::Interrupted,
            Err(_) if cancelled => ExportState::Cancelled,
            Err(message) => ExportState::Failed { message },
        };
        if entry.job.state == ExportState::Interrupted {
            entry.job.started = None;
        } else {
            entry.job.finished = Some(now());
            entry.spec = None;
        }
        let job = entry.job.clone();
        shared.settle(&mut inner, &job);
    }
}

/// Block until a job is queued, and start it; `None` once stopping.
fn next_job(shared: &Shared) -> Option<(u64, ExportSpec, CancelToken)> {
    let mut inner = shared.lock();
    loop {
        if inner.stopping {
            return None;
        }
        if let Some(entry) = inner
            .entries
            .iter_mut()
            .find(|entry| entry.job.state == ExportState::Queued)
        {
            let id = entry.job.id;
            let Some(spec) = entry.spec.clone() else {
                // Only a store edited by hand holds a queued job without
                // its project; there is nothing to export.
                entry.job.state = ExportState::Failed {
                    message: "the export's project was not kept".to_owned(),
                };
                entry.job.finished = Some(now());
                let job = entry.job.clone();
                shared.settle(&mut inner, &job);
                continue;
            };
            entry.job.state = ExportState::Running {
                stage: Stage::Preparing,
                fraction: 0.0,
                remaining_seconds: None,
            };
            entry.job.started = Some(now());
            let job = entry.job.clone();
            let cancel = CancelToken::default();
            inner.running = Some(Running {
                id,
                cancel: cancel.clone(),
                cancelled: false,
            });
            shared.persist(&inner);
            drop(inner);
            (shared.observer)(&job);
            return Some((id, spec, cancel));
        }
        inner = shared
            .wake
            .wait(inner)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// Turns reports into progress that only moves forward, with the time left.
#[derive(Debug)]
struct Meter {
    stage: Stage,
    fraction: f64,
    /// When the current stage began.
    began: Option<Instant>,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            stage: Stage::Preparing,
            fraction: 0.0,
            began: None,
        }
    }
}

impl Meter {
    fn update(&mut self, stage: Stage, fraction: f64, now: Instant) -> ExportState {
        // A stage never goes back, and a new one starts from nothing.
        if stage > self.stage {
            self.stage = stage;
            self.fraction = 0.0;
            self.began = Some(now);
        }
        let began = *self.began.get_or_insert(now);
        if stage == self.stage && fraction.is_finite() {
            self.fraction = self.fraction.max(fraction.clamp(0.0, 1.0));
        }
        let elapsed = now.saturating_duration_since(began);
        let remaining_seconds = (self.stage == Stage::Exporting
            && self.fraction >= ESTIMATE_FROM_FRACTION
            && elapsed >= ESTIMATE_AFTER)
            .then(|| elapsed.as_secs_f64() * (1.0 - self.fraction) / self.fraction);
        ExportState::Running {
            stage: self.stage,
            fraction: self.fraction,
            remaining_seconds,
        }
    }
}

/// Milliseconds since the Unix epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Remove what an interrupted export left beside `target`. Only an export
/// of this target makes that name.
fn remove_partial(target: &Path) {
    let partial = partial_path(target);
    if partial.is_file() {
        let _ = fs::remove_file(partial);
    }
}

fn load(store: &Path) -> Stored {
    let empty = Stored {
        version: STORE_VERSION,
        next: 1,
        entries: Vec::new(),
    };
    let Ok(bytes) = fs::read(store) else {
        return empty;
    };
    match serde_json::from_slice::<Stored>(&bytes) {
        Ok(stored) if stored.version == STORE_VERSION => stored,
        _ => {
            let mut aside = store.as_os_str().to_owned();
            aside.push(".unreadable");
            let _ = fs::rename(store, PathBuf::from(aside));
            empty
        }
    }
}

fn write_atomically(store: &Path, stored: &Stored) -> io::Result<()> {
    if let Some(parent) = store.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(stored).map_err(io::Error::other)?;
    let mut temporary = store.as_os_str().to_owned();
    temporary.push(".writing");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, store)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn progress_never_moves_backwards() {
        let mut meter = Meter::default();
        let t = Instant::now();
        let fraction = |state: ExportState| match state {
            ExportState::Running { fraction, .. } => fraction,
            _ => f64::NAN,
        };
        assert!((fraction(meter.update(Stage::Exporting, 0.4, t)) - 0.4).abs() < f64::EPSILON);
        assert!((fraction(meter.update(Stage::Exporting, 0.2, t)) - 0.4).abs() < f64::EPSILON);
        // A late report of an earlier stage changes nothing.
        let state = meter.update(Stage::Preparing, 0.9, t);
        assert!(
            matches!(state, ExportState::Running { stage: Stage::Exporting, fraction, .. } if (fraction - 0.4).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn time_left_comes_from_measured_throughput() {
        let mut meter = Meter::default();
        let t = Instant::now();
        meter.update(Stage::Exporting, 0.0, t);
        // Too early to say.
        let early = meter.update(Stage::Exporting, 0.001, t + Duration::from_secs(2));
        assert!(matches!(
            early,
            ExportState::Running {
                remaining_seconds: None,
                ..
            }
        ));
        // A quarter in ten seconds: thirty more.
        let state = meter.update(Stage::Exporting, 0.25, t + Duration::from_secs(10));
        let ExportState::Running {
            remaining_seconds: Some(left),
            ..
        } = state
        else {
            panic!("no estimate");
        };
        assert!((left - 30.0).abs() < 1e-9, "{left}");
    }

    #[test]
    fn a_later_stage_starts_from_nothing() {
        let mut meter = Meter::default();
        let t = Instant::now();
        meter.update(Stage::Preparing, 0.8, t);
        let state = meter.update(Stage::Exporting, 0.0, t);
        assert!(matches!(
            state,
            ExportState::Running {
                stage: Stage::Exporting,
                fraction,
                ..
            } if fraction.abs() < f64::EPSILON
        ));
    }
}
