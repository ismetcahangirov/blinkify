//! Monitoring: what the editor hears, as opposed to what is exported (#31).
//!
//! The monitor volume and a track's monitor mute or solo change what comes out
//! of the speaker, and nothing else. They are kept apart from clip gain — which
//! is an operation in the edit graph and is written to the file — in their own
//! types, because conflating the two is how an editor comes to export at its
//! monitoring level: a serious failure nobody notices until the file is out.
//!
//! [`MonitorVolume`] cannot become a number anything else can use; the only
//! thing it does is scale the output buffer as the sink takes each sample.
//!
//! ```compile_fail
//! # use blinkify_engine::playback::MonitorVolume;
//! // There is no way to read a monitor volume back as a gain.
//! let gain: f32 = MonitorVolume::new(0.5).into();
//! ```

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::plan::TrackId;

/// The level the editor listens at, from silent (0) to unity (1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorVolume(f32);

impl MonitorVolume {
    #[must_use]
    pub fn new(level: f32) -> Self {
        Self(if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            1.0
        })
    }

    /// The gain the output buffer applies: the volume, or silence when muted.
    pub(crate) fn output_gain(self, muted: bool) -> f32 {
        if muted { 0.0 } else { self.0 }
    }

    fn level(self) -> f64 {
        f64::from(self.0)
    }
}

/// A monitoring change.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[ts(export)]
pub enum MonitorCommand {
    /// Listen at `level`, 0 to 1.
    Volume {
        level: f64,
    },
    Mute {
        muted: bool,
    },
    /// Solo a track: while any track is soloed, only soloed tracks are heard.
    Solo {
        track: TrackId,
        on: bool,
    },
    /// Silence one track in the monitor.
    MuteTrack {
        track: TrackId,
        on: bool,
    },
    /// Put the meter's clip indication out.
    ResetClip,
}

/// Where monitoring stands, as the renderer shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorStatus {
    pub volume: f64,
    pub muted: bool,
    pub soloed: Vec<TrackId>,
    pub muted_tracks: Vec<TrackId>,
}

/// Every monitoring setting.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MonitorSettings {
    pub volume: MonitorVolume,
    pub muted: bool,
    pub soloed: BTreeSet<TrackId>,
    pub muted_tracks: BTreeSet<TrackId>,
}

impl Default for MonitorSettings {
    fn default() -> Self {
        Self {
            volume: MonitorVolume::new(1.0),
            muted: false,
            soloed: BTreeSet::new(),
            muted_tracks: BTreeSet::new(),
        }
    }
}

impl MonitorSettings {
    /// Whether `track` reaches the monitor: not muted, and soloed if anything
    /// is.
    pub(crate) fn audible(&self, track: TrackId) -> bool {
        !self.muted_tracks.contains(&track)
            && (self.soloed.is_empty() || self.soloed.contains(&track))
    }

    pub(crate) fn apply(&mut self, command: MonitorCommand) {
        match command {
            #[allow(clippy::cast_possible_truncation)]
            MonitorCommand::Volume { level } => self.volume = MonitorVolume::new(level as f32),
            MonitorCommand::Mute { muted } => self.muted = muted,
            MonitorCommand::Solo { track, on } => toggle(&mut self.soloed, track, on),
            MonitorCommand::MuteTrack { track, on } => toggle(&mut self.muted_tracks, track, on),
            MonitorCommand::ResetClip => {}
        }
    }

    pub(crate) fn status(&self) -> MonitorStatus {
        MonitorStatus {
            volume: self.volume.level(),
            muted: self.muted,
            soloed: self.soloed.iter().copied().collect(),
            muted_tracks: self.muted_tracks.iter().copied().collect(),
        }
    }
}

fn toggle(set: &mut BTreeSet<TrackId>, track: TrackId, on: bool) {
    if on {
        set.insert(track);
    } else {
        set.remove(&track);
    }
}

/// Where the audio filter chain of Epic #7 attaches: every segment's samples
/// pass through it after decoding and before they are mixed, so the meter
/// measures what gain, denoise and normalisation produced. Until Epic #7
/// fills it, it is [`PassThrough`], and nothing here assumes otherwise.
pub trait AudioInsert: Send + Sync {
    /// Process interleaved stereo `samples` of segment `segment` of `track`,
    /// in place, at `sample_rate`.
    fn process(&self, track: TrackId, segment: usize, samples: &mut [f32], sample_rate: u32);
}

/// The insertion point with nothing attached.
#[derive(Debug, Clone, Copy, Default)]
pub struct PassThrough;

impl AudioInsert for PassThrough {
    fn process(&self, _: TrackId, _: usize, _: &mut [f32], _: u32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solo_and_mute_decide_which_tracks_are_heard() {
        let mut settings = MonitorSettings::default();
        assert!(settings.audible(0) && settings.audible(1));
        settings.apply(MonitorCommand::MuteTrack { track: 1, on: true });
        assert!(settings.audible(0) && !settings.audible(1));
        settings.apply(MonitorCommand::Solo { track: 1, on: true });
        // Soloed but muted: mute wins, and the solo silences the rest.
        assert!(!settings.audible(0) && !settings.audible(1));
        settings.apply(MonitorCommand::MuteTrack {
            track: 1,
            on: false,
        });
        assert!(!settings.audible(0) && settings.audible(1));
        settings.apply(MonitorCommand::Solo { track: 0, on: true });
        assert!(settings.audible(0) && settings.audible(1));
    }

    #[test]
    fn the_volume_is_clamped_and_mute_silences_it() {
        assert!((MonitorVolume::new(3.0).output_gain(false) - 1.0).abs() < f32::EPSILON);
        assert!(MonitorVolume::new(-1.0).output_gain(false).abs() < f32::EPSILON);
        assert!(MonitorVolume::new(0.5).output_gain(true).abs() < f32::EPSILON);
        assert!((MonitorVolume::new(f32::NAN).output_gain(false) - 1.0).abs() < f32::EPSILON);
    }
}
