//! The shell's handle on the media engine.
//!
//! Thin, like the rest of this crate: it finds the bundled sidecar next to the
//! executable, owns the one [`Orchestrator`] every sidecar process goes
//! through, forwards job progress to the renderer as events, and hands the
//! renderer what the engine concluded. It decides nothing itself.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use blinkify_engine::audio::MonitorLevels;
use blinkify_engine::audio::denoise::{ModelError, Models};
use blinkify_engine::audio::loudness::{Loudness, MeasureRequest, measure_cached};
use blinkify_engine::cache::Cache;
use blinkify_engine::capability;
use blinkify_engine::filmstrip::{self, Filmstrip, Filmstrips};
use blinkify_engine::keyframes::{self, IndexProgress, KeyframeIndex};
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::orchestrator::{JobProgress, Limits, Orchestrator};
use blinkify_engine::playback::{
    AudioChoice, DecodeStats, DefaultDevice, MonitorCommand, MonitorStatus, PlaybackPlan,
    PlaybackStatus, Player, PlayerOptions, SourceMedia, TransportCommand,
};
use blinkify_engine::probe::{MediaInfo, Prober, StreamKind};
use blinkify_engine::project::asset::AssetInfo;
use blinkify_engine::project::trim::StreamExtent;
use blinkify_engine::project::{StreamGeometry, TrackKind};
use blinkify_engine::proxy::{Proxies, Proxy, ProxyReason, proxy_advice};
use blinkify_engine::waveform::{Peaks, WaveformStatus, Waveforms};
use blinkify_engine::{EncoderCapabilities, Sidecar};
use serde::Serialize;
use tauri::http::{Response, StatusCode, header};
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

/// The URI scheme preview frames are served on: `frame://localhost/<session>/<after>`,
/// which `WebView2` reaches as `http://frame.localhost/<session>/<after>`.
///
/// A scheme rather than a command or an event, because a frame is megabytes of
/// raw bytes: a command result or an event payload is serialised, and a
/// `fetch` of a custom scheme is not. See `docs/architecture/preview-pipeline.md`.
pub const FRAME_SCHEME: &str = "frame";

/// How long a frame request waits for a newer frame before answering "none
/// yet". Long enough that an idle preview is not a busy loop, short enough
/// that a closed session is noticed promptly.
const FRAME_WAIT: Duration = Duration::from_millis(50);

/// Transport state changes — play, pause, the end, a speed, a loop, a new audio
/// device — with a [`PlaybackUpdate`] payload. Positions arrive with frames,
/// not here.
pub const PLAYBACK_EVENT: &str = "media://playback";

/// How long shutdown waits for sidecar processes to exit before the window
/// closes anyway. The job object guarantees they die with the process even if
/// this runs out.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Where the bundled models are installed, under the resource directory: the
/// bundler's `resources` map in `tauri.conf.json` puts them there.
const MODELS_DIR: &str = "rnnoise";

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
    /// Files whose index is being completed in the background.
    indexing: Mutex<HashSet<PathBuf>>,
    waveforms: Arc<Mutex<HashMap<(PathBuf, u32), Waveform>>>,
    previews: Mutex<HashMap<u32, Arc<Player>>>,
    next_preview: AtomicU32,
    /// The bundled noise reduction model (#47), or why it cannot be used.
    models: Result<Models, ModelError>,
    /// The last sequence normalisation measured (#48): the digest of the
    /// clips it was measured on, and its gain.
    sequence_gain: Mutex<Option<(String, f64)>>,
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
    pub fn locate(cache_dir: Option<PathBuf>, resource_dir: Option<PathBuf>) -> Self {
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
            indexing: Mutex::new(HashSet::new()),
            waveforms: Arc::new(Mutex::new(HashMap::new())),
            previews: Mutex::new(HashMap::new()),
            next_preview: AtomicU32::new(1),
            // Installed with the application, never fetched (#47).
            models: Models::in_dir(&resource_dir.unwrap_or_default().join(MODELS_DIR)),
            sequence_gain: Mutex::new(None),
        }
    }

    /// The content-keyed cache, if there is a cache directory.
    pub(crate) fn cache(&self) -> Option<&Cache> {
        self.cache.as_ref()
    }

    /// The sequence gain measured for the clips with digest `clips`, if the
    /// last measurement was of exactly them.
    pub(crate) fn sequence_gain(&self, clips: &str) -> Option<f64> {
        self.sequence_gain
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(digest, _)| digest == clips)
            .map(|(_, gain)| *gain)
    }

    pub(crate) fn remember_sequence_gain(&self, clips: String, gain: f64) {
        *self
            .sequence_gain
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((clips, gain));
    }

    /// The bundled models, if they are installed and intact.
    pub(crate) fn models(&self) -> Option<&Models> {
        self.models.as_ref().ok()
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
        // Previews first: each close waits for its decoder process to exit.
        self.close_all_previews();
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

    /// Close every preview session and wait for each decoder to exit. What
    /// closing a project does, and what the application does on its way out.
    pub fn close_all_previews(&self) {
        let sessions: Vec<_> = self
            .previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, session)| session)
            .collect();
        for session in sessions {
            session.close();
        }
    }

    pub(crate) fn preview(&self, session: u32) -> Option<Arc<Player>> {
        self.previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&session)
            .cloned()
    }

    /// The keyframe index of `path`, opened once and shared by background
    /// indexing and by every preview of the file.
    fn index(&self, path: &Path, info: &MediaInfo) -> Result<Arc<KeyframeIndex>, String> {
        let mut indices = self.indices.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = indices.get(path) {
            return Ok(Arc::clone(index));
        }
        let index = Arc::new(
            KeyframeIndex::open(path, info, self.orchestrator()?.clone(), self.cache.clone())
                .map_err(|e| e.to_string())?,
        );
        indices.insert(path.to_path_buf(), Arc::clone(&index));
        Ok(index)
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
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    let index = engine.index(&path, &info)?;
    let fraction = index.fraction();
    // A preview opens the same index without completing it in the background;
    // the first call here starts that, once per file.
    let first_request = engine
        .indexing
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path);
    if first_request && !index.is_complete() {
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
/// `start_seconds`, at `pixels_per_second` of the source, as little-endian
/// `i16` pairs. The timeline asks in its own unit, the zoom; the samples per
/// pixel are worked out here, from the stream's real sample rate.
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
    pixels_per_second: f64,
    start_seconds: f64,
    pixels: u32,
) -> Result<tauri::ipc::Response, String> {
    if !(pixels_per_second.is_finite() && pixels_per_second > 0.0) {
        return Err("the zoom is not a zoom".to_owned());
    }
    let peaks = match engine
        .waveforms
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&(path, stream))
    {
        Some(Waveform::Ready(peaks)) => Arc::clone(peaks),
        _ => return Err("the waveform is not ready".to_owned()),
    };
    let samples_per_pixel = f64::from(peaks.sample_rate.max(1)) / pixels_per_second;
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

/// The filmstrip of `path` at `height` pixels, dense enough for a timeline
/// zoom of `pixels_per_second`: one thumbnail per tile width, on the
/// power-of-two ladder of `filmstrip::interval_for_zoom`, so zooming reuses a
/// sheet already made. Sheets are
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
    pixels_per_second: f64,
) -> Result<Filmstrip, String> {
    let cache = engine
        .cache
        .clone()
        .ok_or_else(|| "no cache directory is available".to_owned())?;
    let info = engine.prober()?.probe(&path).map_err(|e| e.to_string())?;
    let (_, video) = info
        .video()
        .find(|(_, video)| !video.is_attached_picture)
        .ok_or_else(|| "the file has no pictures".to_owned())?;
    let interval = filmstrip::interval_for_zoom(
        pixels_per_second,
        filmstrip::tile_width(video.display_width, video.display_height, height),
    );
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

/// A preview session, as the renderer receives it when one opens.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreviewOpened {
    pub session: u32,
    pub status: PlaybackStatus,
}

/// The payload of [`PLAYBACK_EVENT`].
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlaybackUpdate {
    pub session: u32,
    pub status: PlaybackStatus,
}

/// Open a preview of `path` sized for a `max_width` by `max_height` surface,
/// paused on its first frame. Frames are then fetched from [`FRAME_SCHEME`],
/// never returned through IPC; transport goes through [`transport`].
///
/// # Errors
///
/// The sidecar is missing, or the file cannot be probed, indexed or played.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn open_preview(
    app: AppHandle,
    engine: tauri::State<'_, MediaEngine>,
    path: PathBuf,
    max_width: u32,
    max_height: u32,
) -> Result<PreviewOpened, String> {
    let source = engine.source_media(&path)?;
    let plan = PlaybackPlan::whole(Arc::new(source)).map_err(|e| e.to_string())?;
    engine.start_preview(app, plan, max_width, max_height)
}

impl MediaEngine {
    /// A file ready to preview: probed, indexed, and with the proxy already
    /// made for it, if there is one — one is never made here
    /// (`proxy_reasons` offers, the user decides).
    ///
    /// # Errors
    ///
    /// The sidecar is missing, or the file cannot be probed or indexed.
    /// The keyframe index of the file at `path` (#24), opened once and kept.
    pub(crate) fn keyframe_index(&self, path: &Path) -> Result<Arc<KeyframeIndex>, String> {
        let info = self.prober()?.probe(path).map_err(|e| e.to_string())?;
        self.index(path, &info)
    }

    /// What the export planner (#39) knows about the file at `path`: its
    /// probe and its keyframe index, as far as the index has got.
    pub(crate) fn export_facts(
        &self,
        path: &Path,
    ) -> Result<blinkify_engine::export::plan::SourceFacts, String> {
        let info = self.prober()?.probe(path).map_err(|e| e.to_string())?;
        let index = self.index(path, &info)?;
        let encoders = self
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Ok(blinkify_engine::export::facts::source_facts(
            &info,
            Some(&index),
            encoders.as_ref(),
        ))
    }

    /// What the file at `path` is, for the document: its pictures — the
    /// first video stream that is not a cover image — for the sequence
    /// settings (#57), which ticks of each stream exist, for trim bounds
    /// (#34), and what the library shows (#53). Probed once, here. Nothing
    /// when it cannot be probed.
    pub(crate) fn facts_of(&self, path: &Path) -> SourceFacts {
        match self
            .prober()
            .and_then(|p| p.probe(path).map_err(|e| e.to_string()))
        {
            Ok(info) => SourceFacts::of(&info),
            Err(_) => SourceFacts::default(),
        }
    }

    /// Probe `path` for an import: its facts, or why it cannot be used.
    pub(crate) fn import_facts(&self, path: &Path) -> Result<SourceFacts, String> {
        let info = self
            .prober()?
            .probe(path)
            .map_err(|error| error.to_string())?;
        let facts = SourceFacts::of(&info);
        if facts.asset.is_none() {
            return Err("it has no pictures or sound Blinkify can use".to_owned());
        }
        Ok(facts)
    }

    /// Close one preview session; returns once its decoders have exited.
    pub(crate) fn close_preview(&self, session: u32) {
        let removed = self
            .previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session);
        if let Some(removed) = removed {
            removed.close();
        }
    }

    /// The file's probe: what its streams are.
    pub(crate) fn info_of(&self, path: &Path) -> Result<Arc<MediaInfo>, String> {
        self.prober()?.probe(path).map_err(|e| e.to_string())
    }

    /// How loud `request`'s stretch is, from the content-keyed cache when it
    /// was measured before (Epic #7). Blocks for a full decode otherwise; the
    /// callers are async commands, off the UI thread.
    pub(crate) fn loudness(&self, request: &MeasureRequest) -> Result<Loudness, String> {
        measure_cached(
            self.orchestrator()?,
            self.cache.as_ref(),
            request,
            &CancelToken::default(),
        )
        .map_err(|error| error.to_string())
    }

    pub(crate) fn source_media(&self, path: &Path) -> Result<SourceMedia, String> {
        let info = self.prober()?.probe(path).map_err(|e| e.to_string())?;
        let index = self.index(path, &info)?;
        let proxy = self.proxy_cache.clone().and_then(|cache| {
            Proxies::new(self.orchestrator().ok()?.clone(), cache)
                .find(path)
                .ok()
                .flatten()
        });
        Ok(SourceMedia::new(path, info, index)
            .map_err(|e| e.to_string())?
            .with_proxy(proxy))
    }

    /// A preview session playing `plan`, reporting on [`PLAYBACK_EVENT`].
    ///
    /// # Errors
    ///
    /// The sidecar is missing.
    pub(crate) fn start_preview(
        &self,
        app: AppHandle,
        plan: PlaybackPlan,
        max_width: u32,
        max_height: u32,
    ) -> Result<PreviewOpened, String> {
        let engine = self;
        let player = Player::new(
            engine.orchestrator()?.clone(),
            plan,
            &PlayerOptions {
                audio: AudioChoice::Device,
                max_width,
                max_height,
                default_device: DefaultDevice::System,
                models: engine.models().cloned(),
            },
        );
        let session = engine.next_preview.fetch_add(1, Ordering::Relaxed);
        player.set_listener(move |status| {
            let _ = app.emit(PLAYBACK_EVENT, PlaybackUpdate { session, status });
        });
        let opened = PreviewOpened {
            session,
            status: player.status(),
        };
        engine
            .previews
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session, Arc::new(player));
        Ok(opened)
    }
}

/// Play, pause, step, seek, change speed or loop a preview session, and
/// return where it is afterwards. One command for the whole transport keeps
/// the IPC surface coarse.
///
/// # Errors
///
/// No such session.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn transport(
    engine: tauri::State<'_, MediaEngine>,
    session: u32,
    command: TransportCommand,
) -> Result<PlaybackStatus, String> {
    engine
        .preview(session)
        .map(|player| player.command(command))
        .ok_or_else(|| format!("no preview session {session}"))
}

/// Close a preview session. Returns once its decoder process has exited.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn close_preview(engine: tauri::State<'_, MediaEngine>, session: u32) {
    engine.close_preview(session);
}

/// Close every preview session: what closing a project does.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn close_all_previews(engine: tauri::State<'_, MediaEngine>) {
    engine.close_all_previews();
}

/// Change what the editor hears in a preview session — volume, mute, a
/// track's solo or mute — or put out the clip indication. Monitoring only:
/// nothing here reaches an export.
///
/// # Errors
///
/// No such session.
#[tauri::command]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn monitor(
    engine: tauri::State<'_, MediaEngine>,
    session: u32,
    command: MonitorCommand,
) -> Result<MonitorStatus, String> {
    engine
        .preview(session)
        .map(|player| player.monitor(command))
        .ok_or_else(|| format!("no preview session {session}"))
}

/// The meter of a preview session: peaks, short-term loudness, clip.
///
/// # Errors
///
/// No such session.
#[tauri::command]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn monitor_levels(
    engine: tauri::State<'_, MediaEngine>,
    session: u32,
) -> Result<MonitorLevels, String> {
    engine
        .preview(session)
        .map(|player| player.levels())
        .ok_or_else(|| format!("no preview session {session}"))
}

/// Decode statistics for a preview session, for the diagnostic overlay.
///
/// # Errors
///
/// No such session.
#[tauri::command]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn preview_stats(
    engine: tauri::State<'_, MediaEngine>,
    session: u32,
) -> Result<DecodeStats, String> {
    engine
        .preview(session)
        .map(|session| session.stats())
        .ok_or_else(|| format!("no preview session {session}"))
}

/// Parse a frame request path, `/<session>/<after>`.
fn frame_request(path: &str) -> Option<(u32, u64)> {
    let mut parts = path.trim_matches('/').split('/');
    let session = parts.next()?.parse().ok()?;
    let after = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((session, after))
}

/// Answer one request on [`FRAME_SCHEME`]: `200` with the frame to show now,
/// in the wire format of `blinkify_engine::playback::ShownFrame::wire`; `204` when no
/// newer frame became due within [`FRAME_WAIT`]; `404` for an unknown or
/// closed session. Blocks for up to [`FRAME_WAIT`], so it is called off the
/// webview's thread.
///
/// Backpressure is in the shape of the exchange: the renderer holds at most
/// one request open, so a renderer that falls behind is shown fewer, newer
/// frames, and nothing queues for it.
#[must_use]
pub fn serve_frame(engine: &MediaEngine, path: &str) -> Response<Vec<u8>> {
    let respond = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::CACHE_CONTROL, "no-store")
            // The page is served from a different origin than the scheme.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(body)
            .unwrap_or_default()
    };
    let Some((session, after)) = frame_request(path) else {
        return respond(StatusCode::BAD_REQUEST, Vec::new());
    };
    let Some(preview) = engine.preview(session) else {
        return respond(StatusCode::NOT_FOUND, Vec::new());
    };
    match preview.next_frame(after, FRAME_WAIT) {
        Some(frame) => respond(StatusCode::OK, frame.wire()),
        None => respond(StatusCode::NO_CONTENT, Vec::new()),
    }
}

/// What the document is told about a source: read from one probe.
#[derive(Debug, Default)]
pub(crate) struct SourceFacts {
    pub(crate) geometry: Option<StreamGeometry>,
    pub(crate) extents: Vec<StreamExtent>,
    pub(crate) asset: Option<AssetInfo>,
}

impl SourceFacts {
    fn of(info: &MediaInfo) -> Self {
        Self {
            geometry: info
                .video()
                .filter(|(_, video)| !video.is_attached_picture)
                .find_map(|(_, video)| StreamGeometry::of(video)),
            extents: extents(info),
            asset: AssetInfo::of(info),
        }
    }
}

/// Which ticks of each stream of a probed file exist, in the stream's time
/// base. The probe reports seconds; they are converted once here, the start
/// to the nearest tick and the end down, so a trim bound never names a tick
/// the file does not have.
fn extents(info: &MediaInfo) -> Vec<StreamExtent> {
    info.streams
        .iter()
        .filter_map(|stream| {
            let kind = match stream.kind {
                StreamKind::Video(_) => TrackKind::Video,
                StreamKind::Audio(_) => TrackKind::Audio,
                _ => return None,
            };
            let time_base = stream.time_base.filter(|tb| tb.num > 0 && tb.den > 0)?;
            let duration = stream
                .duration_seconds
                .or(info.container.duration_seconds)
                .filter(|d| d.is_finite() && *d > 0.0)?;
            let begin = stream
                .start_seconds
                .filter(|s| s.is_finite())
                .unwrap_or(0.0);
            #[allow(clippy::cast_precision_loss)]
            let per_second = time_base.den as f64 / time_base.num as f64;
            #[allow(clippy::cast_possible_truncation)]
            let start = (begin * per_second).round() as i64;
            #[allow(clippy::cast_possible_truncation)]
            let end = ((begin + duration) * per_second).floor() as i64;
            (end > start).then_some(StreamExtent {
                source: 0,
                stream: stream.index,
                kind,
                time_base,
                start,
                end,
            })
        })
        .collect()
}

/// The sheet path a request on [`FRAME_SCHEME`] names: `/sheet/<path>`, the
/// path percent-encoded. `None` for anything else.
fn sheet_request(path: &str) -> Option<PathBuf> {
    let encoded = path.strip_prefix("/sheet/")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            decoded.push(byte);
            i += 1;
        }
    }
    String::from_utf8(decoded).ok().map(PathBuf::from)
}

/// Answer a filmstrip sheet request: the JPEG, if the path is a sheet inside
/// the engine's cache. Nothing outside the cache is ever served — the
/// scheme is not a way to read the disk.
pub fn serve_sheet(engine: &MediaEngine, path: &str) -> Option<Response<Vec<u8>>> {
    let sheet = sheet_request(path)?;
    let respond = |status: StatusCode, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(body)
            .unwrap_or_default()
    };
    let inside = engine
        .cache
        .as_ref()
        .is_some_and(|cache| cache.contains(&sheet))
        && sheet
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"));
    if !inside {
        return Some(respond(StatusCode::FORBIDDEN, Vec::new()));
    }
    Some(match std::fs::read(&sheet) {
        Ok(bytes) => respond(StatusCode::OK, bytes),
        Err(_) => respond(StatusCode::NOT_FOUND, Vec::new()),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn stream_extents_are_whole_ticks_the_file_has() {
        let info: MediaInfo = serde_json::from_value(serde_json::json!({
            "container": {
                "formatName": "mov,mp4",
                "formatLongName": null,
                "durationSeconds": 10.01,
                "bitRate": null,
                "sizeBytes": 1,
                "editLists": [],
                "hasEditList": false,
                "chapters": []
            },
            "streams": [
                {
                    "index": 0, "codec": "h264", "profile": null, "level": null,
                    "bitRate": null, "durationSeconds": 10.01, "startSeconds": 0.0,
                    "timeBase": { "num": 1, "den": 90000 }, "isDefault": true,
                    "kind": { "type": "audio", "sampleRate": 48000, "channels": 2, "channelLayout": null, "sampleFormat": null, "bitsPerSample": null }
                },
                {
                    "index": 1, "codec": "aac", "profile": null, "level": null,
                    "bitRate": null, "durationSeconds": null, "startSeconds": 0.021_333,
                    "timeBase": { "num": 1, "den": 48000 }, "isDefault": true,
                    "kind": { "type": "audio", "sampleRate": 48000, "channels": 2, "channelLayout": null, "sampleFormat": null, "bitsPerSample": null }
                },
                {
                    "index": 2, "codec": null, "profile": null, "level": null,
                    "bitRate": null, "durationSeconds": 3.0, "startSeconds": null,
                    "timeBase": null, "isDefault": false,
                    "kind": { "type": "subtitle" }
                }
            ]
        }))
        .expect("media info");
        let found = extents(&info);
        // A stream with no time base cannot be bounded in ticks.
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].stream, found[0].start, found[0].end),
            (0, 0, 900_900)
        );
        // No stream duration: the container's, from the stream's start.
        assert_eq!(found[1].start, 1024);
        assert_eq!(found[1].end, 481_503);
    }

    #[test]
    fn a_sheet_request_names_a_percent_encoded_path() {
        assert_eq!(
            sheet_request("/sheet/C%3A%5CCache%5Cfilmstrip%5Cab-h48-i1000-0.jpg"),
            Some(PathBuf::from(r"C:\Cache\filmstrip\ab-h48-i1000-0.jpg"))
        );
        assert_eq!(sheet_request("/3/41"), None);
        assert_eq!(sheet_request("/sheet/%zz"), None);
        assert_eq!(sheet_request("/sheet/%4"), None);
    }

    #[test]
    fn a_sheet_in_the_cache_is_served_and_nothing_outside_it() {
        let dir = std::env::temp_dir().join("blinkify-sheet-test");
        let sheets = dir.join("artefacts").join("filmstrip");
        std::fs::create_dir_all(&sheets).expect("dir");
        let sheet = sheets.join("ab-h48-i1000-0.jpg");
        std::fs::write(&sheet, b"jpeg").expect("sheet");
        let engine = MediaEngine::locate(Some(dir), None);
        let inside = format!("/sheet/{}", sheet.display().to_string().replace('%', "%25"));
        let response = serve_sheet(&engine, &inside).expect("a sheet request");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), b"jpeg");

        let outside = std::env::current_exe().expect("exe");
        let request = format!(
            "/sheet/{}",
            outside.display().to_string().replace('%', "%25")
        );
        let response = serve_sheet(&engine, &request).expect("a sheet request");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let climbing = "/sheet/..%2F..%2F..%2FWindows%2Fwin.ini";
        assert_eq!(
            serve_sheet(&engine, climbing)
                .expect("a sheet request")
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn a_frame_request_names_a_session_and_the_last_frame_seen() {
        assert_eq!(frame_request("/3/41"), Some((3, 41)));
        assert_eq!(frame_request("3/0"), Some((3, 0)));
        assert_eq!(frame_request("/3"), None);
        assert_eq!(frame_request("/3/41/extra"), None);
        assert_eq!(frame_request("/x/1"), None);
    }

    #[test]
    fn an_unknown_session_is_not_found_rather_than_a_hang() {
        let engine = MediaEngine::locate(None, None);
        assert_eq!(
            serve_frame(&engine, "/99/0").status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_frame(&engine, "/nonsense").status(),
            StatusCode::BAD_REQUEST
        );
    }
}
