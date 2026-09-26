//! Exports in the background (#51): the shell's side of the engine's export
//! queue.
//!
//! The queue, its order, its progress and its crash recovery live in
//! [`blinkify_engine::export::queue`] and are tested there. What is here is
//! only what needs the application: the open project to snapshot, the
//! engine state an export is made with, the file the queue is kept in, the
//! event the renderer follows, and the taskbar flash when an export ends
//! while the window is in the background.

use std::path::PathBuf;
use std::sync::Arc;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{Container, ExportRequest, export};
use blinkify_engine::export::loudness::Resolver;
use blinkify_engine::export::plan::{PlanError, plan};
use blinkify_engine::export::queue::{
    ExportJob, ExportQueue, ExportSpec, Report, RunOutcome, Stage,
};
use blinkify_engine::orchestrator::{CancelToken, JobProgress};
use blinkify_engine::project::evaluate::evaluate;
use tauri::{AppHandle, Emitter, Manager, State, UserAttentionType};

use crate::loudness::{facts, inputs};
use crate::media::MediaEngine;
use crate::project::OpenProject;

/// Every change of every export job: an [`ExportJob`].
pub const JOB_EVENT: &str = "export://job";

/// The file the queue and its history are kept in, in the app data folder.
const STORE: &str = "exports.json";

/// Open the queue kept in the app data folder and start exporting.
pub fn open_queue(app: &AppHandle) -> ExportQueue {
    let store = app.path().app_data_dir().ok().map(|dir| dir.join(STORE));
    let runner = {
        let app = app.clone();
        move |spec: &ExportSpec, cancel: &CancelToken, report: Report| {
            run(&app, spec, cancel, &report)
        }
    };
    let observer = {
        let app = app.clone();
        move |job: &ExportJob| announce(&app, job)
    };
    ExportQueue::open(store, runner, observer)
}

/// Tell the renderer; and when an export ends while the window is not the
/// one the user is looking at, flash it in the taskbar, which is how Windows
/// says "come and look" without a notification service.
fn announce(app: &AppHandle, job: &ExportJob) {
    // A failed emit — the window is gone — changes nothing about the export.
    let _ = app.emit(JOB_EVENT, job);
    if job.state.is_finished()
        && let Some(window) = app.get_webview_window("main")
        && !window.is_focused().unwrap_or(false)
    {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

/// Make the export `spec` asks for: resolve its loudness, plan it, and
/// execute the plan — all from the project as it was when it was asked for.
fn run(
    app: &AppHandle,
    spec: &ExportSpec,
    cancel: &CancelToken,
    report: &Report,
) -> Result<RunOutcome, String> {
    let engine = app.state::<MediaEngine>();
    let orchestrator = engine.orchestrator()?;
    let project = &spec.project;
    report(Stage::Preparing, 0.0);
    let timeline = evaluate(project).map_err(|error| error.to_string())?;
    let facts = facts(&engine, project)?;
    let inputs = inputs(&engine, project)?;
    // A normalisation is measured before anything is planned (#48): the
    // export refuses one that was not.
    let timeline = Resolver {
        orchestrator,
        cache: engine.cache(),
        inputs: &inputs,
        models: engine.models(),
        cancel,
        only_cached: false,
    }
    .resolve(&timeline, &project.sequence.settings, &facts)
    .map_err(|error| error.to_string())?;
    let plan =
        plan(&timeline, &project.sequence.settings, &facts).map_err(|error| match error {
            PlanError::UnknownSource(id) => project.sources.get(&id).map_or_else(
                || error.to_string(),
                |source| format!("{} is missing", source.path().display()),
            ),
            _ => error.to_string(),
        })?;
    report(Stage::Exporting, 0.0);
    let forward = Arc::clone(report);
    let outcome = export(
        orchestrator,
        ExportRequest {
            plan: &plan,
            inputs: &inputs,
            target: &spec.target,
            overwrite: spec.overwrite,
            audio: spec.audio,
            models: engine.models(),
            cancel: cancel.clone(),
            on_progress: Some(Box::new(move |update: JobProgress| {
                forward(Stage::Exporting, update.progress.fraction);
            })),
        },
    )
    .map_err(|error| error.to_string())?;
    let bytes = std::fs::metadata(&outcome.path)
        .map_err(|error| error.to_string())?
        .len();
    Ok(RunOutcome { bytes })
}

/// Queue an export of the open project, as it is now, to `target`.
///
/// # Errors
///
/// No project is open, the target is not a container Blinkify writes, or it
/// is one of the project's sources.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn submit_export(
    queue: State<'_, ExportQueue>,
    state: State<'_, OpenProject>,
    target: PathBuf,
    overwrite: bool,
    audio: AudioTarget,
) -> Result<ExportJob, String> {
    Container::of(&target).map_err(|error| error.to_string())?;
    let project = {
        let guard = state.lock()?;
        let opened = guard.as_ref().ok_or("no project is open")?;
        opened.session.document().project().clone()
    };
    queue
        .submit(ExportSpec {
            name: project.name.clone(),
            project,
            target,
            overwrite,
            audio,
        })
        .map_err(|error| error.to_string())
}

/// Every export: the queue in order, then the history.
#[tauri::command]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn export_jobs(queue: State<'_, ExportQueue>) -> Vec<ExportJob> {
    queue.jobs()
}

/// Cancel export `id`: stop it, every process of it, and remove its partial
/// file.
///
/// # Errors
///
/// There is no such export.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn cancel_export(queue: State<'_, ExportQueue>, id: u64) -> Result<(), String> {
    queue.cancel(id).map_err(|error| error.to_string())
}

/// Export interrupted export `id` again, from the start.
///
/// # Errors
///
/// There is no such export, or it was not interrupted.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn resume_export(queue: State<'_, ExportQueue>, id: u64) -> Result<ExportJob, String> {
    queue.resume(id).map_err(|error| error.to_string())
}

/// Give up interrupted export `id`.
///
/// # Errors
///
/// There is no such export, or it was not interrupted.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn discard_export(queue: State<'_, ExportQueue>, id: u64) -> Result<ExportJob, String> {
    queue.discard(id).map_err(|error| error.to_string())
}

/// Forget every finished export.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn clear_export_history(queue: State<'_, ExportQueue>) {
    queue.clear_history();
}
