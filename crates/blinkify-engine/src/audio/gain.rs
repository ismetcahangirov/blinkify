//! Clip gain (#46): what the limiter will do at a given gain, and a gain to
//! suggest.
//!
//! A limiter that engages without saying so is a silent quality change — the
//! same principle as the export report — so the inspector shows how far it
//! turns the loudest peak down. That is worked out, not listened for: the
//! clip's true peak before the gain is measured once, and the limiter has to
//! take off whatever the gain puts above the ceiling. It is the figure for
//! the export, known before the export runs.
//!
//! The suggestion is advice. It is shown, with the limiting it would cause,
//! and applied only when the user applies it: setting a level unasked takes a
//! decision away from the user.

use std::path::Path;

use serde::Serialize;
use ts_rs::TS;

use super::chain::{ChainError, Stage, filters_before};
use super::denoise::Models;
use super::loudness::{Loudness, MeasureRequest};
use crate::probe::{MediaInfo, StreamKind};
use crate::project::TrackKind;
use crate::project::evaluate::{AudioOperation, Placement};

/// The integrated loudness a suggested gain aims at, in LUFS: the level
/// streaming services and podcast platforms recommend for spoken programmes.
pub const SUGGESTED_LUFS: f64 = -16.0;

/// Below this the limiter is not worth mentioning, in dB: it is under the
/// resolution of the measurement.
const AUDIBLE_LIMITING_DB: f64 = 0.05;

/// A clip's gain, what it costs, and what to try instead.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GainAdvice {
    /// The clip measured before its gain: after noise reduction, if any.
    pub before: Loudness,
    /// The gain in effect, in dB; 0 when there is none or it is bypassed.
    pub gain_db: f64,
    pub ceiling_dbtp: f64,
    /// How far the limiter turns the loudest peak down at this gain, in dB.
    /// 0 when it does not engage.
    pub limiting_db: f64,
    /// What the suggestion aims at.
    pub target_lufs: f64,
    /// The gain that brings the clip to `target_lufs`, to a tenth of a dB.
    /// `None` when the clip is too quiet to measure.
    pub suggested_db: Option<f64>,
    /// How far the limiter would turn the loudest peak down at the
    /// suggested gain.
    pub suggested_limiting_db: f64,
}

/// How far a limiter at `ceiling_dbtp` has to turn down a peak of
/// `true_peak_dbtp` raised by `gain_db`.
#[must_use]
pub fn limiting(true_peak_dbtp: Option<f64>, gain_db: f64, ceiling_dbtp: f64) -> f64 {
    let over = true_peak_dbtp.map_or(0.0, |peak| peak + gain_db - ceiling_dbtp);
    if over < AUDIBLE_LIMITING_DB {
        0.0
    } else {
        over
    }
}

/// The advice for a clip whose chain is `chain`, measured before its gain as
/// `before`.
#[must_use]
pub fn advise(before: Loudness, chain: &[AudioOperation]) -> GainAdvice {
    // The limiter is part of the gain step: present, and not bypassed, it
    // holds the ceiling whatever the gain — an input already over it is
    // limited at 0 dB.
    let (gain_db, ceiling_dbtp, limited) = chain
        .iter()
        .find_map(|step| match *step {
            AudioOperation::Gain {
                db,
                ceiling_dbtp,
                bypassed,
            } => Some((if bypassed { 0.0 } else { db }, ceiling_dbtp, !bypassed)),
            _ => None,
        })
        .unwrap_or((0.0, crate::project::DEFAULT_CEILING_DBTP, false));
    let suggested_db = before
        .integrated_lufs
        .map(|lufs| ((SUGGESTED_LUFS - lufs) * 10.0).round() / 10.0);
    GainAdvice {
        before,
        gain_db,
        ceiling_dbtp,
        limiting_db: if limited {
            limiting(before.true_peak_dbtp, gain_db, ceiling_dbtp)
        } else {
            0.0
        },
        target_lufs: SUGGESTED_LUFS,
        suggested_db,
        suggested_limiting_db: suggested_db
            .map_or(0.0, |db| limiting(before.true_peak_dbtp, db, ceiling_dbtp)),
    }
}

/// What to measure for the sound `placement` plays, at `stage` of its chain:
/// its range of its sound stream, through the steps before `stage`. `None`
/// when the clip has no sound — a silent video clip, or a source without an
/// audio stream.
///
/// # Errors
///
/// A step before `stage` needs a model that is not there.
pub fn measure_request(
    placement: &Placement,
    path: &Path,
    info: &MediaInfo,
    stage: Stage,
    models: Option<&Models>,
) -> Result<Option<MeasureRequest>, ChainError> {
    if placement.silent {
        return Ok(None);
    }
    let stream = match placement.kind {
        TrackKind::Audio => info
            .streams
            .iter()
            .find(|stream| stream.index == placement.stream),
        TrackKind::Video => {
            let streams: Vec<_> = info.audio().collect();
            let default = streams.iter().position(|(stream, _)| stream.is_default);
            streams
                .into_iter()
                .nth(default.unwrap_or(0))
                .map(|(stream, _)| stream)
        }
    };
    let Some(stream) = stream else {
        return Ok(None);
    };
    let StreamKind::Audio(audio) = &stream.kind else {
        return Ok(None);
    };
    let sample_rate = audio.sample_rate.unwrap_or(48_000);
    Ok(Some(MeasureRequest {
        source: path.to_path_buf(),
        stream: stream.index,
        start_seconds: crate::time::seconds(placement.source_in, placement.time_base),
        end_seconds: crate::time::seconds(placement.source_out, placement.time_base),
        sample_rate,
        channels: if audio.channels == Some(1) { 1 } else { 2 },
        filters: filters_before(&placement.audio, stage, sample_rate, models)?,
    }))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn measured(lufs: Option<f64>, peak: Option<f64>) -> Loudness {
        Loudness {
            integrated_lufs: lufs,
            range_lu: None,
            true_peak_dbtp: peak,
            seconds: 10.0,
        }
    }

    fn gain(db: f64) -> AudioOperation {
        AudioOperation::Gain {
            db,
            ceiling_dbtp: -1.0,
            bypassed: false,
        }
    }

    #[test]
    fn the_limiter_takes_off_exactly_what_the_gain_puts_over_the_ceiling() {
        assert_eq!(limiting(Some(-6.0), 3.0, -1.0), 0.0);
        assert!((limiting(Some(-6.0), 9.0, -1.0) - 4.0).abs() < 1e-12);
        // An input already over the ceiling is limited even at no gain.
        assert!((limiting(Some(0.5), 0.0, -1.0) - 1.5).abs() < 1e-12);
        assert_eq!(limiting(None, 40.0, -1.0), 0.0);
    }

    #[test]
    fn advice_reads_the_clips_own_gain_and_suggests_the_target() {
        let advice = advise(measured(Some(-26.04), Some(-9.0)), &[gain(12.0)]);
        assert_eq!(advice.gain_db, 12.0);
        assert!((advice.limiting_db - 4.0).abs() < 1e-12);
        assert_eq!(advice.suggested_db, Some(10.0));
        assert!((advice.suggested_limiting_db - 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_bypassed_or_absent_gain_does_not_limit() {
        let bypassed = AudioOperation::Gain {
            db: 20.0,
            ceiling_dbtp: -1.0,
            bypassed: true,
        };
        assert_eq!(
            advise(measured(Some(-20.0), Some(-1.0)), &[bypassed]).limiting_db,
            0.0
        );
        assert_eq!(
            advise(measured(Some(-20.0), Some(0.0)), &[]).limiting_db,
            0.0
        );
        // Present at 0 dB, it still holds the ceiling.
        assert!(
            (advise(measured(Some(-20.0), Some(0.0)), &[gain(0.0)]).limiting_db - 1.0).abs()
                < 1e-12
        );
    }

    #[test]
    fn silence_gets_no_suggestion() {
        let advice = advise(measured(None, None), &[gain(6.0)]);
        assert_eq!(advice.suggested_db, None);
        assert_eq!(advice.limiting_db, 0.0);
    }
}
