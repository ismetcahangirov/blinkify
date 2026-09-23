//! The shell's handle on the media engine.
//!
//! Thin, like the rest of this crate: it finds the bundled sidecar next to the
//! executable, owns the one [`Orchestrator`] every sidecar process goes
//! through, forwards job progress to the renderer as events, and hands the
//! renderer what the engine concluded. It decides nothing itself.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use blinkify_engine::cache::Cache;
use blinkify_engine::capability;
use blinkify_engine::keyframes::{self, IndexProgress, KeyframeIndex};
use blinkify_engine::orchestrator::{JobProgress, Limits, Orchestrator};
use blinkify_engine::probe::{MediaInfo, Prober};
use blinkify_engine::{EncoderCapabilities, Sidecar};
use tauri::{AppHandle, Emitter};

/// The event every job's progress arrives on, with a [`JobProgress`] payload.
///
/// One channel for all jobs rather than one per job: the renderer subscribes
/// once and routes by `job`, and the IPC surface stays coarse (`CLAUDE.md`
/// section 2). The engine throttles before anything reaches here.
pub const PROGRESS_EVENT: &str = "media://progress";

/// Background keyframe indexing progress, with an [`IndexProgress`] payload.
pub const INDEX_PROGRESS_EVENT: &str = "media://keyframe-index";

/// The on-disk cache budget: keyframe indices, waveform peaks and filmstrips
/// together. Evicted least-recently-used beyond this.
const CACHE_BUDGET_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How long shutdown waits for sidecar processes to exit before the window
/// closes anyway. The job object guarantees they die with the process even if
/// this runs out.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Application-wide engine state, managed by Tauri.
#[derive(Debug)]
pub struct MediaEngine {
    orchestrator: Result<Orchestrator, String>,
    prober: Option<Prober>,
    cache: Option<Cache>,
    capabilities: Arc<Mutex<Option<EncoderCapabilities>>>,
    indices: Mutex<HashMap<PathBuf, Arc<KeyframeIndex>>>,
}

impl MediaEngine {
    /// Locate the sidecar installed beside the executable. There is no
    /// fallback to a system FFmpeg (`ADR-0002`); a missing sidecar is carried
    /// as an error and reported the first time anything asks for media work.
    ///
    /// `cache_dir` is the OS cache directory for this application; without
    /// one, derived artefacts are computed but not kept.
    #[must_use]
    pub fn locate(cache_dir: Option<PathBuf>) -> Self {
        let orchestrator = Sidecar::beside_current_exe()
            .map(|sidecar| Orchestrator::new(sidecar, Limits::for_this_machine()))
            .map_err(|error| error.to_string());
        Self {
            prober: orchestrator.as_ref().ok().cloned().map(Prober::new),
            orchestrator,
            cache: cache_dir.map(|dir| Cache::new(dir, CACHE_BUDGET_BYTES)),
            capabilities: Arc::new(Mutex::new(None)),
            indices: Mutex::new(HashMap::new()),
        }
    }

    /// The orchestrator, or why there is none.
    ///
    /// # Errors
    ///
    /// The sidecar is missing from the installation.
    pub fn orchestrator(&self) -> Result<&Orchestrator, String> {
        self.orchestrator.as_ref().map_err(Clone::clone)
    }

    /// Probe the machine's encoders on a background thread. Every trial runs
    /// through the orchestrator at background priority; `CLAUDE.md` section 12
    /// keeps all of it off the UI thread.
    pub fn probe_encoders_in_background(&self) {
        let Ok(orchestrator) = self.orchestrator.clone() else {
            return;
        };
        let slot = Arc::clone(&self.capabilities);
        thread::spawn(move || {
            let found = capability::probe(&orchestrator);
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        });
    }

    /// Cancel every job and wait for its process to exit. Called as the
    /// application closes.
    pub fn shutdown(&self) {
        // Stop background indexing first and keep what it had, so the next
        // launch starts from there rather than from nothing.
        for index in self
            .indices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            index.stop_background();
            index.persist();
        }
        if let Ok(orchestrator) = &self.orchestrator {
            orchestrator.shutdown(SHUTDOWN_TIMEOUT);
        }
    }

    fn prober(&self) -> Result<&Prober, String> {
        self.orchestrator()?;
        self.prober
            .as_ref()
            .ok_or_else(|| "the media engine is not available".to_owned())
    }
}

/// A progress callback that emits [`PROGRESS_EVENT`].
///
/// Pass it to `JobOptions::on_progress`. A failed emit — the window is gone —
/// is ignored: progress is a display, and the job itself must not fail because
/// nobody is watching it.
pub fn forward_progress(app: &AppHandle) -> impl FnMut(JobProgress) + Send + 'static {
    let app = app.clone();
    move |update| {
        let _ = app.emit(PROGRESS_EVENT, update);
    }
}

/// The machine's encoder capability profile.
///
/// `Ok(None)` while the probe is still running, which the renderer shows as
/// "checking", never as "no encoders".
///
/// # Errors
///
/// The sidecar is missing from the installation.
#[tauri::command]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn encoder_capabilities(
    engine: tauri::State<'_, MediaEngine>,
) -> Result<Option<EncoderCapabilities>, String> {
    engine.orchestrator()?;
    Ok(engine
        .capabilities
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone())
}

/// Everything the engine can tell about a media file: streams, codecs,
/// rotation, frame-rate mode, colour and HDR, edit lists, chapters.
///
/// `async` so Tauri runs it off the main thread: a probe is several `ffprobe`
/// processes, and `CLAUDE.md` section 12 allows no media work on the UI
/// thread.
///
/// # Errors
///
/// The sidecar is missing, or the file could not be read — the message names
/// the reason in terms the user can act on.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn probe_media(
    engine: tauri::State<'_, MediaEngine>,
    path: std::path::PathBuf,
) -> Result<MediaInfo, String> {
    engine
        .prober()?
        .probe(&path)
        .map(|info| (*info).clone())
        .map_err(|error| error.to_string())
}

/// Start indexing every keyframe of `path` in the background, and return the
/// fraction already indexed — `1.0` when a previous session finished it.
///
/// Returns at once. Progress arrives on [`INDEX_PROGRESS_EVENT`]; queries do
/// not wait for it, because each reads the region it needs (#24).
///
/// # Errors
///
/// The sidecar is missing, or the file cannot be probed or read.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn index_keyframes(
    app: AppHandle,
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
) -> Result<f64, String> {
    if let Some(index) = engine
        .indices
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&path)
    {
        return Ok(index.fraction());
    }
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    let index = Arc::new(
        KeyframeIndex::open(
            &path,
            &info,
            engine.orchestrator()?.clone(),
            engine.cache.clone(),
        )
        .map_err(|e| e.to_string())?,
    );
    let fraction = index.fraction();
    engine
        .indices
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path, Arc::clone(&index));
    if !index.is_complete() {
        let _ = keyframes::spawn_background(index, move |progress: IndexProgress| {
            let _ = app.emit(INDEX_PROGRESS_EVENT, progress);
        });
    }
    Ok(fraction)
}
