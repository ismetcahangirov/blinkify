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
use blinkify_engine::filmstrip::{Filmstrip, Filmstrips};
use blinkify_engine::keyframes::{self, IndexProgress, KeyframeIndex};
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::orchestrator::{JobProgress, Limits, Orchestrator};
use blinkify_engine::probe::{MediaInfo, Prober};
use blinkify_engine::proxy::{Proxies, Proxy, ProxyReason, proxy_advice};
use blinkify_engine::waveform::{Peaks, WaveformStatus, Waveforms};
use blinkify_engine::{EncoderCapabilities, Sidecar};
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

/// The event every job's progress arrives on, with a [`JobProgress`] payload.
///
/// One channel for all jobs rather than one per job: the renderer subscribes
/// once and routes by `job`, and the IPC surface stays coarse (`CLAUDE.md`
/// section 2). The engine throttles before anything reaches here.
pub const PROGRESS_EVENT: &str = "media://progress";

/// Background keyframe indexing progress, with an [`IndexProgress`] payload.
pub const INDEX_PROGRESS_EVENT: &str = "media://keyframe-index";

/// Waveform generation progress and completion, with a [`WaveformUpdate`]
/// payload.
pub const WAVEFORM_EVENT: &str = "media://waveform";

/// A filmstrip sheet written, with a `SheetReady` payload.
pub const FILMSTRIP_EVENT: &str = "media://filmstrip";

/// Proxy generation progress, with a `JobProgress` payload.
pub const PROXY_PROGRESS_EVENT: &str = "media://proxy-progress";

/// Proxies get their own directory and budget: an hour of 4K is gigabytes,
/// and sharing the artefact budget would let one proxy evict every waveform.
const PROXY_BUDGET_BYTES: u64 = 50 * 1024 * 1024 * 1024;

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
    proxy_cache: Option<Cache>,
    capabilities: Arc<Mutex<Option<EncoderCapabilities>>>,
    proxies_running: Mutex<HashMap<PathBuf, CancelToken>>,
    indices: Mutex<HashMap<PathBuf, Arc<KeyframeIndex>>>,
    waveforms: Arc<Mutex<HashMap<(PathBuf, u32), Waveform>>>,
}

/// A waveform being generated, or ready to draw.
#[derive(Debug, Clone)]
enum Waveform {
    Pending(f64),
    Ready(Arc<Peaks>),
}

impl Waveform {
    fn status(&self) -> WaveformStatus {
        match self {
            Self::Pending(fraction) => WaveformStatus::Pending {
                fraction: *fraction,
            },
            Self::Ready(_) => WaveformStatus::Ready,
        }
    }
}

/// The payload of [`WAVEFORM_EVENT`].
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WaveformUpdate {
    pub path: String,
    pub stream: u32,
    pub status: WaveformStatus,
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
            cache: cache_dir
                .as_ref()
                .map(|dir| Cache::new(dir.join("artefacts"), CACHE_BUDGET_BYTES)),
            proxy_cache: cache_dir.map(|dir| Cache::new(dir.join("proxies"), PROXY_BUDGET_BYTES)),
            proxies_running: Mutex::new(HashMap::new()),
            capabilities: Arc::new(Mutex::new(None)),
            indices: Mutex::new(HashMap::new()),
            waveforms: Arc::new(Mutex::new(HashMap::new())),
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
        for cancel in self
            .proxies_running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            cancel.cancel();
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

/// Start generating the waveform of audio `stream` of `path`, and say where it
/// stands. Returns at once: `pending` means the timeline draws a placeholder
/// until [`WAVEFORM_EVENT`] reports `ready`.
///
/// # Errors
///
/// The sidecar is missing, or the file cannot be probed.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn generate_waveform(
    app: AppHandle,
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
    stream: u32,
) -> Result<WaveformStatus, String> {
    let key = (path.clone(), stream);
    {
        let mut waveforms = engine
            .waveforms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = waveforms.get(&key) {
            return Ok(existing.status());
        }
        waveforms.insert(key.clone(), Waveform::Pending(0.0));
    }
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    let generator = Waveforms::new(engine.orchestrator()?.clone(), engine.cache.clone());
    let waveforms = Arc::clone(&engine.waveforms);
    thread::spawn(move || {
        let emit = |status: WaveformStatus| {
            let _ = app.emit(
                WAVEFORM_EVENT,
                WaveformUpdate {
                    path: path.display().to_string(),
                    stream,
                    status,
                },
            );
        };
        let progress_app = app.clone();
        let progress_path = path.display().to_string();
        let progress_slots = Arc::clone(&waveforms);
        let progress_key = key.clone();
        let result = generator.peaks(&path, &info, stream, move |fraction| {
            progress_slots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(progress_key.clone(), Waveform::Pending(fraction));
            let _ = progress_app.emit(
                WAVEFORM_EVENT,
                WaveformUpdate {
                    path: progress_path.clone(),
                    stream,
                    status: WaveformStatus::Pending { fraction },
                },
            );
        });
        let mut slots = waveforms.lock().unwrap_or_else(PoisonError::into_inner);
        if let Ok((peaks, _)) = result {
            slots.insert(key, Waveform::Ready(peaks));
            drop(slots);
            emit(WaveformStatus::Ready);
        } else {
            // Forget it, so asking again retries rather than waiting forever.
            slots.remove(&key);
        }
    });
    Ok(WaveformStatus::Pending { fraction: 0.0 })
}

/// The summed `(min, max)` peaks for `pixels` columns starting at
/// `start_seconds`, at `samples_per_pixel`, as little-endian `i16` pairs.
///
/// Binary rather than JSON: a screen of peaks is thousands of numbers per
/// repaint-window change, and the timeline draws them straight into a canvas.
/// Read from the pyramid level for the zoom, so this never decimates the base.
///
/// # Errors
///
/// The waveform has not been generated yet.
#[tauri::command]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn waveform_peaks(
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
    stream: u32,
    samples_per_pixel: f64,
    start_seconds: f64,
    pixels: u32,
) -> Result<tauri::ipc::Response, String> {
    let peaks = match engine
        .waveforms
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&(path, stream))
    {
        Some(Waveform::Ready(peaks)) => Arc::clone(peaks),
        _ => return Err("the waveform is not ready".to_owned()),
    };
    let level = peaks
        .level_for(samples_per_pixel)
        .ok_or_else(|| "the waveform is empty".to_owned())?;
    let seconds_per_bucket =
        f64::from(level.samples_per_bucket) / f64::from(peaks.sample_rate.max(1));
    let buckets_per_pixel = (samples_per_pixel / f64::from(level.samples_per_bucket)).max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let first = (start_seconds.max(0.0) / seconds_per_bucket) as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let span = (f64::from(pixels) * buckets_per_pixel).ceil() as u64;
    let last = first.saturating_add(span).min(level.buckets);
    let bytes: Vec<u8> = peaks
        .summed(level, first..last)
        .flat_map(|(min, max)| [min.to_le_bytes(), max.to_le_bytes()])
        .flatten()
        .collect();
    Ok(tauri::ipc::Response::new(bytes))
}

/// The filmstrip of `path` at `height` pixels, one thumbnail every
/// `interval` seconds (see `filmstrip::interval_for_zoom`). Sheets are
/// announced on [`FILMSTRIP_EVENT`] as each is written, so the timeline fills
/// in from the start while the rest is decoded; the command returns the whole
/// filmstrip when it is done.
///
/// # Errors
///
/// No cache directory, the sidecar is missing, or generation failed.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn generate_filmstrip(
    app: AppHandle,
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
    height: u32,
    interval: f64,
) -> Result<Filmstrip, String> {
    let cache = engine
        .cache
        .clone()
        .ok_or_else(|| "no cache directory is available".to_owned())?;
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    Filmstrips::new(engine.orchestrator()?.clone(), cache)
        .filmstrip(&path, &info, height, interval, None, move |sheet| {
            let _ = app.emit(FILMSTRIP_EVENT, sheet);
        })
        .map_err(|e| e.to_string())
}

/// Why a proxy is worth offering for `path`; empty when it scrubs as it is.
/// Only advice: nothing is generated unless the user asks.
///
/// # Errors
///
/// The sidecar is missing, or the file cannot be probed.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn proxy_reasons(
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
) -> Result<Vec<ProxyReason>, String> {
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    Ok(proxy_advice(&info))
}

/// Make a preview proxy of `path` — resuming a cancelled one — reporting
/// progress on [`PROXY_PROGRESS_EVENT`]. For preview only: nothing in the
/// export path can take a proxy (see `blinkify_engine::proxy`).
///
/// # Errors
///
/// No cache directory, the sidecar is missing, or generation failed or was
/// cancelled.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn generate_proxy(
    app: AppHandle,
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
) -> Result<Proxy, String> {
    let cache = engine
        .proxy_cache
        .clone()
        .ok_or_else(|| "no cache directory is available".to_owned())?;
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    let cancel = CancelToken::default();
    engine
        .proxies_running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.clone(), cancel.clone());
    let result = Proxies::new(engine.orchestrator()?.clone(), cache).generate(
        &path,
        &info,
        &cancel,
        forward_to(app, PROXY_PROGRESS_EVENT),
    );
    engine
        .proxies_running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&path);
    result.map_err(|e| e.to_string())
}

/// Stop generating the proxy of `path`. Its finished segments are kept, so
/// asking again resumes rather than restarts.
#[tauri::command]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn cancel_proxy(engine: tauri::State<'_, MediaEngine>, path: PathBuf) {
    if let Some(cancel) = engine
        .proxies_running
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&path)
    {
        cancel.cancel();
    }
}

fn forward_to(app: AppHandle, event: &'static str) -> impl FnMut(JobProgress) + Send + 'static {
    move |update| {
        let _ = app.emit(event, update);
    }
}
