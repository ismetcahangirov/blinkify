//! Loudness normalisation in the shell (#48): resolving the open project's
//! normalisations for the preview and the export plan, and the two reports
//! the inspector asks for.
//!
//! The preview resolves from the cache only: a normalisation not measured
//! yet is played unprocessed and the diagnostics say so. The reports measure
//! — a whole clip, or the whole mix — and then refresh the preview, which
//! from then on finds every measurement in the cache.

use std::collections::BTreeMap;

use blinkify_engine::audio::normalise::{apply_sequence, fingerprint};
use blinkify_engine::export::execute::ExportInput;
use blinkify_engine::export::loudness::{LoudnessReport, Resolver};
use blinkify_engine::export::plan::SourceFacts;
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::project::evaluate::{Timeline, evaluate};
use blinkify_engine::project::{ClipId, Project, SourceId};
use blinkify_engine::proxy::MediaAsset;
use tauri::State;

use crate::media::MediaEngine;
use crate::project::{OpenProject, refresh};

/// Every present source of `project`, as an export reads it.
pub(crate) fn inputs(
    engine: &MediaEngine,
    project: &Project,
) -> Result<BTreeMap<SourceId, ExportInput>, String> {
    let mut inputs = BTreeMap::new();
    for (&id, source) in &project.sources {
        if source.check().is_present() {
            inputs.insert(
                id,
                ExportInput {
                    source: MediaAsset::new(source.path().to_path_buf()).export_source(),
                    info: engine.info_of(source.path())?,
                },
            );
        }
    }
    Ok(inputs)
}

/// What the planner needs of every present source.
pub(crate) fn facts(
    engine: &MediaEngine,
    project: &Project,
) -> Result<BTreeMap<SourceId, SourceFacts>, String> {
    let mut facts = BTreeMap::new();
    for (&id, source) in &project.sources {
        if source.check().is_present() {
            facts.insert(id, engine.export_facts(source.path())?);
        }
    }
    Ok(facts)
}

fn resolver<'a>(
    engine: &'a MediaEngine,
    inputs: &'a BTreeMap<SourceId, ExportInput>,
    cancel: &'a CancelToken,
    only_cached: bool,
) -> Result<Resolver<'a>, String> {
    Ok(Resolver {
        orchestrator: engine.orchestrator()?,
        cache: engine.cache(),
        inputs,
        models: engine.models(),
        cancel,
        only_cached,
    })
}

/// `timeline` with every normalisation resolved from what is already
/// measured — each clip's from the cache, the sequence's from the last mix
/// measured for exactly these clips — and the rest left unresolved. Cheap:
/// nothing is decoded.
pub(crate) fn resolved(
    engine: &MediaEngine,
    project: &Project,
    timeline: &Timeline,
) -> Result<Timeline, String> {
    if !timeline
        .placements()
        .any(blinkify_engine::audio::normalise::needs_measuring)
        && timeline.loudness.is_none()
    {
        return Ok(timeline.clone());
    }
    let inputs = inputs(engine, project)?;
    let cancel = CancelToken::default();
    let mut clips = resolver(engine, &inputs, &cancel, true)?
        .clips(timeline)
        .map_err(|error| error.to_string())?;
    let gain = engine.sequence_gain(&fingerprint(&clips));
    apply_sequence(&mut clips, gain);
    Ok(clips)
}

/// Clip `clip`'s normalisation: how loud it is before, the one gain, and
/// how loud it comes out — measured, the first time, over the whole clip.
/// `None` for a clip without a normalisation or without sound.
///
/// # Errors
///
/// No project is open, no such clip, or a measurement failed.
#[tauri::command(async)]
// Tauri injects managed state and arguments by value; see
// `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn loudness_report(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
    clip: ClipId,
) -> Result<Option<LoudnessReport>, String> {
    let project = current(&state)?;
    let timeline = evaluate(&project).map_err(|error| error.to_string())?;
    let placement = timeline
        .placements()
        .find(|placement| placement.clip == clip)
        .ok_or_else(|| format!("no clip {clip}"))?
        .clone();
    let inputs = inputs(&engine, &project)?;
    let cancel = CancelToken::default();
    let report = resolver(&engine, &inputs, &cancel, false)?
        .clip_report(&placement)
        .map_err(|error| error.to_string())?;
    refreshed(&engine, &state)?;
    Ok(report)
}

/// The whole sequence's normalisation, measured on the mix. `None` when the
/// sequence is not normalised.
///
/// # Errors
///
/// No project is open, or a render or measurement failed.
#[tauri::command(async)]
// Tauri injects managed state by value; see `updater::pending_update`.
#[allow(clippy::needless_pass_by_value)]
pub fn sequence_loudness_report(
    engine: State<'_, MediaEngine>,
    state: State<'_, OpenProject>,
) -> Result<Option<LoudnessReport>, String> {
    let project = current(&state)?;
    let timeline = evaluate(&project).map_err(|error| error.to_string())?;
    let inputs = inputs(&engine, &project)?;
    let facts = facts(&engine, &project)?;
    let cancel = CancelToken::default();
    let resolver = resolver(&engine, &inputs, &cancel, false)?;
    let clips = resolver
        .clips(&timeline)
        .map_err(|error| error.to_string())?;
    let report = resolver
        .sequence(&clips, &project.sequence.settings, &facts)
        .map_err(|error| error.to_string())?;
    if let Some(report) = &report {
        engine.remember_sequence_gain(fingerprint(&clips), report.gain_db);
    }
    refreshed(&engine, &state)?;
    Ok(report)
}

/// The open project, copied out so nothing is decoded under the lock.
fn current(state: &OpenProject) -> Result<Project, String> {
    let guard = state.lock()?;
    let opened = guard.as_ref().ok_or("no project is open")?;
    Ok(opened.session.document().project().clone())
}

/// Give the preview the plan with what was just measured.
fn refreshed(engine: &MediaEngine, state: &OpenProject) -> Result<(), String> {
    let mut guard = state.lock()?;
    if let Some(opened) = guard.as_mut() {
        refresh(engine, opened)?;
    }
    Ok(())
}
