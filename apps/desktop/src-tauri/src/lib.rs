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

pub mod media;
pub mod updater;
pub mod window_state;

use blinkify_engine::ExportTier;
use tauri::{Manager, RunEvent, WindowEvent};

use media::MediaEngine;
use updater::PendingUpdate;

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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(PendingUpdate::default())
        .manage(MediaEngine::locate())
        .invoke_handler(tauri::generate_handler![
            tier_is_lossless,
            media::encoder_capabilities,
            updater::pending_update,
            updater::install_update
        ])
        .setup(|app| {
            // The one outbound request the application is allowed to make
            // (`CLAUDE.md` section 20 rule 8). Spawned, not awaited: the window
            // must appear whether or not the network answers, and it applies
            // nothing on its own — see `updater`.
            updater::check_on_launch(app.handle());

            // ADR-0003 part 1: what this machine can encode is measured, not
            // assumed, and measured before anything needs the answer.
            app.state::<MediaEngine>().probe_encoders_in_background();

            // Put the window back where the user left it, if that is still
            // somewhere they can see it. `window_state::restore` validates the
            // saved rectangle against the monitors attached right now, because
            // a window restored onto a display that has been unplugged is
            // running, invisible, and unreachable.
            if let Some(window) = app.get_webview_window("main") {
                window_state::restore(app.handle(), &window);
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Saved on the way out rather than on every move and resize. A
            // drag emits hundreds of events and writing the file on each one
            // would put a disk write in the middle of a window drag; the only
            // geometry that matters is the one the window had when it closed.
            //
            // The cost is that a crash loses the position. That is the right
            // trade for a preference: the recovery is that the window opens
            // where it opened last time, which is where it would have opened
            // anyway.
            if matches!(event, WindowEvent::CloseRequested { .. }) {
                window_state::save(window.app_handle(), window);
            }
        })
        .build(tauri::generate_context!())
        .expect("the Blinkify window could not be created")
        .run(|app, event| {
            // No sidecar process outlives the window. This is the orderly
            // path; the engine's job object covers the disorderly one.
            if matches!(event, RunEvent::Exit) {
                app.state::<MediaEngine>().shutdown();
            }
        });
}
