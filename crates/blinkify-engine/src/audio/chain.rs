//! The audio filter chain (Epic #7): the one description of what a clip's
//! gain, noise reduction and normalisation do to its sound, as FFmpeg
//! filters.
//!
//! Both the preview decoder and the export encoder take their filters from
//! [`filters`], and nothing else builds them. Two chains — one for hearing,
//! one for writing — would drift apart, and the user would only find out
//! after exporting. The export's own tests assert that both commands carry
//! exactly what this module returns.
//!
//! The order is fixed, whatever order the clip's operations were added in,
//! because a different order is a different and worse result:
//!
//! 1. **Noise reduction** first, on the sound as it was recorded: gain
//!    before it would raise the noise the denoiser is about to attack, and a
//!    limiter before it would flatten the peaks it uses to tell speech from
//!    noise.
//! 2. **Gain**, then its **true-peak limiter**: the limiter exists only to
//!    catch what the gain pushed over the ceiling.
//! 3. **Normalisation** last, with its own ceiling as the final constraint on
//!    the peaks, since raising the level can push peaks the gain limiter held
//!    back over again.
//!
//! The chain runs at the rate the sound is heading to — the output device's
//! in the preview, the encoding's in the export — after the source is trimmed
//! and resampled and before any speed change, so the denoiser hears speech at
//! its natural pace. See `docs/architecture/audio-chain.md` and ADR-0011.

use std::fmt::Write as _;

use thiserror::Error;

use super::denoise::{self, Models};
pub use crate::project::AudioStage as Stage;
use crate::project::evaluate::AudioOperation;

/// The factor the true-peak limiter oversamples by. ITU-R BS.1770-4 measures
/// true peak at four times 48 kHz; peaks between samples are found there.
pub const OVERSAMPLE: u32 = 4;

/// How far below the ceiling the oversampled limiter holds, in dB. Limiting
/// makes new high-frequency content, and bringing the sound back to its own
/// rate reshapes the peaks by up to about this much; the margin keeps the
/// reshaped peaks under the ceiling. Measured, not guessed: ADR-0011.
pub const LIMITER_MARGIN_DB: f64 = 0.5;

/// The limiter's look-ahead, in milliseconds: how early it starts to turn a
/// peak down. `alimiter`'s own default.
const ATTACK_MS: f64 = 5.0;

/// How quickly the limiter lets go, in milliseconds. Long enough not to
/// pump on speech; short enough not to duck the syllable after a peak.
const RELEASE_MS: f64 = 50.0;

/// Whether the chain applies `step` itself: every step of Epic #7 now.
/// Kept as the one place a new step is declared before it is built.
#[must_use]
pub fn applies(step: &AudioOperation) -> bool {
    matches!(
        step,
        AudioOperation::Gain { .. }
            | AudioOperation::Denoise { .. }
            | AudioOperation::Normalise { .. }
    )
}

/// Why a chain could not be built.
#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum ChainError {
    /// Noise reduction was asked for and its model is not installed.
    #[error("noise reduction needs its model, which is missing from the installation or damaged")]
    NoModel,
    /// Loudness normalisation was asked for and its first pass — the
    /// measurement — has not been made.
    #[error("loudness normalisation has not measured the sound yet")]
    NotMeasured,
}

/// The FFmpeg filters that apply `chain` to sound at `sample_rate`, in the
/// fixed order, comma-separated: empty when nothing applies. A bypassed
/// step is absent, exactly. `models` are the bundled models, if they could
/// be found.
///
/// # Errors
///
/// A step needs a model that is not there.
pub fn filters(
    chain: &[AudioOperation],
    sample_rate: u32,
    models: Option<&Models>,
) -> Result<String, ChainError> {
    build(chain.iter(), sample_rate, models)
}

/// The filters of the steps that run before `stage`: what a measurement at
/// that point in the chain hears.
///
/// # Errors
///
/// As [`filters`].
pub fn filters_before(
    chain: &[AudioOperation],
    stage: Stage,
    sample_rate: u32,
    models: Option<&Models>,
) -> Result<String, ChainError> {
    build(
        chain.iter().filter(|step| step.stage() < stage),
        sample_rate,
        models,
    )
}

/// What the preview plays: [`filters`], leaving out any step that cannot be
/// built yet — noise reduction without its model, a normalisation not yet
/// measured. The preview keeps playing; the diagnostics and the inspector say
/// what is not heard, and the export refuses rather than leave it out.
#[must_use]
pub fn playable(chain: &[AudioOperation], sample_rate: u32, models: Option<&Models>) -> String {
    build(
        chain
            .iter()
            .filter(|step| build(std::iter::once(*step), sample_rate, models).is_ok()),
        sample_rate,
        models,
    )
    .unwrap_or_default()
}

fn build<'a>(
    chain: impl Iterator<Item = &'a AudioOperation>,
    sample_rate: u32,
    models: Option<&Models>,
) -> Result<String, ChainError> {
    let mut steps: Vec<&AudioOperation> = chain.filter(|step| !step.bypassed()).collect();
    steps.sort_by_key(|step| step.stage());
    let mut out = Vec::new();
    for step in steps {
        match *step {
            AudioOperation::Denoise { strength, .. } => {
                let models = models.ok_or(ChainError::NoModel)?;
                out.push(denoise::filters(models, strength, sample_rate));
            }
            AudioOperation::Gain {
                db, ceiling_dbtp, ..
            } => {
                out.push(format!("volume={db}dB"));
                out.extend(true_peak_limiter(ceiling_dbtp, sample_rate));
            }
            AudioOperation::Normalise {
                ceiling_dbtp,
                gain_db,
                ..
            } => {
                // The second pass: one gain for the whole clip, from its
                // measured loudness, and a limiter holding the ceiling after
                // it — never a level that moves with the sound (ADR-0013).
                let db = gain_db.ok_or(ChainError::NotMeasured)?;
                out.push(format!("volume={db:.2}dB"));
                out.extend(true_peak_limiter(ceiling_dbtp, sample_rate));
            }
        }
    }
    Ok(out.join(","))
}

/// A limiter that holds the sound's true peak — the peak of the waveform the
/// samples describe, not of the samples — at or below `ceiling_dbtp`.
///
/// `alimiter` sees only samples, so it runs twice: once at four times the
/// rate, `LIMITER_MARGIN_DB` under the ceiling, where the peaks between
/// samples are samples themselves; and once at the sound's own rate, at the
/// ceiling, which no sample then exceeds. Both compensate their look-ahead,
/// so the sound is neither delayed nor lengthened.
fn true_peak_limiter(ceiling_dbtp: f64, sample_rate: u32) -> [String; 4] {
    let high = sample_rate.saturating_mul(OVERSAMPLE);
    let limiter = |dbtp: f64| {
        let mut filter = String::new();
        let _ = write!(
            filter,
            "alimiter=limit={:.6}:level=0:latency=1:attack={ATTACK_MS}:release={RELEASE_MS}",
            linear(dbtp)
        );
        filter
    };
    [
        format!("aresample={high}:resampler=soxr:precision=28"),
        limiter(ceiling_dbtp - LIMITER_MARGIN_DB),
        format!("aresample={sample_rate}:resampler=soxr:precision=28"),
        limiter(ceiling_dbtp),
    ]
}

/// `db` decibels as a linear factor.
#[must_use]
pub fn linear(db: f64) -> f64 {
    10_f64.powf(db / 20.0)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    fn gain(db: f64, ceiling_dbtp: f64) -> AudioOperation {
        AudioOperation::Gain {
            db,
            ceiling_dbtp,
            bypassed: false,
        }
    }

    #[test]
    fn nothing_to_apply_is_no_filter_at_all() {
        assert_eq!(filters(&[], 48_000, None).expect("built"), "");
        let bypassed = AudioOperation::Gain {
            db: 6.0,
            ceiling_dbtp: -1.0,
            bypassed: true,
        };
        assert_eq!(filters(&[bypassed], 48_000, None).expect("built"), "");
    }

    #[test]
    fn gain_is_followed_by_an_oversampled_limiter_and_a_guard_at_the_ceiling() {
        let chain = filters(&[gain(6.0, -1.0)], 44_100, None).expect("built");
        let steps: Vec<&str> = chain.split(',').collect();
        assert_eq!(steps.len(), 5, "{chain}");
        assert_eq!(steps.first(), Some(&"volume=6dB"));
        assert_eq!(
            steps.get(1),
            Some(&"aresample=176400:resampler=soxr:precision=28")
        );
        // −1.5 dBTP inside, −1 dBTP at the sound's own rate.
        assert!(
            steps
                .get(2)
                .is_some_and(|s| s.starts_with("alimiter=limit=0.841395:level=0:latency=1")),
            "{chain}"
        );
        assert_eq!(
            steps.get(3),
            Some(&"aresample=44100:resampler=soxr:precision=28")
        );
        assert!(
            steps
                .get(4)
                .is_some_and(|s| s.starts_with("alimiter=limit=0.891251:level=0:latency=1")),
            "{chain}"
        );
    }

    #[test]
    fn the_order_is_fixed_whatever_order_the_steps_were_added_in() {
        let models = Models::in_dir(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("models/rnnoise"),
        )
        .expect("bundled");
        let denoise = AudioOperation::Denoise {
            strength: 0.5,
            bypassed: false,
        };
        let normalise = AudioOperation::Normalise {
            target_lufs: -16.0,
            ceiling_dbtp: -1.0,
            bypassed: false,
            gain_db: Some(4.5),
        };
        let one = filters(
            &[normalise, gain(-3.0, -2.0), denoise],
            48_000,
            Some(&models),
        )
        .expect("built");
        let other = filters(
            &[denoise, normalise, gain(-3.0, -2.0)],
            48_000,
            Some(&models),
        )
        .expect("built");
        assert_eq!(one, other);
        let arnndn = one.find("arnndn").expect("denoised");
        let volume = one.find("volume=-3dB").expect("gained");
        let normalised = one.find("volume=4.50dB").expect("normalised");
        assert!(arnndn < volume && volume < normalised, "{one}");
        // The normalisation has its own limiter, at its own ceiling.
        assert_eq!(one.matches("alimiter").count(), 4, "{one}");
        assert!(applies(&denoise) && applies(&gain(0.0, -1.0)) && applies(&normalise));
    }

    #[test]
    fn an_unmeasured_normalisation_cannot_be_exported_and_is_not_heard_until_it_is() {
        let unmeasured = AudioOperation::Normalise {
            target_lufs: -16.0,
            ceiling_dbtp: -1.0,
            bypassed: false,
            gain_db: None,
        };
        let chain = [gain(2.0, -1.0), unmeasured];
        assert_eq!(filters(&chain, 48_000, None), Err(ChainError::NotMeasured));
        assert_eq!(
            playable(&chain, 48_000, None),
            filters(&[gain(2.0, -1.0)], 48_000, None).expect("built")
        );
    }

    #[test]
    fn noise_reduction_without_its_model_is_an_error_to_export_and_left_out_of_the_preview() {
        let denoise = AudioOperation::Denoise {
            strength: 0.5,
            bypassed: false,
        };
        let chain = [denoise, gain(2.0, -1.0)];
        assert_eq!(filters(&chain, 48_000, None), Err(ChainError::NoModel));
        let heard = playable(&chain, 48_000, None);
        assert!(heard.starts_with("volume=2dB,"), "{heard}");
        assert!(!heard.contains("arnndn"));
        // Bypassed, it needs nothing.
        let bypassed = AudioOperation::Denoise {
            strength: 0.5,
            bypassed: true,
        };
        assert_eq!(filters(&[bypassed], 48_000, None), Ok(String::new()));
    }
}
