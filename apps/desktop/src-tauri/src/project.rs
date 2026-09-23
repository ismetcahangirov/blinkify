//! The open project (#32): the one the application was launched with, and
//! relinking its sources.
//!
//! Double-clicking a `.blinkify` file in Explorer starts Blinkify with the
//! file's path as its first argument (the association is registered by the
//! installer, #14). [`LaunchFile`] keeps that argument; the renderer asks for
//! the project with [`launch_project`] once it is ready to show it.
//!
//! The project is previewed through the shared evaluator (#30):
//! [`open_project_preview`] plays it, [`update_project`] replaces the graph
//! and the preview follows without restarting what did not change, and
//! [`operations_at`] is the diagnostic view — what the evaluator applies
//! under the playhead, which is what the export will apply there.
//!
//! Opening a project fingerprints every source, which reads two mebibytes of
//! each, so every command here is `async`: it runs on Tauri's thread pool,
//! never on the window's thread (`CLAUDE.md` section 20 rule 9).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use blinkify_engine::playback::{PlaybackPlan, SourceMedia, chain_rendered};
use blinkify_engine::project::evaluate::{OperationsAt, Timeline, audio_operation, evaluate};
use blinkify_engine::project::{self, ClipId, Operation, Project, SourceId, SourceStatus};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::time::{self, MICROSECONDS, Rounding};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;

use crate::media::{MediaEngine, PreviewOpened};

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

/// The project that is open, where it lives, and its preview.
#[derive(Debug, Default)]
pub struct OpenProject(Mutex<Option<Opened>>);

#[derive(Debug)]
struct Opened {
    path: PathBuf,
    project: Project,
    /// The graph as the evaluator last resolved it.
    timeline: Option<Timeline>,
    preview: Option<ProjectPreview>,
}

/// A preview session playing the project, and the sources it has opened —
/// kept, so a graph change does not probe or index a file again.
#[derive(Debug)]
struct ProjectPreview {
    session: u32,
    sources: BTreeMap<SourceId, Arc<SourceMedia>>,
}

impl OpenProject {
    fn lock(&self) -> Result<MutexGuard<'_, Option<Opened>>, String> {
        self.0
            .lock()
            .map_err(|_| "the project state is poisoned".to_owned())
    }
}

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
    *state.lock()? = Some(Opened {
        path: path.to_path_buf(),
        timeline: evaluate(&project).ok(),
        project,
        preview: None,
    });
    Ok(view)
}

/// Evaluate `opened`'s graph and, if it is previewed, give the player the new
/// plan: sources it has not opened yet are opened, the rest are reused.
fn refresh(engine: &MediaEngine, opened: &mut Opened) -> Result<(), String> {
    let timeline = evaluate(&opened.project).map_err(|error| error.to_string())?;
    if let Some(preview) = &mut opened.preview {
        let plan = plan_for(engine, &opened.project, &timeline, &mut preview.sources)?;
        if let Some(player) = engine.preview(preview.session) {
            player.set_plan(plan);
        }
    }
    opened.timeline = Some(timeline);
    Ok(())
}

/// The preview plan of `timeline`. A source that is offline is left out and
/// plays as a gap; the banner already says so (#32).
fn plan_for(
    engine: &MediaEngine,
    project: &Project,
    timeline: &Timeline,
    sources: &mut BTreeMap<SourceId, Arc<SourceMedia>>,
) -> Result<PlaybackPlan, String> {
    sources.retain(|id, media| {
        project
            .sources
            .get(id)
            .is_some_and(|source| source.path() == media.path)
    });
    for (&id, source) in &project.sources {
        if sources.contains_key(&id) || !source.check().is_present() {
            continue;
        }
        sources.insert(id, Arc::new(engine.source_media(source.path())?));
    }
    PlaybackPlan::from_timeline(timeline, sources).map_err(|error| error.to_string())
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
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    source: SourceId,
    path: PathBuf,
) -> Result<ProjectView, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    // Opened read-only to fingerprint, and never written (`CLAUDE.md`
    // section 19). What is saved is the project file.
    opened
        .project
        .relink(source, &MediaAsset::new(path).export_source())
        .map_err(|error| error.to_string())?;
    opened
        .project
        .save(&opened.path)
        .map_err(|error| error.to_string())?;
    // The relinked source is playable now: the preview picks it up.
    refresh(&engine, opened)?;
    Ok(ProjectView::of(&opened.path, &opened.project))
}

/// Preview the open project through the shared evaluator.
///
/// # Errors
///
/// No project is open, its graph cannot be evaluated (the message says
/// why), or nothing in it can be played.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn open_project_preview(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    max_width: u32,
    max_height: u32,
) -> Result<PreviewOpened, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    let timeline = evaluate(&opened.project).map_err(|error| error.to_string())?;
    let mut sources = BTreeMap::new();
    let plan = plan_for(&engine, &opened.project, &timeline, &mut sources)?;
    let preview = engine.start_preview(app, plan, max_width, max_height)?;
    opened.timeline = Some(timeline);
    opened.preview = Some(ProjectPreview {
        session: preview.session,
        sources,
    });
    Ok(preview)
}

/// Replace the open project's graph — the timeline's edits arrive here — and
/// update its preview in place. Nothing is written: saving is #54, and a
/// graph change is decisions, not files (`CLAUDE.md` section 20 rule 1).
///
/// # Errors
///
/// No project is open, or the new graph cannot be evaluated; the open graph
/// is then unchanged.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn update_project(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    project: Project,
) -> Result<ProjectView, String> {
    evaluate(&project).map_err(|error| error.to_string())?;
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened.project = project;
    refresh(&engine, opened)?;
    Ok(ProjectView::of(&opened.path, &opened.project))
}

/// One operation in the diagnostic view, and whether the preview renders it
/// yet. What the export applies does not depend on the answer.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagnosticOperation {
    pub operation: Operation,
    pub previewed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiagnosticClip {
    pub track: u32,
    pub clip: ClipId,
    pub source: SourceId,
    pub source_name: String,
    #[ts(type = "number")]
    pub source_tick: i64,
    /// What the evaluator applies, in order.
    pub applied: Vec<DiagnosticOperation>,
}

/// What the evaluator applies under the playhead: the diagnostic view.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Diagnostics {
    /// Sequence frames.
    #[ts(type = "number")]
    pub position: i64,
    pub clips: Vec<DiagnosticClip>,
}

impl Diagnostics {
    fn of(at: &OperationsAt, project: &Project) -> Self {
        let clips = at
            .clips
            .iter()
            .map(|clip| DiagnosticClip {
                track: clip.track,
                clip: clip.clip,
                source: clip.source,
                source_name: project
                    .sources
                    .get(&clip.source)
                    .and_then(|source| source.path().file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                source_tick: clip.source_tick,
                applied: clip
                    .operations
                    .iter()
                    .map(|operation| DiagnosticOperation {
                        operation: *operation,
                        previewed: previewed(operation),
                    })
                    .collect(),
            })
            .collect();
        Self {
            position: at.position,
            clips,
        }
    }
}

/// Whether the preview renders `operation`: timing always; of the audio
/// chain, what `chain_rendered` says.
fn previewed(operation: &Operation) -> bool {
    audio_operation(operation).is_none_or(|step| chain_rendered(&step))
}

/// What applies under the playhead of the project's preview `session`.
///
/// # Errors
///
/// No such session, or it is not the open project's.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn operations_at(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    session: u32,
) -> Result<Diagnostics, String> {
    let guard = state.lock()?;
    let opened = guard.as_ref().ok_or("no project is open")?;
    if opened.preview.as_ref().map(|p| p.session) != Some(session) {
        return Err(format!("session {session} is not the project's preview"));
    }
    let timeline = opened
        .timeline
        .as_ref()
        .ok_or("the project has no timeline")?;
    let player = engine
        .preview(session)
        .ok_or_else(|| format!("no preview session {session}"))?;
    let (_, t) = player.position();
    let position = time::rescale(t, MICROSECONDS, timeline.time_base, Rounding::Down)
        .ok_or("the playhead is out of range")?;
    Ok(Diagnostics::of(&timeline.at(position), &opened.project))
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
