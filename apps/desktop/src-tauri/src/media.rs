//! The shell's handle on the media engine.
//!
//! Thin, like the rest of this crate: it finds the bundled sidecar next to the
//! executable, starts the encoder capability probe off the UI thread, and
//! hands the renderer what the engine concluded. It decides nothing itself.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use blinkify_engine::capability::{self, SidecarTrial};
use blinkify_engine::{EncoderCapabilities, Sidecar};

/// Application-wide engine state, managed by Tauri.
#[derive(Debug)]
pub struct MediaEngine {
    sidecar: Result<Sidecar, String>,
    capabilities: Arc<Mutex<Option<EncoderCapabilities>>>,
}

impl MediaEngine {
    /// Locate the sidecar installed beside the executable. There is no
    /// fallback to a system FFmpeg (`ADR-0002`); a missing sidecar is carried
    /// as an error and reported the first time anything asks for media work.
    #[must_use]
    pub fn locate() -> Self {
        Self {
            sidecar: Sidecar::beside_current_exe().map_err(|error| error.to_string()),
            capabilities: Arc::new(Mutex::new(None)),
        }
    }

    /// Probe the machine's encoders on a background thread. Several seconds
    /// of short-lived encoder trials; `CLAUDE.md` section 12 keeps every one of
    /// them off the UI thread.
    pub fn probe_encoders_in_background(&self) {
        let Ok(sidecar) = self.sidecar.clone() else {
            return;
        };
        let slot = Arc::clone(&self.capabilities);
        thread::spawn(move || {
            let found = capability::probe(&SidecarTrial::new(sidecar));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        });
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
    engine.sidecar.as_ref().map_err(Clone::clone)?;
    Ok(engine
        .capabilities
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone())
}
