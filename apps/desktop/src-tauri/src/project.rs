//! The open project (#32): the one the application was launched with, and
//! relinking its sources.
//!
//! Double-clicking a `.blinkify` file in Explorer starts Blinkify with the
//! file's path as its first argument (the association is registered by the
//! installer, #14). [`LaunchFile`] keeps that argument; the renderer asks for
//! the project with [`launch_project`] once it is ready to show it.
//!
//! The project is previewed through the shared evaluator (#30):
//! [`open_project_preview`] plays it, [`edit_project`] applies an edit (#37)
//! and the preview follows without restarting what did not change,
//! [`undo_edit`] and [`redo_edit`] walk the history, and
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
use blinkify_engine::project::edit::{Document, Edit, EditContext, HistoryView, SettingsImpact};
use blinkify_engine::project::evaluate::{OperationsAt, Timeline, audio_operation, evaluate};
use blinkify_engine::project::trim::StreamExtent;
use blinkify_engine::project::{
    self, ClipId, CopyEligibility, Operation, Project, SequenceSettings, SourceId, SourceStatus,
};
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
    /// The graph and its history. Every change to it is an edit (#37).
    document: Document,
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

/// A project as the renderer shows it: the graph, the graph evaluated, its
/// history, and what opening it found about its sources.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProjectView {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub project: Project,
    /// The graph as the evaluator (#30) resolves it — what the timeline
    /// draws, so the renderer never interprets an operation itself. `None`
    /// when the graph does not evaluate.
    pub timeline: Option<Timeline>,
    pub history: HistoryView,
    /// Every source that is not present, and why.
    pub unavailable: BTreeMap<SourceId, SourceStatus>,
    /// The clips that play an unavailable source — the ones to mark.
    pub affected_clips: Vec<ClipId>,
    /// Whether each source's pictures can be stream-copied into the sequence
    /// (#57): the model's answer, which the timeline marks and nothing in the
    /// renderer works out again. A source not listed has no pictures, or is
    /// offline.
    pub eligibility: BTreeMap<SourceId, CopyEligibility>,
    /// Which ticks of each source stream exist (#34): what the timeline
    /// bounds a trim preview by. The engine bounds the trim itself.
    pub extents: Vec<StreamExtent>,
}

/// What an edit, an undo or a redo returns: the project now, and the
/// selection and playhead to show with it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditOutcome {
    pub view: ProjectView,
    pub context: EditContext,
    /// A bound stopped the edit short of what was asked — the end of the
    /// source, a neighbouring clip — and the timeline says so.
    pub clamped: bool,
}

impl ProjectView {
    fn of(path: &Path, document: &Document, timeline: Option<&Timeline>) -> Self {
        let project = document.project();
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
            timeline: timeline.cloned(),
            history: document.history(),
            unavailable,
            affected_clips,
            eligibility: document.eligibility(),
            extents: document.extents(),
        }
    }
}

impl Opened {
    fn view(&self) -> ProjectView {
        ProjectView::of(&self.path, &self.document, self.timeline.as_ref())
    }
}

/// Probe each source that is present for its pictures, so the document can
/// decide copy eligibility and the first-clip default (#57).
fn describe_sources(engine: &MediaEngine, document: &mut Document) {
    let present: Vec<(SourceId, PathBuf)> = document
        .project()
        .sources
        .iter()
        .filter(|(_, source)| source.check().is_present())
        .map(|(&id, source)| (id, source.path().to_path_buf()))
        .collect();
    for (id, path) in present {
        let (geometry, extents) = engine.facts_of(&path);
        document.describe_source(id, geometry);
        document.describe_streams(id, &extents);
    }
}

fn open(engine: &MediaEngine, state: &OpenProject, path: &Path) -> Result<ProjectView, String> {
    let project = Project::load(path).map_err(|error| error.to_string())?;
    let mut document = Document::new(project).map_err(|error| error.to_string())?;
    describe_sources(engine, &mut document);
    let opened = Opened {
        path: path.to_path_buf(),
        timeline: evaluate(document.project()).ok(),
        document,
        preview: None,
    };
    let view = opened.view();
    *state.lock()? = Some(opened);
    Ok(view)
}

/// Evaluate `opened`'s graph and, if it is previewed, give the player the new
/// plan: sources it has not opened yet are opened, the rest are reused.
fn refresh(engine: &MediaEngine, opened: &mut Opened) -> Result<(), String> {
    let project = opened.document.project();
    let timeline = evaluate(project).map_err(|error| error.to_string())?;
    if let Some(preview) = &mut opened.preview {
        let plan = plan_for(engine, project, &timeline, &mut preview.sources)?;
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
    engine: State<'_, MediaEngine>,
    launch: State<'_, LaunchFile>,
    state: State<'_, OpenProject>,
) -> Result<Option<ProjectView>, String> {
    let Some(path) = launch.0.as_deref() else {
        return Ok(None);
    };
    let view = open(&engine, &state, path)?;
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
    context: EditContext,
) -> Result<ProjectView, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    // Opened read-only to fingerprint, and never written (`CLAUDE.md`
    // section 19). What is saved is the project file.
    let relinked = opened
        .document
        .project()
        .sources
        .get(&source)
        .ok_or_else(|| format!("no source {source}"))?
        .relink(&MediaAsset::new(path).export_source())
        .map_err(|error| error.to_string())?;
    opened
        .document
        .relink(source, relinked, &context)
        .map_err(|error| error.to_string())?;
    let (geometry, extents) = opened
        .document
        .project()
        .sources
        .get(&source)
        .map(|reference| engine.facts_of(reference.path()))
        .unwrap_or_default();
    opened.document.describe_source(source, geometry);
    opened.document.describe_streams(source, &extents);
    opened
        .document
        .project()
        .save(&opened.path)
        .map_err(|error| error.to_string())?;
    // The relinked source is playable now: the preview picks it up.
    refresh(&engine, opened)?;
    Ok(opened.view())
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
    let project = opened.document.project();
    let timeline = evaluate(project).map_err(|error| error.to_string())?;
    let mut sources = BTreeMap::new();
    let plan = plan_for(&engine, project, &timeline, &mut sources)?;
    let preview = engine.start_preview(app, plan, max_width, max_height)?;
    opened.timeline = Some(timeline);
    opened.preview = Some(ProjectPreview {
        session: preview.session,
        sources,
    });
    Ok(preview)
}

/// Apply one edit to the open project (#37) and update its preview in place.
/// Nothing is written: saving is #54, and a graph change is decisions, not
/// files (`CLAUDE.md` section 20 rule 1).
///
/// # Errors
///
/// No project is open, or the engine refused the edit — it names something
/// that is not there, or its result would not evaluate. The graph is then
/// unchanged.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn edit_project(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    edit: Edit,
    context: EditContext,
) -> Result<EditOutcome, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    let applied = opened
        .document
        .apply(&edit, &context)
        .map_err(|error| error.to_string())?;
    Ok(settle(&engine, opened, applied.context, applied.clamped))
}

/// Undo the open project's last edit; `None` when there is nothing to undo.
///
/// # Errors
///
/// No project is open.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn undo_edit(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<Option<EditOutcome>, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    Ok(opened
        .document
        .undo()
        .map(|context| settle(&engine, opened, context, false)))
}

/// Redo the open project's last undone edit; `None` when there is nothing to
/// redo.
///
/// # Errors
///
/// No project is open.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn redo_edit(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<Option<EditOutcome>, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    Ok(opened
        .document
        .redo()
        .map(|context| settle(&engine, opened, context, false)))
}

/// What changing the open project's sequence to `settings` would do to
/// copying (#57): asked by the settings dialog before anything is changed.
///
/// # Errors
///
/// No project is open, or no file can carry those settings — the message
/// says why.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn preview_settings(
    state: State<'_, OpenProject>,
    settings: SequenceSettings,
) -> Result<SettingsImpact, String> {
    let guard = state.lock()?;
    let opened = guard.as_ref().ok_or("no project is open")?;
    opened
        .document
        .settings_impact(&settings)
        .map_err(|error| error.to_string())
}

/// Start a gesture — a slider being dragged — whose edits are one history
/// entry, until [`end_gesture`].
///
/// # Errors
///
/// No project is open.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn begin_gesture(
    state: State<'_, OpenProject>,
    label: String,
    context: EditContext,
) -> Result<(), String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened.document.begin_gesture(&label, &context);
    Ok(())
}

/// End the gesture under way.
///
/// # Errors
///
/// No project is open.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn end_gesture(state: State<'_, OpenProject>) -> Result<ProjectView, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    opened.document.end_gesture();
    Ok(opened.view())
}

/// After the graph changed: evaluate it again, update the preview, report.
fn settle(
    engine: &MediaEngine,
    opened: &mut Opened,
    context: EditContext,
    clamped: bool,
) -> EditOutcome {
    // The edit has happened. A graph that no longer previews — only a
    // project that opened unevaluable can get here — says so with
    // `timeline: null` rather than by failing the edit.
    if refresh(engine, opened).is_err() {
        opened.timeline = None;
    }
    EditOutcome {
        view: opened.view(),
        context,
        clamped,
    }
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
    Ok(Diagnostics::of(
        &timeline.at(position),
        opened.document.project(),
    ))
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
