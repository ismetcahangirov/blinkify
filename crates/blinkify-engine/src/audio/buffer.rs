//! The buffer between the audio feeder and the sink, and the record of when
//! each sample is heard.
//!
//! This is where the playback clock comes from (#28). Audio is the reference
//! because it cannot be stretched without artefacts: a video frame can be
//! dropped or held, a sample cannot. So the question "where is playback now?"
//! is answered by "which sample is coming out of the speaker now?", and that
//! is what [`OutputBuffer::heard`] measures — from the frames the sink has
//! consumed and the instant the sink says the first of them will play, which
//! accounts for the device's own latency.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::CHANNELS;

#[derive(Debug, Clone, Copy)]
struct Timing {
    /// The index of a frame the sink consumed…
    frame: u64,
    /// …and when it is heard.
    at: Instant,
}

#[derive(Debug, Default)]
struct State {
    /// Interleaved stereo samples, oldest first.
    queue: VecDeque<f32>,
    /// Frames the sink has taken from the queue, ever.
    consumed: u64,
    timing: Option<Timing>,
}

/// Interleaved stereo `f32` samples on their way to a sink, at one sample
/// rate, indexed by frame from the start of playback.
#[derive(Debug)]
pub struct OutputBuffer {
    sample_rate: u32,
    state: Mutex<State>,
    drained: Condvar,
}

impl OutputBuffer {
    #[must_use]
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            state: Mutex::new(State::default()),
            drained: Condvar::new(),
        }
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Frames written but not yet taken by the sink.
    #[must_use]
    pub fn queued_frames(&self) -> u64 {
        u64::try_from(self.lock().queue.len().div_euclid(CHANNELS)).unwrap_or(u64::MAX)
    }

    /// The index the next frame written will have.
    #[must_use]
    pub fn write_index(&self) -> u64 {
        let state = self.lock();
        state.consumed + u64::try_from(state.queue.len().div_euclid(CHANNELS)).unwrap_or(0)
    }

    /// Append interleaved stereo samples. Returns the index of the first
    /// frame written.
    pub fn push(&self, samples: &[f32]) -> u64 {
        let mut state = self.lock();
        let first =
            state.consumed + u64::try_from(state.queue.len().div_euclid(CHANNELS)).unwrap_or(0);
        state.queue.extend(samples.iter().copied());
        first
    }

    /// Drop everything not yet taken by the sink — a pause or a seek. The
    /// next frame written is then the next the sink will take. Returns its
    /// index.
    pub fn clear(&self) -> u64 {
        let mut state = self.lock();
        state.queue.clear();
        // What was heard before the clear says nothing about when the next
        // written frame will be heard.
        state.timing = None;
        let next = state.consumed;
        drop(state);
        self.drained.notify_all();
        next
    }

    /// Fill `out` — `channels` interleaved samples per frame — for a sink,
    /// with the first frame of `out` to be heard at `plays_at`. What the queue
    /// cannot supply is silence and is not counted as consumed: an underrun
    /// does not move the clock.
    pub fn pull(&self, out: &mut [f32], channels: usize, plays_at: Instant) {
        let channels = channels.max(1);
        let mut state = self.lock();
        let first = state.consumed;
        let mut taken = 0_u64;
        for frame in out.chunks_mut(channels) {
            let (Some(left), Some(right)) = (state.queue.pop_front(), state.queue.pop_front())
            else {
                frame.fill(0.0);
                continue;
            };
            taken += 1;
            match frame {
                [mono] => *mono = f32::midpoint(left, right),
                [l, r, rest @ ..] => {
                    *l = left;
                    *r = right;
                    rest.fill(0.0);
                }
                [] => {}
            }
        }
        state.consumed += taken;
        if taken > 0 {
            state.timing = Some(Timing {
                frame: first,
                at: plays_at,
            });
        }
        drop(state);
        self.drained.notify_all();
    }

    /// How many frames have been heard by `now`: the last timing the sink
    /// reported, advanced at the sample rate, never past what the sink has
    /// actually consumed.
    #[must_use]
    pub fn heard(&self, now: Instant) -> f64 {
        let state = self.lock();
        #[allow(clippy::cast_precision_loss)]
        let consumed = state.consumed as f64;
        let Some(timing) = state.timing else {
            return 0.0;
        };
        #[allow(clippy::cast_precision_loss)]
        let base = timing.frame as f64;
        let rate = f64::from(self.sample_rate);
        let heard = if now >= timing.at {
            base + now.duration_since(timing.at).as_secs_f64() * rate
        } else {
            base - timing.at.duration_since(now).as_secs_f64() * rate
        };
        heard.clamp(0.0, consumed)
    }

    /// Frames the sink has taken, ever.
    #[must_use]
    pub fn consumed(&self) -> u64 {
        self.lock().consumed
    }

    /// Wait until the queue holds fewer than `frames`, or `timeout` passes.
    pub fn wait_below(&self, frames: u64, timeout: Duration) {
        let state = self.lock();
        let limit = usize::try_from(frames)
            .unwrap_or(usize::MAX)
            .saturating_mul(CHANNELS);
        if state.queue.len() < limit {
            return;
        }
        let _ = self
            .drained
            .wait_timeout(state, timeout)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_indexed_across_pushes_pulls_and_clears() {
        let buffer = OutputBuffer::new(48_000);
        assert_eq!(buffer.push(&[0.1, 0.2, 0.3, 0.4]), 0);
        assert_eq!(buffer.write_index(), 2);
        let mut out = [0.0; 2];
        buffer.pull(&mut out, 2, Instant::now());
        assert_eq!(out, [0.1, 0.2]);
        assert_eq!(buffer.consumed(), 1);
        assert_eq!(buffer.clear(), 1, "the unplayed frame is dropped");
        assert_eq!(buffer.push(&[0.5, 0.6]), 1);
    }

    #[test]
    fn an_underrun_is_silence_and_does_not_count() {
        let buffer = OutputBuffer::new(48_000);
        buffer.push(&[1.0, -1.0]);
        let mut out = [9.0; 6];
        buffer.pull(&mut out, 2, Instant::now());
        assert_eq!(out, [1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(buffer.consumed(), 1);
    }

    #[test]
    fn stereo_maps_onto_any_channel_count() {
        let buffer = OutputBuffer::new(48_000);
        buffer.push(&[0.4, 0.2, 0.4, 0.2]);
        let mut mono = [0.0; 1];
        buffer.pull(&mut mono, 1, Instant::now());
        assert!((mono[0] - 0.3).abs() < 1e-6);
        let mut surround = [9.0; 6];
        buffer.pull(&mut surround, 6, Instant::now());
        assert_eq!(surround, [0.4, 0.2, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn what_is_heard_advances_from_the_sinks_timing_and_never_passes_what_was_consumed() {
        let buffer = OutputBuffer::new(1000);
        buffer.push(&[0.0; 2000]);
        let at = Instant::now() + Duration::from_millis(50);
        let mut out = vec![0.0; 200];
        buffer.pull(&mut out, 2, at);
        // Nothing is heard before the device latency has passed…
        assert!(buffer.heard(at.checked_sub(Duration::from_millis(10)).unwrap_or(at)) < 1.0);
        // …then a millisecond per frame at 1 kHz…
        let heard = buffer.heard(at + Duration::from_millis(40));
        assert!((heard - 40.0).abs() < 0.001, "{heard}");
        // …but never more than the sink took.
        assert!((buffer.heard(at + Duration::from_secs(5)) - 100.0).abs() < f64::EPSILON);
    }
}
