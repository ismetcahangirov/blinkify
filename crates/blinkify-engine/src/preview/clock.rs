//! A monotonic media clock driven by the system's monotonic timer.
//!
//! Media time advances with [`Instant`], never by accumulating per-frame or
//! per-tick increments, so it cannot drift from rounding: its position is
//! always the anchor plus the elapsed time since it was started.

use std::sync::{Mutex, PoisonError};
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
struct Anchor {
    at: Instant,
    seconds: f64,
}

/// A clock that runs in real time from wherever it was started.
#[derive(Debug, Default)]
pub struct WallClock {
    anchor: Mutex<Option<Anchor>>,
}

impl WallClock {
    /// Run from `seconds` of media time, now.
    pub fn start(&self, seconds: f64) {
        *self.anchor.lock().unwrap_or_else(PoisonError::into_inner) = Some(Anchor {
            at: Instant::now(),
            seconds,
        });
    }

    pub fn stop(&self) {
        *self.anchor.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The media time now, or `None` while stopped.
    #[must_use]
    pub fn position(&self) -> Option<f64> {
        self.anchor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|anchor| anchor.seconds + anchor.at.elapsed().as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_stopped_clock_has_no_position_and_a_started_one_advances() {
        let clock = WallClock::default();
        assert_eq!(clock.position(), None);
        clock.start(10.0);
        std::thread::sleep(Duration::from_millis(20));
        let position = clock.position().unwrap_or_default();
        assert!((10.02..11.0).contains(&position), "{position}");
        clock.stop();
        assert_eq!(clock.position(), None);
    }
}
