//! Measuring a programme's loudness (Epic #7): integrated loudness and
//! loudness range as EBU R128 defines them, and true peak as ITU-R BS.1770-4
//! does — the numbers a gain suggestion, a limiter reading and loudness
//! normalisation are all worked out from.
//!
//! - **Integrated loudness** (BS.1770-4): K-weighted mean square over 400 ms
//!   blocks overlapping by 75 %, gated absolutely at −70 LUFS and then
//!   relatively at 10 LU below the loudness of the blocks that passed.
//! - **Loudness range** (EBU Tech 3342): the spread between the 10th and 95th
//!   percentiles of the short-term (3 s) loudness, taken every 100 ms, gated
//!   at −70 LUFS and at 20 LU below.
//! - **True peak** (BS.1770-4 Annex 2): the largest magnitude of the signal
//!   reconstructed at four times the rate, where the peaks between samples —
//!   which a lossy encode or a phone's resampler will find — are samples.
//!
//! The K-weighting is the level meter's, computed for the actual rate. The
//! whole programme is measured in one pass and nothing is kept but one number
//! per 100 ms, so an hour of sound costs 36 000 of them.
//!
//! [`measure`] runs a stretch of a source through the audio chain up to a
//! point, in the sidecar, and measures what comes out: the same filters the
//! preview and the export run, so a measurement describes the sound they
//! will play.

use std::f64::consts::PI;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use sha2::{Digest, Sha256};

use super::meter::{Biquad, k_weighting};
use crate::cache::{Cache, ContentKey};
use crate::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand,
};

/// Blocks per gating block (400 ms) and per short-term window (3 s).
const GATING_BLOCKS: usize = 4;
const SHORT_TERM_BLOCKS: usize = 30;

/// The absolute gate, in LUFS, and the relative gates, in LU.
const ABSOLUTE_GATE: f64 = -70.0;
const RELATIVE_GATE: f64 = -10.0;
const RANGE_GATE: f64 = -20.0;

/// Interpolation taps either side of the point reconstructed.
const HALF_TAPS: usize = 6;

/// How far before the stretch the demuxer seeks, as the preview's decoder.
const SEEK_MARGIN_SECONDS: f64 = 0.5;

/// Changes when the way a stretch is measured changes, so an answer cached
/// by an earlier build is not reused.
const CACHE_FORMAT: u32 = 1;

/// What a programme measures.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Loudness {
    /// Integrated loudness, in LUFS. `None` when nothing passed the gate:
    /// silence, or sound too short to measure.
    pub integrated_lufs: Option<f64>,
    /// Loudness range, in LU. `None` under the same conditions, or when the
    /// programme is shorter than one short-term window.
    pub range_lu: Option<f64>,
    /// True peak, in dBTP. `None` for digital silence.
    pub true_peak_dbtp: Option<f64>,
    /// How long was measured, in seconds.
    pub seconds: f64,
}

/// A running measurement of interleaved samples at one rate.
#[derive(Debug)]
pub struct LoudnessMeter {
    channels: usize,
    rate: u32,
    shelf: Biquad,
    high_pass: Biquad,
    state: Vec<[[f64; 4]; 2]>,
    block_frames: usize,
    filled: usize,
    energy: f64,
    /// Channel-summed mean squares of each completed 100 ms block.
    blocks: Vec<f64>,
    /// The last few samples of each channel, for the true-peak interpolator.
    history: Vec<[f32; 2 * HALF_TAPS]>,
    kernel: [[f64; 2 * HALF_TAPS]; 3],
    peak: f64,
    frames: u64,
}

impl LoudnessMeter {
    /// A meter for `channels` interleaved channels at `rate`. Each channel
    /// weighs 1, as BS.1770-4 gives left, right and centre; a source with
    /// surround channels is measured as its stereo downmix.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let (shelf, high_pass) = k_weighting(f64::from(rate.max(1)));
        let channels = channels.max(1);
        Self {
            channels,
            rate: rate.max(10),
            shelf,
            high_pass,
            state: vec![[[0.0; 4]; 2]; channels],
            block_frames: usize::try_from(rate.max(10).div_euclid(10)).unwrap_or(4_800),
            filled: 0,
            energy: 0.0,
            blocks: Vec::new(),
            history: vec![[0.0; 2 * HALF_TAPS]; channels],
            kernel: interpolation_kernel(),
            peak: 0.0,
            frames: 0,
        }
    }

    /// Measure `samples`, interleaved. A trailing partial frame is ignored.
    pub fn push(&mut self, samples: &[f32]) {
        for frame in samples.chunks_exact(self.channels) {
            for (channel, &sample) in frame.iter().enumerate() {
                self.true_peak(channel, sample);
                let Some(state) = self.state.get_mut(channel) else {
                    continue;
                };
                let [shelf_state, pass_state] = state;
                let shelved = self.shelf.run(shelf_state, f64::from(sample));
                let weighted = self.high_pass.run(pass_state, shelved);
                self.energy += weighted * weighted;
            }
            self.filled += 1;
            self.frames += 1;
            if self.filled == self.block_frames {
                #[allow(clippy::cast_precision_loss)]
                self.blocks.push(self.energy / self.block_frames as f64);
                self.filled = 0;
                self.energy = 0.0;
            }
        }
    }

    /// Feed one sample to channel `channel`'s interpolator: the sample
    /// itself and the three points after it, reconstructed.
    fn true_peak(&mut self, channel: usize, sample: f32) {
        let Some(history) = self.history.get_mut(channel) else {
            return;
        };
        history.rotate_left(1);
        if let Some(last) = history.last_mut() {
            *last = sample;
        }
        // The point reconstructed lies between history[HALF_TAPS - 1] and
        // history[HALF_TAPS]; each phase is a fixed fraction of the way.
        let mut peak = history
            .get(HALF_TAPS - 1)
            .map_or(0.0, |sample| f64::from(sample.abs()));
        for phase in &self.kernel {
            let value: f64 = phase
                .iter()
                .zip(history.iter())
                .map(|(tap, &x)| tap * f64::from(x))
                .sum();
            peak = peak.max(value.abs());
        }
        self.peak = self.peak.max(peak);
    }

    /// The measurement of everything pushed.
    #[must_use]
    pub fn finish(&self) -> Loudness {
        #[allow(clippy::cast_precision_loss)]
        let seconds = self.frames as f64 / f64::from(self.rate);
        Loudness {
            integrated_lufs: integrated(&self.blocks),
            range_lu: range(&self.blocks),
            true_peak_dbtp: (self.peak > 0.0).then(|| 20.0 * self.peak.log10()),
            seconds,
        }
    }
}

/// Loudness, in LUFS, of a channel-summed mean square.
fn lufs(mean_square: f64) -> f64 {
    -0.691 + 10.0 * mean_square.max(f64::MIN_POSITIVE).log10()
}

/// Mean squares of overlapping windows of `span` blocks, one per block.
fn windows(blocks: &[f64], span: usize) -> Vec<f64> {
    #[allow(clippy::cast_precision_loss)]
    blocks
        .windows(span)
        .map(|window| window.iter().sum::<f64>() / span as f64)
        .collect()
}

fn mean(values: &[f64]) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn integrated(blocks: &[f64]) -> Option<f64> {
    let gating = windows(blocks, GATING_BLOCKS);
    let loud: Vec<f64> = gating
        .into_iter()
        .filter(|&z| lufs(z) > ABSOLUTE_GATE)
        .collect();
    let threshold = lufs(mean(&loud)?) + RELATIVE_GATE;
    let kept: Vec<f64> = loud.into_iter().filter(|&z| lufs(z) > threshold).collect();
    mean(&kept).map(lufs)
}

fn range(blocks: &[f64]) -> Option<f64> {
    let short_term = windows(blocks, SHORT_TERM_BLOCKS);
    let loud: Vec<f64> = short_term
        .into_iter()
        .filter(|&z| lufs(z) > ABSOLUTE_GATE)
        .collect();
    let threshold = lufs(mean(&loud)?) + RANGE_GATE;
    let mut kept: Vec<f64> = loud
        .into_iter()
        .map(lufs)
        .filter(|&l| l > threshold)
        .collect();
    if kept.is_empty() {
        return None;
    }
    kept.sort_by(f64::total_cmp);
    Some(percentile(&kept, 0.95) - percentile(&kept, 0.10))
}

/// The value at `fraction` of the way through sorted `values`, nearest rank.
fn percentile(values: &[f64], fraction: f64) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values.get(index).copied().unwrap_or_default()
}

/// Windowed-sinc taps reconstructing the points a quarter, a half and
/// three quarters of the way between two samples, from `HALF_TAPS` samples
/// either side: the 48-tap, four-phase interpolator BS.1770-4 Annex 2
/// describes, with a Kaiser window.
fn interpolation_kernel() -> [[f64; 2 * HALF_TAPS]; 3] {
    let mut kernel = [[0.0; 2 * HALF_TAPS]; 3];
    #[allow(clippy::cast_precision_loss)]
    let half = HALF_TAPS as f64;
    for (phase, taps) in kernel.iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let fraction = (phase + 1) as f64 / 4.0;
        let mut sum = 0.0;
        for (i, tap) in taps.iter_mut().enumerate() {
            // The distance from the point reconstructed to sample `i`.
            #[allow(clippy::cast_precision_loss)]
            let distance = (i as f64 - (half - 1.0)) - fraction;
            let sinc = if distance.abs() < 1e-12 {
                1.0
            } else {
                (PI * distance).sin() / (PI * distance)
            };
            let window = kaiser(distance / half, 5.0);
            *tap = sinc * window;
            sum += *tap;
        }
        // Unity gain at DC, so a constant is reconstructed as itself.
        for tap in taps.iter_mut() {
            *tap /= sum;
        }
    }
    kernel
}

/// The Kaiser window at `x` in −1..1.
fn kaiser(x: f64, beta: f64) -> f64 {
    if x.abs() >= 1.0 {
        return 0.0;
    }
    bessel_i0(beta * (1.0 - x * x).sqrt()) / bessel_i0(beta)
}

/// The zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..32_i32 {
        let k = f64::from(k);
        term *= (half / k) * (half / k);
        sum += term;
    }
    sum
}

/// A stretch of a source's sound to measure, through the chain up to a
/// point.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureRequest {
    pub source: PathBuf,
    /// The stream's absolute index in the file.
    pub stream: u32,
    /// The stretch, in the stream's own time, in seconds.
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// The rate the sound is measured at: the one its chain runs at.
    pub sample_rate: u32,
    /// 1 for a mono source, otherwise 2: a surround source is measured as its
    /// stereo downmix.
    pub channels: u32,
    /// The part of the audio chain before the point measured, from
    /// [`chain`](super::chain).
    pub filters: String,
}

impl MeasureRequest {
    #[must_use]
    pub fn command(&self) -> SidecarCommand {
        let mut command = SidecarCommand::ffmpeg()
            .option("-v", "error")
            .flag("-copyts")
            .option("-seek_timestamp", "1")
            .flag("-noaccurate_seek");
        let seek = self.start_seconds - SEEK_MARGIN_SECONDS;
        if seek > 0.0 {
            command = command.option("-ss", format!("{seek:.6}"));
        }
        let layout = if self.channels == 1 { "mono" } else { "stereo" };
        let mut graph = format!(
            "atrim=start={start:.6}:end={end:.6},aresample={rate},",
            start = self.start_seconds.max(0.0),
            end = self.end_seconds,
            rate = self.sample_rate,
        );
        if !self.filters.is_empty() {
            graph.push_str(&self.filters);
            graph.push(',');
        }
        let _ = write!(graph, "aformat=sample_fmts=flt:channel_layouts={layout}");
        command
            .input(&self.source)
            .option("-map", format!("0:{}", self.stream))
            .flags(&["-vn", "-sn", "-dn"])
            .option("-af", graph)
            .option("-f", "f32le")
            .output_stdout()
    }
}

/// Why a measurement did not finish.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum MeasureError {
    #[error("the measurement was cancelled")]
    Cancelled,
    #[error("the sound could not be decoded: {0}")]
    Failed(String),
}

/// Decode `request` and measure it. Blocks until the stretch is measured.
///
/// # Errors
///
/// The sidecar failed, or `cancel` fired.
pub fn measure(
    orchestrator: &Orchestrator,
    request: &MeasureRequest,
    cancel: &CancelToken,
) -> Result<Loudness, MeasureError> {
    let channels = usize::try_from(request.channels.clamp(1, 2)).unwrap_or(2);
    let meter = Arc::new(Mutex::new(LoudnessMeter::new(
        request.sample_rate,
        channels,
    )));
    let sink = Arc::clone(&meter);
    let mut pending: Vec<u8> = Vec::new();
    let options = JobOptions::default()
        .cancel_token(cancel.clone())
        .on_chunk(move |chunk| {
            pending.extend_from_slice(chunk);
            let whole = pending.len() - pending.len() % 4;
            let samples: Vec<f32> = pending
                .get(..whole)
                .unwrap_or_default()
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            pending.drain(..whole);
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(&samples);
            Flow::Continue
        });
    orchestrator
        .run(request.command(), Priority::Background, options)
        .wait()
        .map_err(|error| match error {
            JobError::Cancelled | JobError::ShuttingDown => MeasureError::Cancelled,
            other => MeasureError::Failed(other.to_string()),
        })?;
    let loudness = meter
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .finish();
    Ok(loudness)
}

/// [`measure`], answered from `cache` when this stretch of this version of
/// the file was measured through the same filters before, and remembered
/// there when it was not. The key is the file's content and the whole
/// request but the path, so moving the file keeps the answer and changing a
/// filter before the point measured does not.
///
/// # Errors
///
/// As [`measure`].
pub fn measure_cached(
    orchestrator: &Orchestrator,
    cache: Option<&Cache>,
    request: &MeasureRequest,
    cancel: &CancelToken,
) -> Result<Loudness, MeasureError> {
    let entry = cache.and_then(|cache| {
        let key = ContentKey::of(&request.source).ok()?;
        let mut hasher = Sha256::new();
        hasher.update(
            format!(
                "{CACHE_FORMAT}|{}|{:.6}|{:.6}|{}|{}|{}",
                request.stream,
                request.start_seconds,
                request.end_seconds,
                request.sample_rate,
                request.channels,
                request.filters
            )
            .as_bytes(),
        );
        let digest = hasher.finalize();
        let hex = digest.iter().take(12).fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        Some((cache, cache.path("loudness", &key, &format!("-{hex}.json"))))
    });
    if let Some((cache, path)) = &entry
        && let Some(bytes) = cache.read(path)
        && let Ok(loudness) = serde_json::from_slice::<Loudness>(&bytes)
    {
        return Ok(loudness);
    }
    let loudness = measure(orchestrator, request, cancel)?;
    if let Some((cache, path)) = &entry
        && let Ok(bytes) = serde_json::to_vec(&loudness)
    {
        let _ = cache.write(path, &bytes);
    }
    Ok(loudness)
}

#[cfg(test)]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
mod tests {
    use super::*;

    /// `seconds` of a stereo sine at `frequency` and `amplitude`.
    fn sine(rate: u32, seconds: f64, frequency: f64, amplitude: f64, phase: f64) -> Vec<f32> {
        let frames = (f64::from(rate) * seconds) as usize;
        (0..frames)
            .flat_map(|n| {
                let t = n as f64 / f64::from(rate);
                let value = (amplitude * (2.0 * PI * frequency * t + phase).sin()) as f32;
                [value, value]
            })
            .collect()
    }

    #[test]
    fn a_full_scale_1k_sine_in_both_channels_is_0_lufs() {
        // BS.1770-4: a 0 dBFS 997 Hz sine in one channel reads −3.01 LUFS —
        // the −0.691 offset cancels the K-weighting's gain at 1 kHz — so in
        // both channels, summed, 0 LUFS.
        let mut meter = LoudnessMeter::new(48_000, 2);
        meter.push(&sine(48_000, 5.0, 997.0, 1.0, 0.0));
        let loudness = meter.finish();
        let integrated = loudness.integrated_lufs.unwrap_or(f64::NAN);
        assert!(integrated.abs() < 0.05, "{integrated}");
        // A steady tone has no loudness range.
        assert!(loudness.range_lu.unwrap_or(f64::NAN) < 0.1);
        assert!((loudness.seconds - 5.0).abs() < 1e-9);
    }

    #[test]
    fn the_relative_gate_ignores_quiet_passages() {
        // 10 s at −20 dBFS and 10 s at −60 dBFS: the quiet half is more than
        // 10 LU down, so it does not pull the integrated figure down. Only the
        // few blocks straddling the change, partly loud, pass the gate too.
        let mut meter = LoudnessMeter::new(48_000, 2);
        meter.push(&sine(48_000, 10.0, 997.0, 0.1, 0.0));
        meter.push(&sine(48_000, 10.0, 997.0, 0.001, 0.0));
        let integrated = meter.finish().integrated_lufs.unwrap_or(f64::NAN);
        assert!((integrated - -20.0).abs() < 0.1, "{integrated}");
    }

    #[test]
    fn the_true_peak_finds_the_peak_between_samples() {
        // A sine at a quarter of the rate, sampled 45° off its crest: every
        // sample is at 0.707 of the amplitude, and the waveform they describe
        // reaches the full amplitude between them — 3 dB above the samples.
        let mut meter = LoudnessMeter::new(48_000, 2);
        meter.push(&sine(48_000, 1.0, 12_000.0, 0.5, PI / 4.0));
        let loudness = meter.finish();
        let sample_peak = 20.0 * (0.5_f64 * (PI / 4.0).sin()).log10();
        let true_peak = loudness.true_peak_dbtp.unwrap_or(f64::NAN);
        assert!(
            true_peak > sample_peak + 2.5,
            "{true_peak} vs {sample_peak}"
        );
        assert!(
            (true_peak - 20.0 * 0.5_f64.log10()).abs() < 0.2,
            "{true_peak}"
        );
    }

    #[test]
    fn silence_measures_as_nothing_rather_than_minus_infinity() {
        let mut meter = LoudnessMeter::new(48_000, 1);
        meter.push(&vec![0.0; 48_000]);
        let loudness = meter.finish();
        assert_eq!(loudness.integrated_lufs, None);
        assert_eq!(loudness.range_lu, None);
        assert_eq!(loudness.true_peak_dbtp, None);
    }

    #[test]
    fn the_interpolator_passes_a_constant_unchanged() {
        for phase in interpolation_kernel() {
            let sum: f64 = phase.iter().sum();
            assert!((sum - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn the_command_trims_and_runs_the_chain_before_measuring() {
        let command = MeasureRequest {
            source: PathBuf::from("a.mp4"),
            stream: 1,
            start_seconds: 10.0,
            end_seconds: 12.5,
            sample_rate: 48_000,
            channels: 1,
            filters: "volume=3dB".to_owned(),
        }
        .command()
        .to_string();
        assert!(command.contains("-ss 9.500000"), "{command}");
        assert!(
            command.contains(
                "atrim=start=10.000000:end=12.500000,aresample=48000,volume=3dB,aformat=sample_fmts=flt:channel_layouts=mono"
            ),
            "{command}"
        );
    }
}
