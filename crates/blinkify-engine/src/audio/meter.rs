//! The level meter: sample peak per channel, EBU R128 short-term loudness,
//! and a clip indication that stays lit until it is reset (#31).
//!
//! Loudness follows ITU-R BS.1770-4: each channel is K-weighted — a high
//! shelf for the head, then a high-pass — squared, averaged over 100 ms
//! blocks, and the short-term value is the mean of the last 30 blocks (3 s),
//! `−0.691 + 10·log10(Σ channel means)`, ungated, as EBU Tech 3341 defines it.
//! The coefficients are computed for the output rate, not tabulated for 48
//! kHz, so a 44.1 kHz device measures the same.
//!
//! The meter measures what is being *played*, at the moment the sink takes
//! it: after the filter-chain insertion point, before the monitor volume. It
//! is a statement about the programme, and the monitor volume is not part of
//! the programme.

use std::collections::VecDeque;
use std::f64::consts::PI;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::CHANNELS;

/// Blocks in the short-term window: 30 of 100 ms.
const SHORT_TERM_BLOCKS: usize = 30;

/// Blocks the peak is held over: 300 ms.
const PEAK_BLOCKS: usize = 3;

/// A sample at or above this magnitude has clipped. Not exactly 1.0: the
/// largest 16-bit sample is 32767/32768, and a full-scale integer source must
/// light the indication too — this is −0.0009 dBFS.
const CLIP: f32 = 0.999_9;

/// What the meter shows.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorLevels {
    /// Sample peak of each channel over the last 300 ms, in dBFS. `None` for
    /// silence.
    pub peak_db: [Option<f64>; 2],
    /// EBU R128 short-term loudness, in LUFS, once 3 s have been heard.
    pub short_term_lufs: Option<f64>,
    /// A sample reached 0 dBFS since the indication was last reset.
    pub clipped: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
}

impl Biquad {
    pub(crate) fn run(&self, state: &mut [f64; 4], x: f64) -> f64 {
        let [x1, x2, y1, y2] = *state;
        let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[1] * y1 - self.a[2] * y2;
        *state = [x, x1, y, y1];
        y
    }
}

/// The two K-weighting stages for `rate`.
pub(crate) fn k_weighting(rate: f64) -> (Biquad, Biquad) {
    // Stage 1: the high shelf that models the head.
    let f0 = 1_681.974_450_955_533;
    let gain_db = 3.999_843_853_973_347;
    let q = 0.707_175_236_955_419_6;
    let k = (PI * f0 / rate).tan();
    let vh = 10_f64.powf(gain_db / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [
            (vh + vb * k / q + k * k) / a0,
            2.0 * (k * k - vh) / a0,
            (vh - vb * k / q + k * k) / a0,
        ],
        a: [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    // Stage 2: the RLB high-pass.
    let f0 = 38.135_470_876_024_44;
    let q = 0.500_327_037_323_877_3;
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let high_pass = Biquad {
        b: [1.0, -2.0, 1.0],
        a: [1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
    };
    (shelf, high_pass)
}

/// A running meter for interleaved stereo at one rate.
#[derive(Debug)]
pub(crate) struct Meter {
    shelf: Biquad,
    high_pass: Biquad,
    state: [[[f64; 4]; 2]; CHANNELS],
    block_frames: usize,
    /// The block being filled: squared K-weighted sums and peaks per channel.
    filled: usize,
    energy: [f64; CHANNELS],
    peak: [f32; CHANNELS],
    /// Completed blocks, newest last: mean squares and peaks.
    blocks: VecDeque<([f64; CHANNELS], [f32; CHANNELS])>,
    clipped: bool,
}

impl Meter {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let (shelf, high_pass) = k_weighting(f64::from(sample_rate.max(1)));
        Self {
            shelf,
            high_pass,
            state: [[[0.0; 4]; 2]; CHANNELS],
            block_frames: usize::try_from(sample_rate.max(10))
                .unwrap_or(48_000)
                .div_euclid(10),
            filled: 0,
            energy: [0.0; CHANNELS],
            peak: [0.0; CHANNELS],
            blocks: VecDeque::with_capacity(SHORT_TERM_BLOCKS),
            clipped: false,
        }
    }

    /// Measure one interleaved stereo frame.
    pub(crate) fn frame(&mut self, left: f32, right: f32) {
        for (channel, sample) in [left, right].into_iter().enumerate() {
            let magnitude = sample.abs();
            if magnitude >= CLIP {
                self.clipped = true;
            }
            let (Some(state), Some(peak), Some(energy)) = (
                self.state.get_mut(channel),
                self.peak.get_mut(channel),
                self.energy.get_mut(channel),
            ) else {
                continue;
            };
            *peak = peak.max(magnitude);
            let [shelf_state, pass_state] = state;
            let shelved = self.shelf.run(shelf_state, f64::from(sample));
            let weighted = self.high_pass.run(pass_state, shelved);
            *energy += weighted * weighted;
        }
        self.filled += 1;
        if self.filled == self.block_frames {
            #[allow(clippy::cast_precision_loss)]
            let frames = self.block_frames as f64;
            let means = self.energy.map(|sum| sum / frames);
            if self.blocks.len() == SHORT_TERM_BLOCKS {
                self.blocks.pop_front();
            }
            self.blocks.push_back((means, self.peak));
            self.filled = 0;
            self.energy = [0.0; CHANNELS];
            self.peak = [0.0; CHANNELS];
        }
    }

    /// Forget what was heard — a pause, a seek — but not that it clipped.
    pub(crate) fn restart(&mut self) {
        self.state = [[[0.0; 4]; 2]; CHANNELS];
        self.filled = 0;
        self.energy = [0.0; CHANNELS];
        self.peak = [0.0; CHANNELS];
        self.blocks.clear();
    }

    pub(crate) fn reset_clip(&mut self) {
        self.clipped = false;
    }

    pub(crate) fn levels(&self) -> MonitorLevels {
        let recent = self.blocks.iter().rev().take(PEAK_BLOCKS);
        let mut peak = [0.0_f32; CHANNELS];
        for (_, block_peak) in recent {
            for (held, value) in peak.iter_mut().zip(block_peak) {
                *held = held.max(*value);
            }
        }
        let to_db = |value: f32| (value > 0.0).then(|| 20.0 * f64::from(value).log10());
        let short_term = (self.blocks.len() == SHORT_TERM_BLOCKS).then(|| {
            #[allow(clippy::cast_precision_loss)]
            let count = self.blocks.len() as f64;
            let sum: f64 = self
                .blocks
                .iter()
                .map(|(means, _)| means.iter().sum::<f64>())
                .sum::<f64>()
                / count;
            -0.691 + 10.0 * sum.max(1e-12).log10()
        });
        MonitorLevels {
            peak_db: [
                peak.first().copied().and_then(to_db),
                peak.get(1).copied().and_then(to_db),
            ],
            short_term_lufs: short_term,
            clipped: self.clipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(meter: &mut Meter, rate: u32, frequency: f64, amplitude: f32, seconds: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (f64::from(rate) * seconds) as usize;
        for n in 0..frames {
            #[allow(clippy::cast_precision_loss)]
            let t = n as f64 / f64::from(rate);
            #[allow(clippy::cast_possible_truncation)]
            let sample = (f64::from(amplitude) * (2.0 * PI * frequency * t).sin()) as f32;
            meter.frame(sample, sample);
        }
    }

    #[test]
    fn a_1khz_tone_at_minus_20_dbfs_measures_as_bs_1770_says() {
        // 1 kHz is where K-weighting is (nearly) flat: +0.69 dB. A sine's
        // mean square is half its peak squared (−3.01 dB), and two channels
        // add +3.01 dB, so the loudness is −20 + 0.69 − 0.691 ≈ −20.0 LUFS.
        for rate in [44_100, 48_000] {
            let mut meter = Meter::new(rate);
            sine(&mut meter, rate, 1000.0, 0.1, 4.0);
            let levels = meter.levels();
            let lufs = levels.short_term_lufs.unwrap_or(f64::NAN);
            assert!((lufs - -20.0).abs() < 0.1, "{rate} Hz: {lufs} LUFS");
            let peak = levels.peak_db[0].unwrap_or(f64::NAN);
            assert!((peak - -20.0).abs() < 0.05, "{peak} dBFS");
            assert!(!levels.clipped);
        }
    }

    #[test]
    fn a_clip_stays_lit_until_it_is_reset() {
        let mut meter = Meter::new(48_000);
        sine(&mut meter, 48_000, 440.0, 1.0, 0.1);
        sine(&mut meter, 48_000, 440.0, 0.01, 3.5);
        assert!(meter.levels().clipped, "held after the signal went quiet");
        meter.restart();
        assert!(meter.levels().clipped, "held through a seek");
        meter.reset_clip();
        assert!(!meter.levels().clipped);
    }

    #[test]
    fn short_term_loudness_waits_for_three_seconds_and_silence_has_no_peak() {
        let mut meter = Meter::new(48_000);
        sine(&mut meter, 48_000, 1000.0, 0.0, 2.9);
        let levels = meter.levels();
        assert_eq!(levels.short_term_lufs, None);
        assert_eq!(levels.peak_db, [None, None]);
    }
}
