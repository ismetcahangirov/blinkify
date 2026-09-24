//! Sequence settings: the shape of the timeline, and the rule that binds it
//! to copy eligibility (#57).
//!
//! A stream copy moves a source's packets into the output unchanged, so it is
//! only possible where the sequence *is* the source's shape: the same picture
//! size, the same frame rate, the same pixel aspect. A 1080p sequence holding
//! 4K footage, or a 24 fps sequence holding 30 fps footage, re-encodes every
//! frame of it. That makes these settings the one switch that can turn the
//! whole product off, so:
//!
//! - a new sequence **matches its first clip** exactly, and the common case is
//!   copy-eligible without the user knowing the rule exists;
//! - [`copy_eligibility`] is the **one** predicate: the planner (#39), the
//!   inspector (#56) and the export dialog (#50) read its answer, and none of
//!   them works it out again;
//! - colour is **SDR (Rec. 709)** in v1. An HDR source is copied as recorded
//!   where it can be copied; anything of it that would have to be rendered is
//!   declined with the reason, never tone-mapped (ADR-0008).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::probe::{FrameRateMode, Rational, VideoInfo};

/// How the sequence treats colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum ColourPolicy {
    /// Standard dynamic range, Rec. 709: what the preview shows and what a
    /// re-encoded segment is made in. The only policy in v1.
    Sdr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SequenceSettings {
    pub width: u32,
    pub height: u32,
    /// Frames per second, exactly: `30000/1001`, not `29.97`.
    pub frame_rate: Rational,
    pub pixel_aspect: Rational,
    pub colour: ColourPolicy,
}

/// The largest picture a sequence may have: what H.264 level 6.2 and HEVC
/// encode, and what every container Blinkify writes can carry.
pub const MAX_DIMENSION: u32 = 8192;

/// The frame rates a sequence may run at, in frames per second.
pub const FRAME_RATE_RANGE: (i64, i64) = (1, 240);

/// Why a sequence setting cannot be used.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SettingsError {
    #[error(
        "a picture of {width}×{height} cannot be encoded: each side must be even and between 2 and {MAX_DIMENSION}"
    )]
    Size { width: u32, height: u32 },
    #[error(
        "{num}/{den} is not a frame rate a video file can carry: it must be between 1 and 240 frames a second"
    )]
    FrameRate { num: i64, den: i64 },
    #[error("{num}:{den} is not a pixel aspect ratio")]
    PixelAspect { num: i64, den: i64 },
}

/// Why a source cannot be stream-copied into the sequence. Each is reported
/// separately — #39 requires the preconditions to be separately reportable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "reason",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Mismatch {
    Resolution {
        sequence_width: u32,
        sequence_height: u32,
        source_width: u32,
        source_height: u32,
    },
    FrameRate {
        sequence: Rational,
        source: Rational,
    },
    PixelAspect {
        sequence: Rational,
        source: Rational,
    },
}

/// Something true of a copy-eligible source that the user should know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum CopyNote {
    /// Variable frame rate: copied with its timing as recorded, never
    /// conformed to the sequence's grid.
    VariableFrameRate,
    /// HDR: copied as recorded. A part of it that would have to be rendered
    /// — a cut off a keyframe, a filter — is declined in v1, not tone-mapped.
    Hdr,
}

/// Whether a source's pictures can be stream-copied into the sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CopyEligibility {
    /// No mismatch: the geometry permits a copy. Keyframes and filters are
    /// the planner's other preconditions, decided per segment (#39).
    pub eligible: bool,
    pub mismatches: Vec<Mismatch>,
    pub notes: Vec<CopyNote>,
}

/// What the sequence needs to know about a source's pictures: its shape as
/// displayed, its rate, whether the rate varies, whether it is HDR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StreamGeometry {
    /// As displayed: rotation applied, since the copied stream carries its
    /// rotation and plays upright.
    pub width: u32,
    pub height: u32,
    /// The nominal rate: the average for a constant-rate source; for a
    /// variable one, the rate its timestamps are based on.
    pub frame_rate: Rational,
    pub pixel_aspect: Rational,
    pub variable_frame_rate: bool,
    pub hdr: bool,
}

/// `num/den` in lowest terms, with a positive denominator.
#[must_use]
pub fn reduced(rate: Rational) -> Rational {
    let (mut a, mut b) = (rate.num.unsigned_abs(), rate.den.unsigned_abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let divisor = i64::try_from(a.max(1)).unwrap_or(1);
    let sign = if rate.den < 0 { -1 } else { 1 };
    Rational {
        // Exact: the divisor divides both.
        num: sign * rate.num.div_euclid(divisor),
        den: sign * rate.den.div_euclid(divisor),
    }
}

impl StreamGeometry {
    /// The geometry of a probed video stream; `None` for a cover image or a
    /// stream with no known rate.
    #[must_use]
    pub fn of(video: &VideoInfo) -> Option<Self> {
        if video.is_attached_picture || video.display_width == 0 || video.display_height == 0 {
            return None;
        }
        let variable = video.frame_rate.mode == FrameRateMode::Variable;
        let nominal = if variable {
            video.frame_rate.real_base.or(video.frame_rate.average)
        } else {
            video.frame_rate.average.or(video.frame_rate.real_base)
        }
        .filter(|rate| rate.num > 0 && rate.den > 0)?;
        let pixel_aspect = video
            .sample_aspect_ratio
            .filter(|ratio| ratio.num > 0 && ratio.den > 0)
            .map_or(Rational { num: 1, den: 1 }, reduced);
        Some(Self {
            width: video.display_width,
            height: video.display_height,
            frame_rate: reduced(nominal),
            pixel_aspect,
            variable_frame_rate: variable,
            hdr: video.hdr.is_some(),
        })
    }
}

impl SequenceSettings {
    /// The sequence's time base: one tick per frame. Every clip position on
    /// the timeline is counted in it, so a clip always starts on a frame.
    #[must_use]
    pub fn time_base(&self) -> Rational {
        Rational {
            num: self.frame_rate.den,
            den: self.frame_rate.num,
        }
    }

    /// The picture's shape on screen: width × pixel aspect over height.
    #[must_use]
    pub fn display_aspect(&self) -> Rational {
        reduced(Rational {
            num: i64::from(self.width) * self.pixel_aspect.num,
            den: i64::from(self.height) * self.pixel_aspect.den,
        })
    }

    /// Settings that are exactly `source`'s: the `match first clip` default,
    /// under which that clip is copy-eligible.
    ///
    /// # Errors
    ///
    /// The source's shape is one no sequence can have — an odd size, an
    /// absurd rate. The sequence keeps its settings and the clip is not
    /// copy-eligible, which is said, not hidden.
    pub fn matching(source: &StreamGeometry) -> Result<Self, SettingsError> {
        let settings = Self {
            width: source.width,
            height: source.height,
            frame_rate: source.frame_rate,
            pixel_aspect: source.pixel_aspect,
            colour: ColourPolicy::Sdr,
        };
        settings.validate()?;
        Ok(settings)
    }

    /// Whether a video file can be made at these settings.
    ///
    /// # Errors
    ///
    /// The first thing that cannot.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let side = |value: u32| (2..=MAX_DIMENSION).contains(&value) && value.is_multiple_of(2);
        if !side(self.width) || !side(self.height) {
            return Err(SettingsError::Size {
                width: self.width,
                height: self.height,
            });
        }
        let rate = self.frame_rate;
        let (low, high) = FRAME_RATE_RANGE;
        let in_range = rate.num > 0
            && rate.den > 0
            && i128::from(rate.num) >= i128::from(low) * i128::from(rate.den)
            && i128::from(rate.num) <= i128::from(high) * i128::from(rate.den);
        if !in_range {
            return Err(SettingsError::FrameRate {
                num: rate.num,
                den: rate.den,
            });
        }
        if self.pixel_aspect.num <= 0 || self.pixel_aspect.den <= 0 {
            return Err(SettingsError::PixelAspect {
                num: self.pixel_aspect.num,
                den: self.pixel_aspect.den,
            });
        }
        Ok(())
    }
}

impl Default for SequenceSettings {
    /// 1080p30, square pixels: what a sequence has before its first clip
    /// gives it its own.
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate: Rational { num: 30, den: 1 },
            pixel_aspect: Rational { num: 1, den: 1 },
            colour: ColourPolicy::Sdr,
        }
    }
}

/// The copy-eligibility predicate: can `source`'s pictures be stream-copied
/// into a sequence with `settings`? The only place this is decided.
#[must_use]
pub fn copy_eligibility(settings: &SequenceSettings, source: &StreamGeometry) -> CopyEligibility {
    let mut mismatches = Vec::new();
    if settings.width != source.width || settings.height != source.height {
        mismatches.push(Mismatch::Resolution {
            sequence_width: settings.width,
            sequence_height: settings.height,
            source_width: source.width,
            source_height: source.height,
        });
    }
    if reduced(settings.frame_rate) != reduced(source.frame_rate) {
        mismatches.push(Mismatch::FrameRate {
            sequence: settings.frame_rate,
            source: source.frame_rate,
        });
    }
    if reduced(settings.pixel_aspect) != reduced(source.pixel_aspect) {
        mismatches.push(Mismatch::PixelAspect {
            sequence: settings.pixel_aspect,
            source: source.pixel_aspect,
        });
    }
    let mut notes = Vec::new();
    if source.variable_frame_rate {
        notes.push(CopyNote::VariableFrameRate);
    }
    if source.hdr {
        notes.push(CopyNote::Hdr);
    }
    CopyEligibility {
        eligible: mismatches.is_empty(),
        mismatches,
        notes,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    const R30: Rational = Rational { num: 30, den: 1 };
    const SQUARE: Rational = Rational { num: 1, den: 1 };

    fn phone() -> StreamGeometry {
        StreamGeometry {
            width: 1080,
            height: 1920,
            frame_rate: R30,
            pixel_aspect: SQUARE,
            variable_frame_rate: false,
            hdr: false,
        }
    }

    #[test]
    fn a_sequence_made_from_a_clip_can_copy_it() {
        let settings = SequenceSettings::matching(&phone()).expect("valid");
        assert_eq!((settings.width, settings.height), (1080, 1920));
        assert_eq!(settings.display_aspect(), Rational { num: 9, den: 16 });
        let eligibility = copy_eligibility(&settings, &phone());
        assert!(eligibility.eligible);
        assert!(eligibility.mismatches.is_empty());
    }

    #[test]
    fn each_mismatch_is_named_on_its_own() {
        let settings = SequenceSettings::default();
        let uhd = StreamGeometry {
            width: 3840,
            height: 2160,
            ..phone()
        };
        let eligibility = copy_eligibility(&settings, &uhd);
        assert!(!eligibility.eligible);
        assert_eq!(
            eligibility.mismatches,
            vec![Mismatch::Resolution {
                sequence_width: 1920,
                sequence_height: 1080,
                source_width: 3840,
                source_height: 2160
            }]
        );

        let film = StreamGeometry {
            width: 1920,
            height: 1080,
            frame_rate: Rational { num: 24, den: 1 },
            ..phone()
        };
        let eligibility = copy_eligibility(&settings, &film);
        assert_eq!(
            eligibility.mismatches,
            vec![Mismatch::FrameRate {
                sequence: R30,
                source: Rational { num: 24, den: 1 }
            }]
        );

        let anamorphic = StreamGeometry {
            width: 1920,
            height: 1080,
            pixel_aspect: Rational { num: 4, den: 3 },
            frame_rate: Rational { num: 24, den: 1 },
            ..phone()
        };
        assert_eq!(copy_eligibility(&settings, &anamorphic).mismatches.len(), 2);
    }

    #[test]
    fn equal_rates_are_equal_however_they_are_written() {
        let ntsc = SequenceSettings {
            frame_rate: Rational {
                num: 30_000,
                den: 1001,
            },
            ..SequenceSettings::default()
        };
        let source = StreamGeometry {
            width: 1920,
            height: 1080,
            frame_rate: Rational {
                num: 60_000,
                den: 2002,
            },
            ..phone()
        };
        assert!(copy_eligibility(&ntsc, &source).eligible);
    }

    #[test]
    fn a_variable_rate_or_hdr_source_copies_and_says_how() {
        let source = StreamGeometry {
            variable_frame_rate: true,
            hdr: true,
            ..phone()
        };
        let settings = SequenceSettings::matching(&source).expect("valid");
        let eligibility = copy_eligibility(&settings, &source);
        assert!(eligibility.eligible);
        assert_eq!(
            eligibility.notes,
            vec![CopyNote::VariableFrameRate, CopyNote::Hdr]
        );
    }

    #[test]
    fn a_setting_no_file_can_carry_is_refused() {
        let odd = StreamGeometry {
            width: 1081,
            ..phone()
        };
        assert_eq!(
            SequenceSettings::matching(&odd),
            Err(SettingsError::Size {
                width: 1081,
                height: 1920
            })
        );
        for (num, den) in [(0, 1), (1000, 1), (1, 2), (30, 0)] {
            let settings = SequenceSettings {
                frame_rate: Rational { num, den },
                ..SequenceSettings::default()
            };
            assert!(
                matches!(settings.validate(), Err(SettingsError::FrameRate { .. })),
                "{num}/{den}"
            );
        }
        let huge = SequenceSettings {
            width: 16_384,
            ..SequenceSettings::default()
        };
        assert!(huge.validate().is_err());
        assert!(SequenceSettings::default().validate().is_ok());
    }
}
