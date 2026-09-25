//! Running the FFmpeg sidecar.
//!
//! Every media operation in Blinkify goes through here: probing, indexing,
//! waveforms, thumbnails, proxies, and later decode and export. What this
//! layer guarantees is what makes the application feel responsive instead of
//! like a shell script with a window:
//!
//! - **Commands are typed** ([`SidecarCommand`]); a filename is never parsed as
//!   an option, a URL or a shell token.
//! - **Nothing blocks on a pipe.** stdout and stderr are both drained on their
//!   own threads from the moment the process starts. An undrained stderr fills
//!   its pipe buffer and FFmpeg stops, which looks like a hang at a random
//!   percentage.
//! - **Cancellation is real.** It is cooperative at the Rust layer — readers
//!   stop and the job is marked cancelled at once — and forceful at the
//!   process layer: the child is terminated, reaped, and its handles closed
//!   before [`Job::wait`] returns. A "cancelled" flag that lets FFmpeg run on
//!   is a lie to the user (`CLAUDE.md` section 12).
//! - **Work has a priority.** Interactive work (playback decode, seek) has its
//!   own slots and never queues behind a background index; background work
//!   (keyframe indexing, waveforms, thumbnails, proxies) runs at a lower OS
//!   priority class and is admitted only when nothing more urgent is waiting.
//! - **No process outlives the application** — see [`registry`].

mod command;
pub mod progress;
mod registry;

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

pub use command::{SidecarCommand, Stdout, Tool};
pub use progress::Progress;
use progress::{ProgressParser, ProgressReporter};

use crate::sidecar::Sidecar;

/// How many trailing stderr lines an error carries.
const STDERR_TAIL: usize = 20;

/// How long a killed process gets to disappear before the job reports it
/// could not be stopped.
const KILL_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the supervisor checks for exit and cancellation.
const POLL: Duration = Duration::from_millis(10);

/// The most standard output a [`Stdout::Collect`] job may buffer. `ffprobe`
/// JSON for a file with thousands of chapters is still far below this; more
/// than this is a file trying to exhaust memory.
const COLLECT_LIMIT: usize = 64 * 1024 * 1024;

/// How urgently a job needs to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// The user is waiting on it right now: a probe, a single-frame seek.
    Interactive,
    /// A decode that runs for as long as playback does: the preview's video
    /// and audio. Long-lived, so it has its own slots — held in the
    /// interactive ones it would leave a probe queued behind a film.
    Playback,
    /// The user asked for it and is watching it.
    Foreground,
    /// A process of an export pipeline (ADR-0010). The readers, encoders and
    /// the muxer of one export are coupled by pipes and must all run at once:
    /// one left queued behind the others stalls the rest. They have slots of
    /// their own, never shared with anything that could hold them.
    Export,
    /// Nobody is waiting: keyframe indexing, waveforms, thumbnails, proxies.
    Background,
}

/// How many processes may run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Slots reserved for interactive work, which nothing else may take.
    pub interactive: usize,
    /// Slots for playback decodes: video and audio of the clip playing, and
    /// of the next one, so a clip boundary never waits for a process to start.
    pub playback: usize,
    /// Slots shared by foreground and background work.
    pub shared: usize,
    /// The most of the shared slots background work may hold, so a queue of
    /// thumbnails can never occupy every slot an export needs.
    pub background: usize,
    /// Slots for the processes of export pipelines: enough for the muxer,
    /// a reader or encoder per stream, and a seam encoder, twice over.
    pub export: usize,
}

impl Limits {
    /// Sized for the machine. FFmpeg is itself multi-threaded, so a slot is
    /// worth several cores; more concurrent processes than this compete for
    /// the same cores and memory bandwidth and finish no sooner.
    #[must_use]
    pub fn for_this_machine() -> Self {
        let cores = thread::available_parallelism().map_or(4, std::num::NonZero::get);
        // A quarter of the cores, between two and four.
        let shared = (cores >> 2).clamp(2, 4);
        Self {
            interactive: 2,
            playback: 4,
            shared,
            background: shared - 1,
            export: 8,
        }
    }
}

/// A job's identity, as the renderer sees it in progress events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JobId(pub u32);

/// A progress update for one job, the payload of the progress event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobProgress {
    pub job: JobId,
    pub progress: Progress,
}

/// Why a job did not produce its result.
#[derive(Debug, Error)]
pub enum JobError {
    /// The sidecar could not be started at all.
    #[error("could not start {command}: {source}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },
    /// The sidecar ran and failed.
    #[error("{command} failed with {}: {}", exit_code.map_or_else(|| "no exit code".to_owned(), |c| format!("exit code {c}")), stderr_tail.join(" | "))]
    Failed {
        command: String,
        exit_code: Option<i32>,
        /// The last lines FFmpeg wrote to stderr — usually the actual reason.
        stderr_tail: Vec<String>,
    },
    /// The job was cancelled, and its process is gone.
    #[error("cancelled")]
    Cancelled,
    /// The consumer of the job's output rejected it.
    #[error("{command}: {message}")]
    Consumer { command: String, message: String },
    /// The process was killed but did not exit. Reported rather than ignored,
    /// because it means a file may still be held open.
    #[error("{command} did not exit after being terminated")]
    Unkillable { command: String },
    /// The application is shutting down and accepts no new work.
    #[error("the media engine is shutting down")]
    ShuttingDown,
}

/// What a finished job leaves behind.
#[derive(Debug, Clone, Default)]
pub struct JobOutput {
    /// Standard output, for [`Stdout::Collect`] jobs; empty otherwise.
    pub stdout: Vec<u8>,
    /// The last lines of stderr, for diagnostics.
    pub stderr_tail: Vec<String>,
}

/// What a streaming consumer tells the job after each chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Continue,
    /// The consumer has what it needs; stop the process. The job succeeds.
    Stop,
    /// The output is unusable. The job ends as [`JobError::Consumer`].
    Fail(String),
}

type ChunkConsumer = Box<dyn FnMut(&[u8]) -> Flow + Send>;
type ProgressCallback = Box<dyn FnMut(JobProgress) + Send>;
type LineConsumer = Box<dyn FnMut(&str) + Send>;

/// Everything a job needs besides its command line.
#[derive(Default)]
pub struct JobOptions {
    on_chunk: Option<ChunkConsumer>,
    on_progress: Option<ProgressCallback>,
    on_stderr_line: Option<LineConsumer>,
    remove_on_failure: Vec<PathBuf>,
    cancel: Option<CancelToken>,
    stdin: Option<mpsc::Receiver<Vec<u8>>>,
}

impl std::fmt::Debug for JobOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobOptions")
            .field("on_chunk", &self.on_chunk.is_some())
            .field("on_progress", &self.on_progress.is_some())
            .field("on_stderr_line", &self.on_stderr_line.is_some())
            .field("remove_on_failure", &self.remove_on_failure)
            .field("cancel", &self.cancel)
            .field("stdin", &self.stdin.is_some())
            .finish()
    }
}

impl JobOptions {
    /// Receive standard output as it arrives, for [`Stdout::Stream`] jobs.
    /// Called on the reader thread; it must not block for long.
    #[must_use]
    pub fn on_chunk(mut self, consumer: impl FnMut(&[u8]) -> Flow + Send + 'static) -> Self {
        self.on_chunk = Some(Box::new(consumer));
        self
    }

    /// Receive throttled progress, tagged with the job's id, for jobs built
    /// with [`SidecarCommand::report_progress`].
    #[must_use]
    pub fn on_progress(mut self, callback: impl FnMut(JobProgress) + Send + 'static) -> Self {
        self.on_progress = Some(Box::new(callback));
        self
    }

    /// Receive every line FFmpeg writes to stderr, as it is written — for
    /// filters that report per frame there, such as `showinfo`. Called on the
    /// stderr drain thread, which must never stall: a blocked drain fills the
    /// pipe and stops FFmpeg.
    #[must_use]
    pub fn on_stderr_line(mut self, consumer: impl FnMut(&str) + Send + 'static) -> Self {
        self.on_stderr_line = Some(Box::new(consumer));
        self
    }

    /// Cancel the job through `token`, which the caller already holds — for
    /// work that owns its own cancel switch across several steps.
    #[must_use]
    pub fn cancel_token(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// Feed the process's standard input from `chunks`, in order, closing it
    /// when the sender is dropped — for the export muxer, which reads the
    /// packets the engine routes to it (`-i pipe:0`). If the process exits
    /// first, the receiver is dropped and the sender's next send fails,
    /// which is how the producer learns to stop.
    #[must_use]
    pub fn stdin(mut self, chunks: mpsc::Receiver<Vec<u8>>) -> Self {
        self.stdin = Some(chunks);
        self
    }

    /// Delete `path` if the job is cancelled or fails, after its process has
    /// exited. A cancelled export leaves no partial file behind.
    #[must_use]
    pub fn remove_on_failure(mut self, path: PathBuf) -> Self {
        self.remove_on_failure.push(path);
        self
    }
}

/// A shareable cancel switch for one job.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A job in flight.
#[derive(Debug)]
pub struct Job {
    id: JobId,
    cancel: CancelToken,
    pid: Arc<OnceLock<u32>>,
    result: mpsc::Receiver<Result<JobOutput, JobError>>,
    supervisor: Option<JoinHandle<()>>,
}

impl Job {
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Cancel the job. Returns immediately; [`Job::wait`] returns once the
    /// process is gone.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The operating-system process id, once the process has started.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }

    /// Where the process id appears once the job has started — for a caller
    /// that hands the job to another thread to wait on but still needs to
    /// know which process is its.
    #[must_use]
    pub fn pid_handle(&self) -> Arc<OnceLock<u32>> {
        Arc::clone(&self.pid)
    }

    /// A handle that can cancel this job from elsewhere.
    #[must_use]
    pub fn canceller(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Block until the job has finished and its process has exited and been
    /// reaped.
    ///
    /// # Errors
    ///
    /// Whatever stopped the job; see [`JobError`].
    pub fn wait(mut self) -> Result<JobOutput, JobError> {
        let result = self.result.recv().unwrap_or(Err(JobError::Cancelled));
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        result
    }
}

#[derive(Debug, Default)]
struct State {
    running: BTreeMap<Priority, usize>,
    waiting: BTreeMap<Priority, usize>,
    live: BTreeMap<JobId, CancelToken>,
    next_id: u32,
    shutting_down: bool,
    /// The most jobs ever running at once, per priority. Read by tests.
    peak: BTreeMap<Priority, usize>,
}

impl State {
    fn count(map: &BTreeMap<Priority, usize>, priority: Priority) -> usize {
        map.get(&priority).copied().unwrap_or(0)
    }

    fn may_start(&self, priority: Priority, limits: &Limits) -> bool {
        let running = |p| Self::count(&self.running, p);
        let shared_in_use = running(Priority::Foreground) + running(Priority::Background);
        match priority {
            Priority::Interactive => running(Priority::Interactive) < limits.interactive,
            Priority::Playback => running(Priority::Playback) < limits.playback,
            Priority::Foreground => shared_in_use < limits.shared,
            Priority::Export => running(Priority::Export) < limits.export,
            Priority::Background => {
                shared_in_use < limits.shared
                    && running(Priority::Background) < limits.background
                    // Yield: nothing more urgent may be waiting for a slot.
                    && Self::count(&self.waiting, Priority::Foreground) == 0
                    && Self::count(&self.waiting, Priority::Export) == 0
                    && Self::count(&self.waiting, Priority::Interactive) == 0
                    && Self::count(&self.waiting, Priority::Playback) == 0
            }
        }
    }
}

#[derive(Debug)]
struct Inner {
    sidecar: Sidecar,
    limits: Limits,
    state: Mutex<State>,
    changed: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The one place sidecar processes are started.
#[derive(Debug, Clone)]
pub struct Orchestrator {
    inner: Arc<Inner>,
}

impl Orchestrator {
    #[must_use]
    pub fn new(sidecar: Sidecar, limits: Limits) -> Self {
        Self {
            inner: Arc::new(Inner {
                sidecar,
                limits,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            }),
        }
    }

    #[must_use]
    pub fn sidecar(&self) -> &Sidecar {
        &self.inner.sidecar
    }

    /// Queue `command` at `priority`. Returns at once; the job starts when a
    /// slot allows.
    #[must_use]
    pub fn run(&self, command: SidecarCommand, priority: Priority, mut options: JobOptions) -> Job {
        let (sender, result) = mpsc::channel();
        let cancel = options.cancel.take().unwrap_or_default();
        let id = {
            let mut state = self.inner.lock();
            let id = JobId(state.next_id);
            state.next_id = state.next_id.wrapping_add(1);
            state.live.insert(id, cancel.clone());
            id
        };
        let inner = Arc::clone(&self.inner);
        let token = cancel.clone();
        let pid = Arc::new(OnceLock::new());
        let pid_slot = Arc::clone(&pid);
        let supervisor = thread::Builder::new()
            .name(format!("sidecar-job-{}", id.0))
            .spawn(move || {
                let outcome = supervise(&inner, id, &command, priority, &token, &pid_slot, options);
                inner.lock().live.remove(&id);
                inner.changed.notify_all();
                let _ = sender.send(outcome);
            })
            .ok();
        Job {
            id,
            cancel,
            pid,
            result,
            supervisor,
        }
    }

    /// Run `command` and wait for it. For short jobs on a thread that is
    /// already off the UI thread.
    ///
    /// # Errors
    ///
    /// As [`Job::wait`].
    pub fn run_to_end(
        &self,
        command: SidecarCommand,
        priority: Priority,
    ) -> Result<JobOutput, JobError> {
        self.run(command, priority, JobOptions::default()).wait()
    }

    /// How many jobs are running right now, per priority.
    #[must_use]
    pub fn running(&self, priority: Priority) -> usize {
        State::count(&self.inner.lock().running, priority)
    }

    /// The most jobs that were ever running at once, per priority.
    #[must_use]
    pub fn peak(&self, priority: Priority) -> usize {
        State::count(&self.inner.lock().peak, priority)
    }

    /// Cancel every job, refuse new ones, and wait until every process has
    /// exited. Called when the application closes; the job object in
    /// [`registry`] covers the case where this never gets to run.
    pub fn shutdown(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut state = self.inner.lock();
        state.shutting_down = true;
        for token in state.live.values() {
            token.cancel();
        }
        self.inner.changed.notify_all();
        while !state.live.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            state = self
                .inner
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Wait for a slot, run the process, and account for it.
fn supervise(
    inner: &Inner,
    id: JobId,
    command: &SidecarCommand,
    priority: Priority,
    cancel: &CancelToken,
    pid: &OnceLock<u32>,
    options: JobOptions,
) -> Result<JobOutput, JobError> {
    if !acquire(inner, priority, cancel)? {
        return Err(JobError::Cancelled);
    }
    let remove_on_failure = options.remove_on_failure.clone();
    let outcome = execute(inner, id, command, priority, cancel, pid, options);
    release(inner, priority);
    if outcome.is_err() {
        for path in &remove_on_failure {
            let _ = fs::remove_file(path);
        }
    }
    outcome
}

/// Block until the job may start. `Ok(false)` if it was cancelled while
/// queued, in which case no process was ever spawned.
fn acquire(inner: &Inner, priority: Priority, cancel: &CancelToken) -> Result<bool, JobError> {
    let mut state = inner.lock();
    *state.waiting.entry(priority).or_default() += 1;
    let admitted = loop {
        if state.shutting_down {
            break Err(JobError::ShuttingDown);
        }
        if cancel.is_cancelled() {
            break Ok(false);
        }
        if state.may_start(priority, &inner.limits) {
            break Ok(true);
        }
        // Timed, so a cancel on a queued job is noticed without anyone
        // having to notify this particular waiter.
        state = inner
            .changed
            .wait_timeout(state, Duration::from_millis(50))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    };
    if let Some(waiting) = state.waiting.get_mut(&priority) {
        *waiting = waiting.saturating_sub(1);
    }
    if matches!(admitted, Ok(true)) {
        let running = state.running.entry(priority).or_default();
        *running += 1;
        let now = *running;
        let peak = state.peak.entry(priority).or_default();
        *peak = (*peak).max(now);
    }
    drop(state);
    inner.changed.notify_all();
    admitted
}

fn release(inner: &Inner, priority: Priority) {
    let mut state = inner.lock();
    if let Some(running) = state.running.get_mut(&priority) {
        *running = running.saturating_sub(1);
    }
    drop(state);
    inner.changed.notify_all();
}

fn execute(
    inner: &Inner,
    id: JobId,
    command: &SidecarCommand,
    priority: Priority,
    cancel: &CancelToken,
    pid: &OnceLock<u32>,
    mut options: JobOptions,
) -> Result<JobOutput, JobError> {
    let program = match command.tool() {
        Tool::Ffmpeg => inner.sidecar.ffmpeg(),
        Tool::Ffprobe => inner.sidecar.ffprobe(),
    };
    let stdin_chunks = options.stdin.take();
    let mut process = Command::new(program);
    process
        .args(command.args())
        .stdin(
            stdin_chunks
                .as_ref()
                .map_or_else(Stdio::null, |_| Stdio::piped()),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    set_creation_flags(&mut process, priority);

    let mut child = process.spawn().map_err(|source| JobError::Spawn {
        command: command.to_string(),
        source,
    })?;
    let _ = pid.set(child.id());
    let _ = registry::adopt(&child);
    let stdin_writer = child
        .stdin
        .take()
        .zip(stdin_chunks)
        .map(|(stdin, chunks)| feed_stdin(stdin, chunks));

    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
    let stderr_reader = child.stderr.take().map(|stderr| {
        drain_stderr(
            stderr,
            Arc::clone(&stderr_tail),
            options.on_stderr_line.take(),
        )
    });
    let consumer_verdict: Arc<Mutex<Option<Flow>>> = Arc::new(Mutex::new(None));
    let stdout_reader = child.stdout.take().map(|stdout| {
        read_stdout(
            stdout,
            id,
            command,
            options,
            cancel.clone(),
            Arc::clone(&consumer_verdict),
        )
    });

    let status = wait_or_kill(&mut child, cancel);

    // The process is gone, so both pipes are closed and both readers finish;
    // the stdin feeder's next write fails and it ends too.
    let _ = stdin_writer.map(JoinHandle::join);
    let collected = stdout_reader.and_then(|reader| reader.join().ok());
    if let Some(reader) = stderr_reader {
        let _ = reader.join();
    }
    let stderr_tail: Vec<String> = stderr_tail
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .cloned()
        .collect();

    let Some(status) = status else {
        return Err(JobError::Unkillable {
            command: command.to_string(),
        });
    };
    let verdict = consumer_verdict
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    match verdict {
        Some(Flow::Fail(message)) => {
            return Err(JobError::Consumer {
                command: command.to_string(),
                message,
            });
        }
        // The consumer had what it needed and stopped the process on purpose.
        Some(Flow::Stop) => {
            return Ok(JobOutput {
                stdout: Vec::new(),
                stderr_tail,
            });
        }
        Some(Flow::Continue) | None => {}
    }
    if cancel.is_cancelled() && !status.success() {
        return Err(JobError::Cancelled);
    }
    if !status.success() {
        return Err(JobError::Failed {
            command: command.to_string(),
            exit_code: status.code(),
            stderr_tail,
        });
    }
    let (stdout, finish) = collected.unwrap_or_default();
    if let Some(finish) = finish {
        finish();
    }
    Ok(JobOutput {
        stdout,
        stderr_tail,
    })
}

/// Poll until the child exits, killing it if the job is cancelled. `None`
/// means it was killed and still would not go.
fn wait_or_kill(child: &mut Child, cancel: &CancelToken) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => break,
        }
        if cancel.is_cancelled() {
            break;
        }
        thread::sleep(POLL);
    }
    // TerminateProcess, then wait for the kernel to finish tearing the process
    // down — only then are its file handles closed.
    let _ = child.kill();
    let deadline = Instant::now() + KILL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            _ => return None,
        }
    }
}

/// Write every chunk to the child's standard input, then close it. Stops at
/// the first failed write — the process has exited — dropping `chunks` so the
/// producer's sends fail.
fn feed_stdin(stdin: std::process::ChildStdin, chunks: mpsc::Receiver<Vec<u8>>) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut stdin = stdin;
        for chunk in chunks {
            if std::io::Write::write_all(&mut stdin, &chunk).is_err() {
                return;
            }
        }
        let _ = std::io::Write::flush(&mut stdin);
    })
}

fn drain_stderr(
    stderr: ChildStderr,
    tail: Arc<Mutex<VecDeque<String>>>,
    mut on_line: Option<LineConsumer>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = Vec::new();
        // Byte lines, not `lines()`: FFmpeg prints file names in whatever
        // encoding they came in, and a non-UTF-8 line must not end the drain.
        while reader
            .read_until(b'\n', &mut line)
            .is_ok_and(|read| read > 0)
        {
            let text = String::from_utf8_lossy(&line).trim_end().to_owned();
            if let Some(on_line) = on_line.as_mut() {
                on_line(&text);
            }
            if !text.is_empty() {
                let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(text);
            }
            line.clear();
        }
    })
}

type Finish = Box<dyn FnOnce() + Send>;

/// Read standard output according to the command's [`Stdout`] mode. Returns
/// the collected bytes, and for progress jobs a closure that sends the final
/// 100 percent — called only once the process has exited successfully.
fn read_stdout(
    stdout: ChildStdout,
    job: JobId,
    command: &SidecarCommand,
    mut options: JobOptions,
    cancel: CancelToken,
    verdict: Arc<Mutex<Option<Flow>>>,
) -> JoinHandle<(Vec<u8>, Option<Finish>)> {
    let mode = command.stdout();
    let total = command.progress_total().unwrap_or_default();
    thread::spawn(move || match mode {
        Stdout::Progress => {
            let mut parser = ProgressParser::default();
            let mut reporter = ProgressReporter::new(total);
            let mut callback = options.on_progress.take();
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 0) {
                if let Some(block) = parser.line(&line)
                    && let (Some(update), Some(callback)) =
                        (reporter.block(&block, Instant::now()), callback.as_mut())
                {
                    callback(JobProgress {
                        job,
                        progress: update,
                    });
                }
                line.clear();
            }
            let finish: Option<Finish> = callback.map(|mut callback| {
                Box::new(move || {
                    if let Some(done) = reporter.finish() {
                        callback(JobProgress {
                            job,
                            progress: done,
                        });
                    }
                }) as Finish
            });
            (Vec::new(), finish)
        }
        Stdout::Stream => {
            let mut consumer = options.on_chunk.take();
            let mut reader = stdout;
            let mut buffer = vec![0_u8; 256 * 1024];
            loop {
                let read = match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                if cancel.is_cancelled() {
                    // Keep draining so the process is never blocked on a full
                    // pipe while it is being killed.
                    continue;
                }
                let flow = match (consumer.as_mut(), buffer.get(..read)) {
                    (Some(consumer), Some(chunk)) => consumer(chunk),
                    _ => Flow::Continue,
                };
                if flow != Flow::Continue {
                    *verdict.lock().unwrap_or_else(PoisonError::into_inner) = Some(flow);
                    cancel.cancel();
                }
            }
            (Vec::new(), None)
        }
        Stdout::Collect => {
            let mut collected = Vec::new();
            let mut limited = stdout.take(u64::try_from(COLLECT_LIMIT).unwrap_or(u64::MAX));
            if limited.read_to_end(&mut collected).is_err() {
                collected.clear();
            }
            // Drain anything past the limit rather than leave FFmpeg blocked.
            let _ = std::io::copy(&mut limited.into_inner(), &mut std::io::sink());
            (collected, None)
        }
    })
}

/// No console window, and a lower scheduling class for background work so it
/// yields the CPU to playback rather than competing with it.
fn set_creation_flags(command: &mut Command, priority: Priority) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        let class = match priority {
            Priority::Background => BELOW_NORMAL_PRIORITY_CLASS,
            Priority::Interactive
            | Priority::Playback
            | Priority::Foreground
            | Priority::Export => 0,
        };
        command.creation_flags(CREATE_NO_WINDOW | class);
    }
    #[cfg(not(windows))]
    let _ = (command, priority);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(running: &[(Priority, usize)], waiting: &[(Priority, usize)]) -> State {
        State {
            running: running.iter().copied().collect(),
            waiting: waiting.iter().copied().collect(),
            ..State::default()
        }
    }

    const LIMITS: Limits = Limits {
        interactive: 2,
        playback: 2,
        shared: 2,
        background: 1,
        export: 8,
    };

    #[test]
    fn interactive_work_never_queues_behind_background_work() {
        let busy = state(
            &[(Priority::Background, 1), (Priority::Foreground, 1)],
            &[(Priority::Background, 50)],
        );
        assert!(busy.may_start(Priority::Interactive, &LIMITS));
    }

    #[test]
    fn background_work_cannot_take_every_shared_slot() {
        let one_background = state(&[(Priority::Background, 1)], &[]);
        assert!(!one_background.may_start(Priority::Background, &LIMITS));
        assert!(one_background.may_start(Priority::Foreground, &LIMITS));
    }

    #[test]
    fn background_work_yields_while_anything_more_urgent_waits() {
        let export_waiting = state(&[], &[(Priority::Foreground, 1)]);
        assert!(!export_waiting.may_start(Priority::Background, &LIMITS));
        let seek_waiting = state(&[], &[(Priority::Interactive, 1)]);
        assert!(!seek_waiting.may_start(Priority::Background, &LIMITS));
    }

    #[test]
    fn the_interactive_limit_still_holds() {
        // An export's processes never queue behind shared work (#110): with
        // every shared slot taken, the next process of a pipeline still runs.
        let shared_full = state(&[(Priority::Foreground, 2), (Priority::Export, 3)], &[]);
        assert!(!shared_full.may_start(Priority::Foreground, &LIMITS));
        assert!(shared_full.may_start(Priority::Export, &LIMITS));
        let export_waiting = state(&[], &[(Priority::Export, 1)]);
        assert!(!export_waiting.may_start(Priority::Background, &LIMITS));

        let full = state(&[(Priority::Interactive, 2)], &[]);
        assert!(!full.may_start(Priority::Interactive, &LIMITS));
    }
}
