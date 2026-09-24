//! The media library (#53): importing files into the open project.
//!
//! **Import copies nothing.** Each file is probed where it is, fingerprinted
//! (#32), and added to the project as a reference — never copied into a
//! project folder, which would be an intermediate file (`CLAUDE.md` section
//! 20 rule 1). A file Blinkify cannot use — unreadable, empty, no pictures
//! or sound — is refused with the reason, not added as a broken entry.
//!
//! A hundred files are probed one after another off the window's thread,
//! with an event per file so the grid fills in as they arrive; together they
//! are one undoable step.

use std::path::PathBuf;

use blinkify_engine::project::SourceId;
use blinkify_engine::project::edit::EditContext;
use blinkify_engine::proxy::MediaAsset;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use ts_rs::TS;

use crate::media::{MediaEngine, SourceFacts};
use crate::project::{OpenProject, ProjectView, describe};

/// Import progress, one event per file, with an [`ImportProgress`] payload.
pub const IMPORT_EVENT: &str = "library://import";

/// One file of an import, done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportProgress {
    #[ts(type = "string")]
    pub path: PathBuf,
    /// How many files are done, this one included.
    pub done: u32,
    pub total: u32,
    /// Why this file was refused, if it was.
    pub refused: Option<String>,
}

/// A file an import refused, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Refusal {
    #[ts(type = "string")]
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportOutcome {
    pub view: ProjectView,
    /// The source of each file taken, in order — a file already in the
    /// project keeps its source.
    pub imported: Vec<SourceId>,
    pub refused: Vec<Refusal>,
}

/// Import `paths` into the open project.
///
/// # Errors
///
/// No project is open. A file that cannot be used is not an error: it is
/// listed in `refused`, with the reason.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn import_media(
    app: AppHandle,
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    paths: Vec<PathBuf>,
    context: EditContext,
) -> Result<ImportOutcome, String> {
    let total = u32::try_from(paths.len()).unwrap_or(u32::MAX);
    let mut taken: Vec<(blinkify_engine::project::SourceRef, SourceFacts)> = Vec::new();
    let mut refused = Vec::new();
    // Probed before the project is locked: probing takes a while, and edits
    // should not wait for it.
    for (done, path) in (1..=total).zip(&paths) {
        let outcome = engine.import_facts(path).and_then(|facts| {
            // Opened read-only to fingerprint; never written (section 19).
            let reference = blinkify_engine::project::SourceRef::of(
                &MediaAsset::new(path.clone()).export_source(),
            )
            .map_err(|error| format!("it could not be read: {error}"))?;
            Ok((reference, facts))
        });
        let reason = match outcome {
            Ok(entry) => {
                taken.push(entry);
                None
            }
            Err(reason) => {
                refused.push(Refusal {
                    path: path.clone(),
                    reason: reason.clone(),
                });
                Some(reason)
            }
        };
        let _ = app.emit(
            IMPORT_EVENT,
            ImportProgress {
                path: path.clone(),
                done,
                total,
                refused: reason,
            },
        );
    }

    let mut guard = state.lock()?;
    let opened = guard.as_mut().ok_or("no project is open")?;
    let references: Vec<_> = taken
        .iter()
        .map(|(reference, _)| reference.clone())
        .collect();
    let imported = opened
        .session
        .document_mut()
        .add_sources(&references, &context);
    for ((_, facts), &id) in taken.into_iter().zip(&imported) {
        describe(opened.session.document_mut(), id, facts);
    }
    let view = opened.view();
    crate::lifecycle::title(&app, &view);
    Ok(ImportOutcome {
        view,
        imported,
        refused,
    })
}
