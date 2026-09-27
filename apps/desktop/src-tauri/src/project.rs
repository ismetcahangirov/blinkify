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

use blinkify_engine::audio::gain::{GainAdvice, advise, measure_request};
use blinkify_engine::export::plan::{ExportPlan, plan};
use blinkify_engine::playback::{PlaybackPlan, SourceMedia, chain_rendered};
use blinkify_engine::project::asset::AssetInfo;
use blinkify_engine::project::edit::{Document, Edit, EditContext, HistoryView, SettingsImpact};
use blinkify_engine::project::evaluate::{OperationsAt, Timeline, audio_operation, evaluate};
use blinkify_engine::project::session::Session;
use blinkify_engine::project::speed::SpeedVerdict;
use blinkify_engine::project::split::{CutPoint, cut_point};
use blinkify_engine::project::trim::StreamExtent;
use blinkify_engine::project::{
    self, AudioStage, ClipId, CopyEligibility, Operation, Project, SequenceSettings, SourceId,
    SourceStatus,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::time::{self, MICROSECONDS, Rounding};
use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

use crate::media::{MediaEngine, PreviewOpened, SourceFacts};

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
pub(crate) struct Opened {
    /// The project, its history, where it lives and what was saved (#37,
    /// #54). Every change to the graph is an edit on its document.
    pub(crate) session: Session,
    /// The graph as the evaluator last resolved it.
    pub(crate) timeline: Option<Timeline>,
    pub(crate) preview: Option<ProjectPreview>,
}

/// A preview session playing the project, and the sources it has opened —
/// kept, so a graph change does not probe or index a file again.
#[derive(Debug)]
pub(crate) struct ProjectPreview {
    pub(crate) session: u32,
    sources: BTreeMap<SourceId, Arc<SourceMedia>>,
}

impl OpenProject {
    pub(crate) fn lock(&self) -> Result<MutexGuard<'_, Option<Opened>>, String> {
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
    /// Where the project file is; `null` for a project never saved (#54).
    #[ts(type = "string | null")]
    pub path: Option<PathBuf>,
    /// It differs from what was last saved or opened (#54).
    pub dirty: bool,
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
    /// What the library shows of each source, from its probe (#53).
    pub assets: BTreeMap<SourceId, AssetInfo>,
    /// What each video clip's speed does to its pictures at export (#56):
    /// the model's answer, which the inspector states and never works out.
    pub speeds: BTreeMap<ClipId, SpeedVerdict>,
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
    fn of(session: &Session, timeline: Option<&Timeline>) -> Self {
        let document = session.document();
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
            path: session.path().map(Path::to_path_buf),
            dirty: session.is_dirty(),
            project: project.clone(),
            timeline: timeline.cloned(),
            history: document.history(),
            unavailable,
            affected_clips,
            eligibility: document.eligibility(),
            extents: document.extents(),
            assets: document.assets(),
            speeds: timeline
                .map(|timeline| document.speed_verdicts(timeline))
                .unwrap_or_default(),
        }
    }
}

impl Opened {
    /// A session opened: its sources probed, its graph evaluated.
    pub(crate) fn new(engine: &MediaEngine, mut session: Session) -> Self {
        describe_sources(engine, session.document_mut());
        Self {
            timeline: evaluate(session.document().project()).ok(),
            session,
            preview: None,
        }
    }

    pub(crate) fn view(&self) -> ProjectView {
        ProjectView::of(&self.session, self.timeline.as_ref())
    }
}

/// Tell the document what a probe found about `source`.
pub(crate) fn describe(document: &mut Document, source: SourceId, facts: SourceFacts) {
    document.describe_source(source, facts.geometry);
    document.describe_streams(source, &facts.extents);
    document.describe_asset(source, facts.asset);
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
        describe(document, id, engine.facts_of(&path));
    }
}

/// Evaluate `opened`'s graph and, if it is previewed, give the player the new
/// plan: sources it has not opened yet are opened, the rest are reused.
pub(crate) fn refresh(engine: &MediaEngine, opened: &mut Opened) -> Result<(), String> {
    let project = opened.session.document().project();
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
    // Normalisation as measured so far (#48): what is not measured yet is
    // played unprocessed, never guessed.
    let timeline = crate::loudness::resolved(engine, project, timeline)?;
    PlaybackPlan::from_timeline(&timeline, sources).map_err(|error| error.to_string())
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
    // A double-click in Explorer takes the same path as File > Open (#54).
    crate::lifecycle::open_project(app, engine, state, path.to_path_buf()).map(Some)
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
        .session
        .document()
        .project()
        .sources
        .get(&source)
        .ok_or_else(|| format!("no source {source}"))?
        .relink(&MediaAsset::new(path).export_source())
        .map_err(|error| error.to_string())?;
    opened
        .session
        .document_mut()
        .relink(source, relinked, &context)
        .map_err(|error| error.to_string())?;
    let facts = opened
        .session
        .document()
        .project()
        .sources
        .get(&source)
        .map(|reference| engine.facts_of(reference.path()))
        .unwrap_or_default();
    describe(opened.session.document_mut(), source, facts);
    if opened.session.path().is_some() {
        opened.session.save().map_err(|error| error.to_string())?;
    }
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
    let project = opened.session.document().project();
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
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    edit: Edit,
    context: EditContext,
) -> Result<EditOutcome, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    let applied = opened
        .session
        .document_mut()
        .apply(&edit, &context)
        .map_err(|error| error.to_string())?;
    // An edit that is expensive to redo is autosaved at once, not at the
    // next tick (#54). A recovery file that cannot be written does not undo
    // the edit; the interval tries again.
    if significant(&edit) {
        let _ = opened.session.autosave();
    }
    Ok(settle(
        &app,
        &engine,
        opened,
        applied.context,
        applied.clamped,
    ))
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
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<Option<EditOutcome>, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    Ok(opened
        .session
        .document_mut()
        .undo()
        .map(|context| settle(&app, &engine, opened, context, false)))
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
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<Option<EditOutcome>, String> {
    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    Ok(opened
        .session
        .document_mut()
        .redo()
        .map(|context| settle(&app, &engine, opened, context, false)))
}

/// Whether a cut at sequence frame `position` on the main video track is
/// lossless, and where the keyframes around it are (#35): the timeline's
/// keyframe indicator. `None` when no clip plays forwards there. Reads the
/// keyframe index around the position, so it runs off the window's thread.
///
/// # Errors
///
/// No project is open, or the source's index cannot be read.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn cut_point_at(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    position: i64,
) -> Result<Option<CutPoint>, String> {
    // Only what is needed is copied out, so the index is read without the
    // project locked.
    let (placement, path) = {
        let guard = state.lock()?;
        let opened = guard.as_ref().ok_or("no project is open")?;
        let Some(timeline) = opened.timeline.as_ref() else {
            return Ok(None);
        };
        let main = timeline
            .tracks
            .iter()
            .find(|track| track.kind == project::TrackKind::Video);
        let Some(placement) = main
            .and_then(|track| track.placements.iter().find(|p| p.covers(position)))
            .filter(|placement| placement.motion.is_none())
            .cloned()
        else {
            return Ok(None);
        };
        let Some(path) = opened
            .session
            .document()
            .project()
            .sources
            .get(&placement.source)
            .filter(|source| source.check().is_present())
            .map(|source| source.path().to_path_buf())
        else {
            return Ok(None);
        };
        (placement, path)
    };
    let Some(tick) = placement.source_at(position) else {
        return Ok(None);
    };
    let index = engine.keyframe_index(&path)?;
    let stream = placement.stream;
    let shown = index
        .frame_at_or_before(stream, tick)
        .map_err(|error| error.to_string())?
        .unwrap_or(tick);
    let before = index
        .at_or_before(stream, shown)
        .map_err(|error| error.to_string())?;
    let after = index
        .at_or_after(stream, shown.saturating_add(1))
        .map_err(|error| error.to_string())?;
    Ok(Some(cut_point(&placement, position, shown, before, after)))
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
        .session
        .document()
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
    opened
        .session
        .document_mut()
        .begin_gesture(&label, &context);
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
    opened.session.document_mut().end_gesture();
    Ok(opened.view())
}

/// Edits worth an autosave the moment they happen: they change many clips,
/// or would take effort to do again.
fn significant(edit: &Edit) -> bool {
    match edit {
        Edit::RippleDelete { .. }
        | Edit::RemoveTrack { .. }
        | Edit::DetachAudio { .. }
        | Edit::Split { .. }
        | Edit::FreezeFrame { .. }
        | Edit::SetSettings { .. } => true,
        Edit::MoveClips { moves } => moves.len() > 1,
        Edit::RemoveClips { clips } => clips.len() > 1,
        _ => false,
    }
}

/// After the graph changed: evaluate it again, update the preview, report.
fn settle(
    app: &AppHandle,
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
    let view = opened.view();
    // The window says whether there is unsaved work, as the app bar does.
    crate::lifecycle::title(app, &view);
    EditOutcome {
        view,
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
    fn of(at: &OperationsAt, project: &Project, models: bool) -> Self {
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
                        previewed: previewed(operation, models),
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

/// Whether the preview renders `operation`: timing always, a hold and a
/// reverse too, as the export renders them (#113); of the audio chain, what
/// `chain_rendered` says — and noise reduction only while its model is
/// installed (#47).
fn previewed(operation: &Operation, models: bool) -> bool {
    audio_operation(operation)
        .is_none_or(|step| chain_rendered(&step) && (models || step.stage() != AudioStage::Denoise))
}

/// What exporting the open project would do (#39): every segment of the
/// output, copied or re-encoded, with every reason. Pure and cheap once the
/// sources are indexed, so the export dialog (#50) asks again after every
/// edit. A source that is offline is left out, and the plan fails naming
/// it: nothing can be exported from a file that is not there.
///
/// # Errors
///
/// No project is open, or its timeline cannot be planned.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn plan_export(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<ExportPlan, String> {
    // Copied out, so no probe or index read happens under the lock.
    let (project, timeline) = {
        let guard = state.lock()?;
        let opened = guard.as_ref().ok_or("no project is open")?;
        let project = opened.session.document().project().clone();
        let timeline = match &opened.timeline {
            Some(timeline) => timeline.clone(),
            None => evaluate(&project).map_err(|error| error.to_string())?,
        };
        (project, timeline)
    };
    let mut facts = BTreeMap::new();
    for (&id, source) in &project.sources {
        if source.check().is_present() {
            facts.insert(id, engine.export_facts(source.path())?);
        }
    }
    // A normalised sequence processes every clip's sound (#48): the plan
    // must know, measured or not.
    let timeline = crate::loudness::resolved(&engine, &project, &timeline)?;
    plan(&timeline, &project.sequence.settings, &facts).map_err(|error| error.to_string())
}

/// What clip `clip`'s gain does (#46): how loud it is before the gain, how
/// far the limiter turns its loudest peak down at the gain it has, and a
/// gain to suggest. `None` for a clip with no sound. The first call for a
/// stretch decodes it; later ones are answered from the cache.
///
/// # Errors
///
/// No project is open, no such clip, or its source cannot be read.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn gain_advice(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    clip: ClipId,
) -> Result<Option<GainAdvice>, String> {
    // Copied out, so the decode happens outside the lock.
    let (placement, path) = {
        let guard = state.lock()?;
        let opened = guard.as_ref().ok_or("no project is open")?;
        let project = opened.session.document().project();
        let timeline = match &opened.timeline {
            Some(timeline) => timeline.clone(),
            None => evaluate(project).map_err(|error| error.to_string())?,
        };
        let placement = timeline
            .placements()
            .find(|placement| placement.clip == clip)
            .cloned()
            .ok_or_else(|| format!("no clip {clip}"))?;
        let path = project
            .sources
            .get(&placement.source)
            .ok_or_else(|| format!("no source {}", placement.source))?
            .path()
            .to_path_buf();
        (placement, path)
    };
    let info = engine.info_of(&path)?;
    // Measured after noise reduction, as the gain hears it.
    let Some(request) =
        measure_request(&placement, &path, &info, AudioStage::Gain, engine.models())
            .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let before = engine.loudness(&request)?;
    Ok(Some(advise(before, &placement.audio)))
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
        opened.session.document().project(),
        engine.models().is_some(),
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

    /// An operation as the project file writes it: the shell never takes
    /// the graph apart itself (`pnpm evaluator:check`).
    #[allow(clippy::expect_used)]
    fn operation(json: &str) -> Operation {
        serde_json::from_str(json).expect("an operation as the project file writes it")
    }

    #[test]
    fn a_hold_and_a_reverse_are_previewed_and_a_missing_model_is_not() {
        let hold = operation(r#"{"op":"freeze","frames":90}"#);
        let reverse = operation(r#"{"op":"reverse"}"#);
        let denoise = operation(r#"{"op":"denoise","strength":0.5,"bypassed":false}"#);
        assert!(previewed(&hold, false));
        assert!(previewed(&reverse, false));
        assert!(!previewed(&denoise, false));
        assert!(previewed(&denoise, true));
    }
}
