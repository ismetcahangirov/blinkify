//! The shell's handle on the media engine.
//!
//! Thin, like the rest of this crate: it finds the bundled sidecar next to the
//! executable, owns the one [`Orchestrator`] every sidecar process goes
//! through, forwards job progress to the renderer as events, and hands the
//! renderer what the engine concluded. It decides nothing itself.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use blinkify_engine::capability;
use blinkify_engine::orchestrator::{JobProgress, Limits, Orchestrator};
use blinkify_engine::{EncoderCapabilities, Sidecar};
use tauri::{AppHandle, Emitter};

/// The event every job's progress arrives on, with a [`JobProgress`] payload.
///
/// One channel for all jobs rather than one per job: the renderer subscribes
/// once and routes by `job`, and the IPC surface stays coarse (`CLAUDE.md`
/// section 2). The engine throttles before anything reaches here.
pub const PROGRESS_EVENT: &str = "media://progress";

/// How long shutdown waits for sidecar processes to exit before the window
/// closes anyway. The job object guarantees they die with the process even if
/// this runs out.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Application-wide engine state, managed by Tauri.
#[derive(Debug)]
pub struct MediaEngine {
    orchestrator: Result<Orchestrator, String>,
    capabilities: Arc<Mutex<Option<EncoderCapabilities>>>,
}

impl MediaEngine {
    /// Locate the sidecar installed beside the executable. There is no
    /// fallback to a system FFmpeg (`ADR-0002`); a missing sidecar is carried
    /// as an error and reported the first time anything asks for media work.
    #[must_use]
    pub fn locate() -> Self {
        Self {
            orchestrator: Sidecar::beside_current_exe()
                .map(|sidecar| Orchestrator::new(sidecar, Limits::for_this_machine()))
                .map_err(|error| error.to_string()),
            capabilities: Arc::new(Mutex::new(None)),
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
        if let Ok(orchestrator) = &self.orchestrator {
            orchestrator.shutdown(SHUTDOWN_TIMEOUT);
        }
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
