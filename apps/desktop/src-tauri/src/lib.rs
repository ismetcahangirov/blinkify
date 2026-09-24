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

pub mod library;
pub mod lifecycle;
pub mod media;
pub mod project;
pub mod updater;
pub mod window_state;

use blinkify_engine::ExportTier;
use tauri::{Manager, RunEvent, WindowEvent};

use media::MediaEngine;
use project::{LaunchFile, OpenProject};
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
        // Native open and save dialogs for projects and media (#54, #53).
        .plugin(tauri_plugin_dialog::init())
        .manage(PendingUpdate::default())
        // A double-clicked `.blinkify` arrives as the first argument (#32).
        .manage(LaunchFile::from_args(std::env::args()))
        .manage(OpenProject::default())
        // Preview frames are fetched, not invoked: see `media::FRAME_SCHEME`.
        // Each request may wait for its frame to fall due, so it is answered
        // on its own thread and never on the webview's.
        .register_asynchronous_uri_scheme_protocol(media::FRAME_SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_owned();
            std::thread::spawn(move || {
                let engine = app.state::<MediaEngine>();
                // Filmstrip sheets for the timeline (#33) share the scheme:
                // one origin, one CSP entry, and the same off-thread answer.
                let response = media::serve_sheet(&engine, &path)
                    .unwrap_or_else(|| media::serve_frame(&engine, &path));
                responder.respond(response);
            });
        })
        .invoke_handler(tauri::generate_handler![
            tier_is_lossless,
            media::encoder_capabilities,
            media::probe_media,
            media::index_keyframes,
            media::generate_waveform,
            media::waveform_peaks,
            media::generate_filmstrip,
            media::proxy_reasons,
            media::generate_proxy,
            media::cancel_proxy,
            media::open_preview,
            media::transport,
            media::monitor,
            media::monitor_levels,
            media::close_preview,
            media::close_all_previews,
            media::preview_stats,
            project::launch_project,
            project::relink_source,
            project::open_project_preview,
            project::edit_project,
            project::undo_edit,
            project::redo_edit,
            project::begin_gesture,
            project::end_gesture,
            project::preview_settings,
            project::cut_point_at,
            lifecycle::new_project,
            lifecycle::open_project,
            lifecycle::save_project,
            lifecycle::save_project_as,
            lifecycle::close_project,
            lifecycle::autosave_project,
            lifecycle::recent_projects,
            lifecycle::recovery_offers,
            lifecycle::restore_recovery,
            lifecycle::discard_recovery,
            lifecycle::quit_app,
            library::import_media,
            project::operations_at,
            updater::pending_update,
            updater::install_update
        ])
        .setup(|app| {
            // The one outbound request the application is allowed to make
            // (`CLAUDE.md` section 20 rule 8). Spawned, not awaited: the window
            // must appear whether or not the network answers, and it applies
            // nothing on its own — see `updater`.
            updater::check_on_launch(app.handle());

            // The engine needs the OS cache directory, which only exists
            // once the app does — hence managed here rather than on the
            // builder.
            app.manage(MediaEngine::locate(app.path().app_cache_dir().ok()));

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
            if let WindowEvent::CloseRequested { api, .. } = event {
                window_state::save(window.app_handle(), window);
                // Not on unsaved work (#54): the renderer asks the user to
                // save, discard or cancel, and then quits itself.
                if !lifecycle::may_close(window) {
                    api.prevent_close();
                }
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
