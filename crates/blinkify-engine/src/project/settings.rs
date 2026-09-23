//! Sequence settings: the shape of the timeline.
//!
//! #57 owns what these mean — how they are chosen from the first clip, and
//! the rule that binds them to copy eligibility. This is the one place they
//! are defined; the edit graph refers to them and never repeats a field.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::probe::Rational;

/// How the sequence treats colour. Only SDR until #57 says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum ColourPolicy {
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
}

impl Default for SequenceSettings {
    /// 1080p30, square pixels — until #57 derives the settings from the
    /// first clip.
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
