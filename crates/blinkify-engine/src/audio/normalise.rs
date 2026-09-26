//! Two-pass loudness normalisation (#48): measure, then apply one gain.
//!
//! The first pass measures a clip's integrated loudness (EBU R128, through
//! `audio::loudness`) at the point of the chain normalisation runs — after
//! noise reduction and gain. The second pass is a single gain, the target
//! less that measurement, followed by a true-peak limiter at the
//! normalisation's own ceiling: the last constraint on the peaks, since
//! raising the level can push peaks the gain limiter held back over again.
//!
//! One gain for the whole clip is the point. FFmpeg's single-pass `loudnorm`
//! is a dynamic normaliser that rides the level and pumps on speech; its
//! two-pass form still switches to that dynamic mode, silently, whenever the
//! measured peak would pass its ceiling. Here the level never moves with the
//! sound: where the peaks would pass the ceiling, the limiter catches them.
//! ADR-0013.
//!
//! This module works on an evaluated [`Timeline`] and leaves the measuring
//! to its callers — the export resolves by measuring what is missing, the
//! preview only from the cache — so the arithmetic is the same for both.

use super::loudness::Loudness;
use crate::project::evaluate::{AudioOperation, Placement, Timeline};

/// The target a new normalisation starts at, in LUFS: what `YouTube` and the
/// music streaming services play back at.
pub const DEFAULT_TARGET_LUFS: f64 = -14.0;

/// The gain that brings sound measured as `measured` to `target_lufs`, in
/// dB, to a hundredth. Sound too quiet to measure — nothing passed R128's
/// gate — is left where it is: there is no loudness to move.
#[must_use]
pub fn gain_for(target_lufs: f64, measured: &Loudness) -> f64 {
    measured
        .integrated_lufs
        .map_or(0.0, |lufs| ((target_lufs - lufs) * 100.0).round() / 100.0)
}

/// How close to the target a resolved gain must bring the sound, in LU.
pub const TOLERANCE_LU: f64 = 0.1;

/// How many times a gain is measured again at most when the limiter takes
/// loudness off: each pass closes most of what is left.
const PASSES: usize = 4;

/// The gain that brings the sound to `target_lufs` once the limiter at
/// `ceiling_dbtp` has done its work.
///
/// `first` is the loudness before normalisation. Where its true peak, raised
/// by the plain gain, stays under the ceiling, the limiter will be idle and
/// the plain gain is exact. Where it would not, the limiter takes a little
/// loudness off, so `measure` measures the result of a candidate gain and the
/// shortfall is added — still one gain for the whole clip, never a level that
/// moves with the sound.
///
/// # Errors
///
/// Whatever `measure` returns.
pub fn converge<E>(
    target_lufs: f64,
    ceiling_dbtp: f64,
    first: &Loudness,
    mut measure: impl FnMut(f64) -> Result<Loudness, E>,
) -> Result<f64, E> {
    let mut gain = gain_for(target_lufs, first);
    let (Some(_), Some(peak)) = (first.integrated_lufs, first.true_peak_dbtp) else {
        return Ok(gain);
    };
    if peak + gain <= ceiling_dbtp - super::chain::LIMITER_MARGIN_DB {
        return Ok(gain);
    }
    for _ in 0..PASSES {
        let Some(reached) = measure(gain)?.integrated_lufs else {
            break;
        };
        let short = target_lufs - reached;
        if short.abs() <= TOLERANCE_LU {
            break;
        }
        gain = ((gain + short) * 100.0).round() / 100.0;
    }
    Ok(gain)
}

/// A digest of `timeline`: two timelines with the same clips, chains and
/// resolved gains have the same one. The sequence's gain is measured on a
/// mix and remembered against the digest of the clips it was measured on,
/// so any change to them makes it unknown again.
#[must_use]
pub fn fingerprint(timeline: &Timeline) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let json = serde_json::to_vec(timeline).unwrap_or_default();
    Sha256::digest(&json)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Whether `placement` asks for a normalisation that is not resolved yet.
#[must_use]
pub fn needs_measuring(placement: &Placement) -> bool {
    placement.audio.iter().any(|step| {
        matches!(
            step,
            AudioOperation::Normalise {
                bypassed: false,
                gain_db: None,
                ..
            }
        )
    })
}

/// Resolve every clip's own normalisation in `timeline`: `resolve` answers,
/// for a placement and its normalisation's target and ceiling, the gain —
/// or `None` when that is not known, and the step stays unresolved.
///
/// # Errors
///
/// Whatever `resolve` returns.
pub fn resolve_clips<E>(
    timeline: &mut Timeline,
    mut resolve: impl FnMut(&Placement, f64, f64) -> Result<Option<f64>, E>,
) -> Result<(), E> {
    for placement in timeline
        .tracks
        .iter_mut()
        .flat_map(|track| track.placements.iter_mut())
    {
        if !needs_measuring(placement) {
            continue;
        }
        let Some((target, ceiling)) = placement.audio.iter().find_map(|step| match *step {
            AudioOperation::Normalise {
                target_lufs,
                ceiling_dbtp,
                bypassed: false,
                gain_db: None,
            } => Some((target_lufs, ceiling_dbtp)),
            _ => None,
        }) else {
            continue;
        };
        let Some(resolved) = resolve(placement, target, ceiling)? else {
            continue;
        };
        for step in &mut placement.audio {
            if let AudioOperation::Normalise {
                bypassed: false,
                gain_db,
                ..
            } = step
                && gain_db.is_none()
            {
                *gain_db = Some(resolved);
            }
        }
    }
    Ok(())
}

/// Apply the sequence's own loudness target, if it has one: every clip with
/// sound gets a last normalisation step with the same gain, `gain_db` — the
/// target less the loudness of the whole mix — so the levels between clips
/// the user set are kept. With the mix not measured yet, the steps are added
/// unresolved: the plan still knows every clip's sound is processed, and the
/// export still refuses until it is measured.
pub fn apply_sequence(timeline: &mut Timeline, gain_db: Option<f64>) {
    let Some(target) = timeline.loudness else {
        return;
    };
    for placement in timeline
        .tracks
        .iter_mut()
        .flat_map(|track| track.placements.iter_mut())
        .filter(|placement| !placement.silent)
    {
        placement.audio.push(AudioOperation::Normalise {
            target_lufs: target.target_lufs,
            ceiling_dbtp: target.ceiling_dbtp,
            bypassed: false,
            gain_db,
        });
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::probe::Rational;
    use crate::project::evaluate::EvaluatedTrack;
    use crate::project::{LoudnessTarget, TrackKind};

    fn loudness(lufs: Option<f64>) -> Loudness {
        Loudness {
            integrated_lufs: lufs,
            range_lu: None,
            true_peak_dbtp: None,
            seconds: 1.0,
        }
    }

    fn placement(clip: u32, audio: Vec<AudioOperation>) -> Placement {
        Placement {
            track: 1,
            kind: TrackKind::Video,
            clip,
            source: 1,
            stream: 0,
            time_base: Rational { num: 1, den: 1000 },
            source_in: 0,
            source_out: 1000,
            start: 0,
            length: 30,
            speed: Rational { num: 1, den: 1 },
            audio,
            sequence_time_base: Rational { num: 1, den: 30 },
            motion: None,
            silent: false,
            forced: None,
        }
    }

    fn timeline(placements: Vec<Placement>) -> Timeline {
        Timeline {
            time_base: Rational { num: 1, den: 30 },
            frame_rate: Rational { num: 30, den: 1 },
            tracks: vec![EvaluatedTrack {
                id: 1,
                kind: TrackKind::Video,
                visible: true,
                audible: true,
                placements,
            }],
            loudness: None,
        }
    }

    fn normalise(target_lufs: f64) -> AudioOperation {
        AudioOperation::Normalise {
            target_lufs,
            ceiling_dbtp: -1.0,
            bypassed: false,
            gain_db: None,
        }
    }

    fn gains(timeline: &Timeline) -> Vec<Vec<Option<f64>>> {
        timeline
            .placements()
            .map(|p| {
                p.audio
                    .iter()
                    .filter_map(|step| match step {
                        AudioOperation::Normalise { gain_db, .. } => Some(*gain_db),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_gain_is_the_target_less_the_measurement_and_silence_is_left_alone() {
        assert_eq!(gain_for(-14.0, &loudness(Some(-23.456))), 9.46);
        assert_eq!(gain_for(-23.0, &loudness(Some(-9.0))), -14.0);
        assert_eq!(gain_for(-14.0, &loudness(None)), 0.0);
    }

    #[test]
    fn a_gain_the_limiter_leaves_alone_is_not_measured_again() {
        let quiet = Loudness {
            integrated_lufs: Some(-30.0),
            range_lu: None,
            true_peak_dbtp: Some(-20.0),
            seconds: 1.0,
        };
        let gain = converge(-16.0, -1.0, &quiet, |_| -> Result<Loudness, ()> {
            panic!("measured")
        })
        .expect("exact");
        assert_eq!(gain, 14.0);
    }

    #[test]
    fn a_gain_the_limiter_takes_loudness_from_is_raised_until_it_arrives() {
        // A limiter that takes a quarter of whatever is over the ceiling.
        let first = Loudness {
            integrated_lufs: Some(-26.0),
            range_lu: None,
            true_peak_dbtp: Some(-8.0),
            seconds: 1.0,
        };
        let reached = |gain: f64| {
            let over = (-8.0 + gain - -1.0_f64).max(0.0);
            -26.0 + gain - over / 4.0
        };
        let gain = converge(-14.0, -1.0, &first, |g| -> Result<Loudness, ()> {
            Ok(Loudness {
                integrated_lufs: Some(reached(g)),
                ..first
            })
        })
        .expect("converged");
        assert!((reached(gain) - -14.0).abs() <= TOLERANCE_LU, "{gain}");
        assert!(gain > 12.0);
    }

    #[test]
    fn each_clip_is_resolved_from_its_own_measurement() {
        let mut t = timeline(vec![
            placement(1, vec![normalise(-14.0)]),
            placement(2, vec![normalise(-23.0)]),
            placement(3, vec![]),
        ]);
        let mut asked = Vec::new();
        resolve_clips(&mut t, |p, target, _| -> Result<_, ()> {
            asked.push(p.clip);
            Ok((p.clip == 1).then(|| gain_for(target, &loudness(Some(-20.0)))))
        })
        .expect("resolved");
        // The clip without a normalisation is never measured, and one whose
        // measurement is not known stays unresolved.
        assert_eq!(asked, vec![1, 2]);
        assert_eq!(gains(&t), vec![vec![Some(6.0)], vec![None], vec![]]);
    }

    #[test]
    fn the_sequence_target_moves_every_clip_by_the_same_gain() {
        let mut t = timeline(vec![
            placement(1, vec![]),
            placement(2, vec![normalise(-14.0)]),
        ]);
        t.loudness = Some(LoudnessTarget {
            target_lufs: -16.0,
            ceiling_dbtp: -1.5,
        });
        apply_sequence(&mut t, Some(gain_for(-16.0, &loudness(Some(-22.0)))));
        assert_eq!(gains(&t), vec![vec![Some(6.0)], vec![None, Some(6.0)]]);
        // Unmeasured, every clip still carries the step.
        let mut u = timeline(vec![placement(1, vec![])]);
        u.loudness = t.loudness;
        apply_sequence(&mut u, None);
        assert_eq!(gains(&u), vec![vec![None]]);
    }
}
