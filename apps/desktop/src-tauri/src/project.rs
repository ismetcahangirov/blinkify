//! The open project (#32): the one the application was launched with, and
//! relinking its sources.
//!
//! Double-clicking a `.blinkify` file in Explorer starts Blinkify with the
//! file's path as its first argument (the association is registered by the
//! installer, #14). [`LaunchFile`] keeps that argument; the renderer asks for
//! the project with [`launch_project`] once it is ready to show it.
//!
//! Opening a project fingerprints every source, which reads two mebibytes of
//! each, so every command here is `async`: it runs on Tauri's thread pool,
//! never on the window's thread (`CLAUDE.md` section 20 rule 9).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blinkify_engine::project::{self, ClipId, Project, SourceId, SourceStatus};
use blinkify_engine::proxy::MediaAsset;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

/// The project file the application was started with, if any.
#[derive(Debug, Default)]
pub struct LaunchFile(pub Option<PathBuf>);

impl LaunchFile {
    /// The first argument, if it names a project file. Anything else — no
    /// argument, a media file, a flag — is not a project to open.
    #[must_use]
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Self {
        let _program = args.next();
        Self(args.next().map(PathBuf::from).filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case(project::EXTENSION))
        }))
    }
}

/// The project that is open, and where it lives.
#[derive(Debug, Default)]
pub struct OpenProject(pub Mutex<Option<(PathBuf, Project)>>);

/// A project as the renderer shows it: the graph, and what opening it found
/// about its sources.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectView {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub project: Project,
    /// Every source that is not present, and why.
    pub unavailable: BTreeMap<SourceId, SourceStatus>,
    /// The clips that play an unavailable source — the ones to mark.
    pub affected_clips: Vec<ClipId>,
}

impl ProjectView {
    fn of(path: &Path, project: &Project) -> Self {
        let unavailable: BTreeMap<SourceId, SourceStatus> = project
            .check_sources()
            .into_iter()
            .filter(|(_, status)| !status.is_present())
            .collect();
        let affected_clips = unavailable
            .keys()
            .flat_map(|&source| project.clips_of(source))
            .collect();
        Self {
            path: path.to_path_buf(),
            project: project.clone(),
            unavailable,
            affected_clips,
        }
    }
}

fn open(state: &OpenProject, path: &Path) -> Result<ProjectView, String> {
    let project = Project::load(path).map_err(|error| error.to_string())?;
    let view = ProjectView::of(path, &project);
    *state
        .0
        .lock()
        .map_err(|_| "the project state is poisoned")? = Some((path.to_path_buf(), project));
    Ok(view)
}

/// The project Blinkify was launched with, opened — or `None` when it was
/// launched without one.
///
/// # Errors
///
/// The file could not be opened; the message says why, in words for the
/// user (damaged, or from a newer version).
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn launch_project(
    app: AppHandle,
    launch: State<'_, LaunchFile>,
    state: State<'_, OpenProject>,
) -> Result<Option<ProjectView>, String> {
    let Some(path) = launch.0.as_deref() else {
        return Ok(None);
    };
    let view = open(&state, path)?;
    // The taskbar names the project, as the app bar does. A title that
    // cannot be set costs the user nothing, so it is not an error.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_title(&window_title(&view.project.name));
    }
    Ok(Some(view))
}

/// "Trip — Blinkify", or plain "Blinkify" for a project with no name.
fn window_title(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        "Blinkify".to_owned()
    } else {
        format!("{name} — Blinkify")
    }
}

/// Point `source` at the file at `path` — the same content, moved — and save
/// the project so it opens without asking next time.
///
/// # Errors
///
/// No project is open, the file is a different one, or the project could not
/// be saved.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn relink_source(
    state: State<'_, OpenProject>,
    source: SourceId,
    path: PathBuf,
) -> Result<ProjectView, String> {
    let mut guard = state
        .0
        .lock()
        .map_err(|_| "the project state is poisoned")?;
    let (file, project) = guard.as_mut().ok_or("no project is open")?;
    // Opened read-only to fingerprint, and never written (`CLAUDE.md`
    // section 19). What is saved is the project file.
    project
        .relink(source, &MediaAsset::new(path).export_source())
        .map_err(|error| error.to_string())?;
    project.save(file).map_err(|error| error.to_string())?;
    Ok(ProjectView::of(file, project))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|arg| (*arg).to_owned())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn the_window_is_named_after_the_project() {
        assert_eq!(window_title("Trip"), "Trip — Blinkify");
        assert_eq!(window_title("  "), "Blinkify");
    }

    #[test]
    fn only_a_project_file_argument_is_a_project_to_open() {
        assert_eq!(
            LaunchFile::from_args(args(&["blinkify.exe", "C:\\Work\\Trip.blinkify"])).0,
            Some(PathBuf::from("C:\\Work\\Trip.blinkify"))
        );
        assert_eq!(
            LaunchFile::from_args(args(&["blinkify.exe", "D:\\Trip.BLINKIFY"])).0,
            Some(PathBuf::from("D:\\Trip.BLINKIFY"))
        );
        assert_eq!(LaunchFile::from_args(args(&["blinkify.exe"])).0, None);
        assert_eq!(
            LaunchFile::from_args(args(&["blinkify.exe", "clip.mp4"])).0,
            None
        );
    }
}
