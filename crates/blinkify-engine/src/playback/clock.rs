//! The playback clock: which timeline position is being heard now.
//!
//! The clock is derived from audio, never from a timer or from frame
//! delivery (#28). The feeder writes samples to the output buffer and records
//! an [`Anchor`] wherever the mapping from samples to timeline jumps — the
//! start, a seek, a loop, a speed change. The position is then the anchor's
//! timeline position plus the frames heard since it, at the anchor's speed.
//! Between device callbacks the output buffer interpolates with the monotonic
//! timer, so the position moves smoothly; it never runs past the audio that
//! has actually been consumed, so a starved device stops the clock rather than
//! letting video run ahead of sound.
//!
//! Every discontinuity also starts a new *generation*. Video decodes are
//! tagged with the generation they serve, so a decode prepared for the far
//! side of a loop is not mistaken for the one playing now.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::plan::ProgramTime;

/// Where the mapping from output frames to the timeline changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Anchor {
    /// The output-buffer frame the mapping starts at.
    pub(crate) frame: u64,
    /// The timeline position of that frame.
    pub(crate) at: ProgramTime,
    /// Timeline microseconds per output second, over one million.
    pub(crate) speed: f64,
    pub(crate) generation: u64,
}

#[derive(Debug)]
enum Mode {
    Paused { at: ProgramTime, generation: u64 },
    Running,
}

#[derive(Debug)]
struct State {
    mode: Mode,
    anchors: VecDeque<Anchor>,
    /// The last position reported, per generation, so the clock never moves
    /// backwards within one.
    last: (u64, ProgramTime),
}

/// The playback clock.
#[derive(Debug)]
pub(crate) struct PlaybackClock {
    state: Mutex<State>,
}

impl PlaybackClock {
    /// A clock paused at `at`.
    #[must_use]
    pub(crate) fn new(at: ProgramTime, generation: u64) -> Self {
        Self {
            state: Mutex::new(State {
                mode: Mode::Paused { at, generation },
                anchors: VecDeque::new(),
                last: (generation, at),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stop at `at`, in `generation`, dropping every anchor.
    pub(crate) fn pause(&self, at: ProgramTime, generation: u64) {
        let mut state = self.lock();
        state.mode = Mode::Paused { at, generation };
        state.anchors.clear();
        state.last = (generation, at);
    }

    /// Run from `anchor` onwards.
    pub(crate) fn run(&self, anchor: Anchor) {
        let mut state = self.lock();
        state.mode = Mode::Running;
        state.anchors.clear();
        state.anchors.push_back(anchor);
        state.last = (anchor.generation, anchor.at);
    }

    /// Add a discontinuity, at an output frame later than every earlier one.
    pub(crate) fn anchor(&self, anchor: Anchor) {
        self.lock().anchors.push_back(anchor);
    }

    /// The generation and timeline position heard now, given how many output
    /// frames have been heard and the output rate.
    pub(crate) fn position(&self, heard: f64, sample_rate: u32) -> (u64, ProgramTime) {
        let mut state = self.lock();
        let (generation, at) = match state.mode {
            Mode::Paused { at, generation } => return (generation, at),
            Mode::Running => {
                // The newest anchor already reached; earlier ones are spent.
                #[allow(clippy::cast_precision_loss)]
                while state.anchors.len() > 1
                    && state
                        .anchors
                        .get(1)
                        .is_some_and(|next| next.frame as f64 <= heard)
                {
                    state.anchors.pop_front();
                }
                let Some(anchor) = state.anchors.front().copied() else {
                    return state.last;
                };
                #[allow(clippy::cast_precision_loss)]
                let since = (heard - anchor.frame as f64).max(0.0);
                let micros = since / f64::from(sample_rate.max(1)) * 1_000_000.0 * anchor.speed;
                // Saturating, and bounded by any real playback's length.
                #[allow(clippy::cast_possible_truncation)]
                let micros = micros.floor() as i64;
                (anchor.generation, anchor.at.saturating_add(micros))
            }
        };
        let at = if state.last.0 == generation {
            at.max(state.last.1)
        } else {
            at
        };
        state.last = (generation, at);
        (generation, at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(frame: u64, at: ProgramTime, speed: f64, generation: u64) -> Anchor {
        Anchor {
            frame,
            at,
            speed,
            generation,
        }
    }

    #[test]
    fn a_paused_clock_stays_where_it_was_put() {
        let clock = PlaybackClock::new(5_000, 1);
        assert_eq!(clock.position(1e9, 48_000), (1, 5_000));
    }

    #[test]
    fn a_running_clock_follows_the_frames_heard_at_its_speed() {
        let clock = PlaybackClock::new(0, 0);
        clock.run(anchor(1_000, 2_000_000, 1.0, 1));
        // Before the anchor's first frame is heard: pre-roll, at the anchor.
        assert_eq!(clock.position(500.0, 1_000), (1, 2_000_000));
        assert_eq!(clock.position(1_500.0, 1_000), (1, 2_500_000));
        clock.run(anchor(0, 0, 2.0, 2));
        assert_eq!(clock.position(1_000.0, 1_000), (2, 2_000_000));
        clock.run(anchor(0, 0, 0.25, 3));
        assert_eq!(clock.position(1_000.0, 1_000), (3, 250_000));
    }

    #[test]
    fn a_loop_anchor_takes_over_once_its_frame_is_heard() {
        let clock = PlaybackClock::new(0, 0);
        clock.run(anchor(0, 9_000_000, 1.0, 1));
        clock.anchor(anchor(1_000, 0, 1.0, 2));
        assert_eq!(clock.position(999.0, 1_000), (1, 9_999_000));
        assert_eq!(clock.position(1_250.0, 1_000), (2, 250_000));
    }

    #[test]
    fn the_clock_never_moves_backwards_within_a_generation() {
        let clock = PlaybackClock::new(0, 0);
        clock.run(anchor(0, 0, 1.0, 1));
        assert_eq!(clock.position(2_000.0, 1_000), (1, 2_000_000));
        assert_eq!(clock.position(1_000.0, 1_000), (1, 2_000_000));
    }
}
