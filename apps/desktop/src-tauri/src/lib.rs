//! The Tauri shell.
//!
//! `CLAUDE.md` section 2: this layer stays thin — window lifecycle, the command
//! surface, the event channel. Every media decision lives in
//! [`blinkify_engine`], which knows nothing about Tauri and is tested without
//! it.
//!
//! The command surface is deliberately coarse. Tauri 2 IPC serialises across a
//! process boundary, so a per-frame or per-clip command is a performance bug
//! waiting to be written.

use blinkify_engine::ExportTier;

/// Whether a given export tier leaves the pixels untouched.
///
/// A placeholder command that proves the renderer reaches the engine through
/// the shell rather than around it. Note the direction: the renderer asks the
/// engine, it never decides for itself.
#[tauri::command]
#[must_use]
fn tier_is_lossless(tier: ExportTier) -> bool {
    tier.is_lossless()
}

/// Build and run the application.
///
/// # Panics
///
/// Panics if the Tauri context cannot be built or the event loop fails to
/// start. Both mean the window can never appear, so there is nothing to
/// degrade to.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
#[allow(clippy::expect_used)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![tier_is_lossless])
        .run(tauri::generate_context!())
        .expect("the Blinkify window could not be created");
}

pub fn scratch_broken() -> u32 { "not a u32" }
