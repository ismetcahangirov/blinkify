//! The project lifecycle (#54): new, open, save, save as, close, recent
//! projects, autosave, recovery after an unclean shutdown, and the question
//! before closing the window on unsaved work.
//!
//! The rules live in the engine's `project::session`; this is the command
//! surface over it and the places Tauri knows about — the application's data
//! and settings folders, the window, the preview to tear down on close.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use blinkify_engine::project::recent::{RecentProject, RecentProjects};
use blinkify_engine::project::session::{self, RecoveryOffer, SaveError, Session};
use blinkify_engine::project::{self, Project, SequenceSettings, Track, TrackKind};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use crate::media::MediaEngine;
use crate::project::{OpenProject, Opened, ProjectView};

/// The event the renderer answers when the window is closed on unsaved work.
pub const CLOSE_REQUESTED_EVENT: &str = "project://close-requested";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| u64::try_from(since.as_millis()).ok())
        .unwrap_or(0)
}

/// Where autosaves of never-saved projects go.
fn recovery_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("recovery"))
        .map_err(|error| error.to_string())
}

/// The recent-projects list: a preference, kept with the other settings.
fn recent_file(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("recent-projects.json"))
        .map_err(|error| error.to_string())
}

/// Put `path` first in the recent list. A list that cannot be written costs
/// the user nothing but a shortcut, so it is not an error.
fn remember(app: &AppHandle, path: &Path, name: &str) {
    if let Ok(file) = recent_file(app) {
        let mut recent = RecentProjects::load(&file);
        recent.touch(path, name, now());
        let _ = recent.save(&file);
    }
}

/// "● Trip — Blinkify" while there are unsaved changes, "Trip — Blinkify"
/// otherwise.
#[must_use]
pub fn window_title(name: &str, dirty: bool) -> String {
    let name = name.trim();
    let name = if name.is_empty() {
        "Untitled project"
    } else {
        name
    };
    if dirty {
        format!("● {name} — Blinkify")
    } else {
        format!("{name} — Blinkify")
    }
}

/// Name the window after `view`. A title that cannot be set costs nothing.
pub fn title(app: &AppHandle, view: &ProjectView) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&window_title(&view.project.name, view.dirty));
    }
}

/// Close whatever is open: its preview's decoders exit, its history goes.
fn close_open(engine: &MediaEngine, state: &OpenProject) -> Result<(), String> {
    let previous = state.lock()?.take();
    if let Some(opened) = previous
        && let Some(preview) = opened.preview
    {
        engine.close_preview(preview.session);
    }
    Ok(())
}

/// Install `session` as the open project, and report it.
fn install(
    app: &AppHandle,
    engine: &MediaEngine,
    state: &OpenProject,
    session: Session,
) -> Result<ProjectView, String> {
    close_open(engine, state)?;
    let opened = Opened::new(engine, session);
    let view = opened.view();
    *state.lock()? = Some(opened);
    title(app, &view);
    Ok(view)
}

/// A new project. With `settings` `None` its sequence takes the first video
/// clip's (#57). It starts with one video track and one audio track, as an
/// editor opens, so there is somewhere to drop.
///
/// # Errors
///
/// The settings cannot be used, or the application folders are unavailable.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn new_project(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    name: String,
    settings: Option<SequenceSettings>,
) -> Result<ProjectView, String> {
    let mut project = match settings {
        Some(settings) => {
            settings.validate().map_err(|error| error.to_string())?;
            Project::new(name.trim(), settings)
        }
        None => Project::matching_first_clip(name.trim()),
    };
    project.sequence.tracks = vec![
        Track::new(1, TrackKind::Video, Vec::new()),
        Track::new(2, TrackKind::Audio, Vec::new()),
    ];
    let session = Session::untitled(project, &recovery_dir(&app)?).map_err(|e| e.to_string())?;
    install(&app, &engine, &state, session)
}

/// Open the project file at `path`: from the menu, the recent list, or a
/// double-click in Explorer — one path for all of them.
///
/// # Errors
///
/// The file cannot be read, is damaged, or was saved by a newer version —
/// refused whole, never half-loaded.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn open_project(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    path: PathBuf,
) -> Result<ProjectView, String> {
    let session = Session::open(&path).map_err(|error| error.to_string())?;
    let name = session.document().project().name.clone();
    let view = install(&app, &engine, &state, session)?;
    remember(&app, &path, &name);
    Ok(view)
}

/// Save the open project to its file. A project never saved is refused with
/// a message; the renderer asks where, and calls [`save_project_as`].
///
/// # Errors
///
/// No project is open, it has no file yet, or the file cannot be written —
/// it then stays dirty and its recovery file stays.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn save_project(app: AppHandle, state: State<'_, OpenProject>) -> Result<ProjectView, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened.session.save().map_err(|error| match error {
        SaveError::Untitled => "untitled".to_owned(),
        SaveError::Project(error) => error.to_string(),
    })?;
    let view = opened.view();
    if let Some(path) = opened.session.path() {
        remember(&app, path, &view.project.name);
    }
    title(&app, &view);
    Ok(view)
}

/// Save the open project to `path`, which becomes its file.
///
/// # Errors
///
/// No project is open, or the file cannot be written.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn save_project_as(
    app: AppHandle,
    state: State<'_, OpenProject>,
    path: PathBuf,
) -> Result<ProjectView, String> {
    let path = if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(project::EXTENSION))
    {
        path
    } else {
        path.with_extension(project::EXTENSION)
    };
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened
        .session
        .save_as(&path)
        .map_err(|error| error.to_string())?;
    let view = opened.view();
    remember(&app, &path, &view.project.name);
    title(&app, &view);
    Ok(view)
}

/// Close the open project: its preview's decode processes exit, its undo
/// history goes. With `discard`, its unsaved changes — and their recovery
/// file — go too; the renderer has asked first.
///
/// # Errors
///
/// The project state is unavailable.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn close_project(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    discard: bool,
) -> Result<(), String> {
    let previous = state.lock()?.take();
    if let Some(opened) = previous {
        if let Some(preview) = &opened.preview {
            engine.close_preview(preview.session);
        }
        if discard {
            opened.session.discard();
        }
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title("Blinkify");
    }
    Ok(())
}

/// Write the recovery file if there is anything unsaved and new. The renderer
/// calls this on an interval; significant edits call it at once (see
/// `project::edit_project`). Returns whether it wrote.
///
/// # Errors
///
/// No project is open, or the recovery file cannot be written.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn autosave_project(state: State<'_, OpenProject>) -> Result<bool, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened.session.autosave().map_err(|error| error.to_string())
}

/// The recently opened projects, most recent first.
///
/// # Errors
///
/// The settings folder is unavailable.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn recent_projects(app: AppHandle) -> Result<Vec<RecentProject>, String> {
    Ok(RecentProjects::load(&recent_file(&app)?).entries)
}

/// Unsaved work from a session that did not end cleanly: what the launch
/// offers to restore, with when it was written and when the project was last
/// saved.
///
/// # Errors
///
/// The application folders are unavailable.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn recovery_offers(app: AppHandle) -> Result<Vec<RecoveryOffer>, String> {
    let recent = RecentProjects::load(&recent_file(&app)?);
    Ok(session::scan(&recent.paths(), &recovery_dir(&app)?))
}

/// Reopen from a recovery file, dirty until saved.
///
/// # Errors
///
/// The recovery file cannot be read.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn restore_recovery(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    offer: RecoveryOffer,
) -> Result<ProjectView, String> {
    let session = Session::restore(&offer).map_err(|error| error.to_string())?;
    install(&app, &engine, &state, session)
}

/// Forget an offered recovery: the user chose to discard it.
///
/// # Errors
///
/// The file could not be removed.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
pub fn discard_recovery(offer: RecoveryOffer) -> Result<(), String> {
    session::discard_recovery(&offer).map_err(|error| error.to_string())
}

/// Leave the application — after the renderer has asked about unsaved work.
/// With `discard`, the unsaved changes and their recovery file go.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn quit_app(app: AppHandle, state: State<'_, OpenProject>, discard: bool) {
    if discard
        && let Ok(mut guard) = state.lock()
        && let Some(opened) = guard.take()
    {
        opened.session.discard();
    }
    app.exit(0);
}

/// Whether the window may close now: not while there is unsaved work. When
/// it may not, the renderer is asked to ask the user.
pub fn may_close(window: &Window) -> bool {
    let dirty = window
        .app_handle()
        .state::<OpenProject>()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|opened| opened.session.is_dirty()))
        .unwrap_or(false);
    if dirty {
        let _ = window.emit(CLOSE_REQUESTED_EVENT, ());
    }
    !dirty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_says_when_work_is_unsaved() {
        assert_eq!(window_title("Trip", false), "Trip — Blinkify");
        assert_eq!(window_title("Trip", true), "● Trip — Blinkify");
        assert_eq!(window_title("  ", false), "Untitled project — Blinkify");
    }
}
