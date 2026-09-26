//! The first pass of loudness normalisation, for an export (#48): measure
//! what is not measured yet, and resolve every normalisation to its gain.
//!
//! - **A clip** is measured over its whole range, through its chain up to
//!   the normalisation — never piece by piece as the plan cuts it, or each
//!   piece would be brought to the target on its own.
//! - **The sequence** is measured as a mix: the plan of the timeline with
//!   every clip's own chain resolved, each audio segment rendered exactly as
//!   the export would make it, one after another into one meter. The gain it
//!   gives goes on every clip alike, so the levels between them are kept.
//!
//! Clip measurements go through the content-keyed cache; see
//! `audio::loudness::measure_cached`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use super::audio::render_job;
use super::execute::{ExportError, ExportInput};
use super::plan::{ExportPlan, Media, SourceFacts, plan};
use serde::Serialize;
use ts_rs::TS;

use crate::audio::chain::{ChainError, Stage, filters};
use crate::audio::denoise::Models;
use crate::audio::gain::measure_request;
use crate::audio::loudness::{Loudness, LoudnessMeter, MeasureError, cached, measure_cached};
use crate::audio::normalise::{apply_sequence, converge, resolve_clips};
use crate::cache::Cache;
use crate::orchestrator::{CancelToken, Flow, JobError, JobOptions, Orchestrator, Priority};
use crate::project::evaluate::{AudioOperation, Placement, Timeline};
use crate::project::{SequenceSettings, SourceId};

/// What an export's loudness is resolved with.
#[derive(Debug, Clone, Copy)]
pub struct Resolver<'a> {
    pub orchestrator: &'a Orchestrator,
    pub cache: Option<&'a Cache>,
    pub inputs: &'a BTreeMap<SourceId, ExportInput>,
    pub models: Option<&'a Models>,
    pub cancel: &'a CancelToken,
    /// Answer only from the cache and measure nothing: the preview's way,
    /// which leaves a normalisation not measured yet unresolved — unheard,
    /// and said so — rather than decode a whole clip before playing.
    pub only_cached: bool,
}

/// The whole sequence's loudness, before and after its normalisation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LoudnessReport {
    /// Where normalisation starts from: the clip after its own chain up to
    /// the normalisation, or the whole mix.
    pub before: Loudness,
    /// What comes out, measured.
    pub after: Loudness,
    pub target_lufs: f64,
    pub ceiling_dbtp: f64,
    /// The one gain the normalisation applies.
    pub gain_db: f64,
}

impl Resolver<'_> {
    /// How loud `placement` is where its normalisation runs: measured, or
    /// from the cache.
    ///
    /// # Errors
    ///
    /// The source is missing, its chain cannot be built, or the measurement
    /// failed.
    pub fn clip(&self, placement: &Placement) -> Result<Option<Loudness>, ExportError> {
        self.measure_at(placement, None)
    }

    /// How loud `placement` comes out of its whole chain, with its
    /// normalisation at `gain_db`.
    ///
    /// # Errors
    ///
    /// As [`Resolver::clip`].
    pub fn clip_after(
        &self,
        placement: &Placement,
        gain_db: f64,
    ) -> Result<Option<Loudness>, ExportError> {
        self.measure_at(placement, Some(gain_db))
    }

    fn measure_at(
        &self,
        placement: &Placement,
        normalised: Option<f64>,
    ) -> Result<Option<Loudness>, ExportError> {
        let input = self
            .inputs
            .get(&placement.source)
            .ok_or(ExportError::MissingSource(placement.source))?;
        let Some(mut request) = measure_request(
            placement,
            input.source.path(),
            &input.info,
            Stage::Normalise,
            self.models,
        )?
        else {
            return Ok(None);
        };
        if let Some(gain) = normalised {
            let mut after = placement.clone();
            for step in &mut after.audio {
                if let AudioOperation::Normalise {
                    bypassed: false,
                    gain_db,
                    ..
                } = step
                    && gain_db.is_none()
                {
                    *gain_db = Some(gain);
                }
            }
            request.filters = filters(&after.audio, request.sample_rate, self.models)?;
        }
        if self.only_cached {
            return cached(self.cache, &request)
                .map(Some)
                .ok_or(ExportError::AudioChain(ChainError::NotMeasured));
        }
        measure_cached(self.orchestrator, self.cache, &request, self.cancel)
            .map(Some)
            .map_err(measure_error)
    }

    /// The gain `placement`'s own normalisation resolves to.
    ///
    /// # Errors
    ///
    /// As [`Resolver::clip`].
    pub fn clip_gain(
        &self,
        placement: &Placement,
        target_lufs: f64,
        ceiling_dbtp: f64,
    ) -> Result<Option<f64>, ExportError> {
        let Some(first) = self.clip(placement)? else {
            return Ok(None);
        };
        converge(target_lufs, ceiling_dbtp, &first, |gain| {
            self.clip_after(placement, gain)?
                .ok_or_else(|| ExportError::Mismatch("the clip lost its sound".to_owned()))
        })
        .map(Some)
    }

    /// How loud the mix of `plan` is: every audio segment rendered as the
    /// export makes it, measured as one programme.
    ///
    /// # Errors
    ///
    /// A segment cannot be rendered, or a render failed.
    pub fn mix(&self, plan: &ExportPlan) -> Result<Loudness, ExportError> {
        let meter = Arc::new(Mutex::new(LoudnessMeter::new(48_000, 2)));
        let mut segments: Vec<_> = plan
            .segments
            .iter()
            .filter(|segment| segment.media == Media::Audio)
            .collect();
        segments.sort_by_key(|segment| segment.start);
        for segment in segments {
            let command = render_job(segment, self.inputs, plan.time_base, self.models)?;
            let sink = Arc::clone(&meter);
            let mut pending: Vec<u8> = Vec::new();
            let options = JobOptions::default()
                .cancel_token(self.cancel.clone())
                .on_chunk(move |chunk| {
                    pending.extend_from_slice(chunk);
                    let whole = pending.len() - pending.len() % 4;
                    let samples: Vec<f32> = pending
                        .get(..whole)
                        .unwrap_or_default()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|bytes| f32::from_le_bytes(*bytes))
                        .collect();
                    pending.drain(..whole);
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(&samples);
                    Flow::Continue
                });
            self.orchestrator
                .run(command, Priority::Background, options)
                .wait()
                .map_err(|error| match error {
                    JobError::Cancelled | JobError::ShuttingDown => ExportError::Cancelled,
                    other => ExportError::Engine(other),
                })?;
        }
        let loudness = meter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .finish();
        Ok(loudness)
    }

    /// `timeline` with every normalisation resolved — each clip's own, then
    /// the sequence's — measuring whatever is not in the cache. What an
    /// export plans from.
    ///
    /// # Errors
    ///
    /// A measurement failed, or the timeline cannot be planned for one.
    pub fn resolve(
        &self,
        timeline: &Timeline,
        settings: &SequenceSettings,
        facts: &BTreeMap<SourceId, SourceFacts>,
    ) -> Result<Timeline, ExportError> {
        let mut resolved = self.clips(timeline)?;
        let gain = self
            .sequence(&resolved, settings, facts)?
            .map(|report| report.gain_db);
        apply_sequence(&mut resolved, gain);
        Ok(resolved)
    }

    /// `timeline` with each clip's own normalisation resolved; with
    /// `only_cached`, those not in the cache are left unresolved.
    ///
    /// # Errors
    ///
    /// A measurement failed.
    pub fn clips(&self, timeline: &Timeline) -> Result<Timeline, ExportError> {
        let mut resolved = timeline.clone();
        resolve_clips(&mut resolved, |placement, target, ceiling| {
            match self.clip_gain(placement, target, ceiling) {
                Err(ExportError::AudioChain(ChainError::NotMeasured)) if self.only_cached => {
                    Ok(None)
                }
                other => other,
            }
        })?;
        Ok(resolved)
    }

    /// The sequence's normalisation of `clips` — a timeline whose clips are
    /// resolved — measured on the mix: its gain, and the mix before and
    /// after. `None` when the sequence is not normalised.
    ///
    /// # Errors
    ///
    /// A render or measurement failed, or the timeline cannot be planned.
    pub fn sequence(
        &self,
        clips: &Timeline,
        settings: &SequenceSettings,
        facts: &BTreeMap<SourceId, SourceFacts>,
    ) -> Result<Option<LoudnessReport>, ExportError> {
        let Some(target) = clips.loudness else {
            return Ok(None);
        };
        let planned = |timeline: &Timeline| {
            plan(timeline, settings, facts)
                .map_err(|error| ExportError::Declined(error.to_string()))
        };
        let before = self.mix(&planned(clips)?)?;
        let mut after = None;
        let gain = converge(target.target_lufs, target.ceiling_dbtp, &before, |gain| {
            let mut trial = clips.clone();
            apply_sequence(&mut trial, Some(gain));
            let measured = self.mix(&planned(&trial)?)?;
            after = Some((gain, measured));
            Ok::<_, ExportError>(measured)
        })?;
        let after = match after {
            Some((tried, measured)) if (tried - gain).abs() < f64::EPSILON => measured,
            _ => {
                let mut trial = clips.clone();
                apply_sequence(&mut trial, Some(gain));
                self.mix(&planned(&trial)?)?
            }
        };
        Ok(Some(LoudnessReport {
            before,
            after,
            target_lufs: target.target_lufs,
            ceiling_dbtp: target.ceiling_dbtp,
            gain_db: gain,
        }))
    }

    /// One clip's normalisation: its loudness before, the gain, and what
    /// comes out. `None` for a clip with no normalisation or no sound.
    ///
    /// # Errors
    ///
    /// As [`Resolver::clip`].
    pub fn clip_report(
        &self,
        placement: &Placement,
    ) -> Result<Option<LoudnessReport>, ExportError> {
        let Some((target, ceiling)) = placement.audio.iter().find_map(|step| match *step {
            AudioOperation::Normalise {
                target_lufs,
                ceiling_dbtp,
                bypassed: false,
                ..
            } => Some((target_lufs, ceiling_dbtp)),
            _ => None,
        }) else {
            return Ok(None);
        };
        let Some(before) = self.clip(placement)? else {
            return Ok(None);
        };
        let Some(gain) = self.clip_gain(placement, target, ceiling)? else {
            return Ok(None);
        };
        let after = self
            .clip_after(placement, gain)?
            .ok_or_else(|| ExportError::Mismatch("the clip lost its sound".to_owned()))?;
        Ok(Some(LoudnessReport {
            before,
            after,
            target_lufs: target,
            ceiling_dbtp: ceiling,
            gain_db: gain,
        }))
    }
}

fn measure_error(error: MeasureError) -> ExportError {
    match error {
        MeasureError::Cancelled => ExportError::Cancelled,
        MeasureError::Failed(reason) => ExportError::Mismatch(reason),
    }
}
