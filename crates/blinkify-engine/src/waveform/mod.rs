//! Audio waveform peaks, with a zoom pyramid, cached on disk.
//!
//! The timeline draws waveforms at every zoom level, and decoding audio to
//! draw a frame is not viable. So peaks are computed once per file version and
//! read cheaply ever after:
//!
//! - **Minimum and maximum per bucket, per channel** — never RMS alone, which
//!   hides exactly the clipping and transients a user zooms in to find. Samples
//!   are decoded as floats and clamped into `i16`, so a clipped sample reaches
//!   full scale and stays visible.
//! - **A pyramid**: the base level has 256 samples per bucket, and each level
//!   above halves the resolution by combining pairs. Any zoom reads the level
//!   nearest its scale instead of decimating a two-hour base array on every
//!   repaint.
//! - **The whole file**, even when a clip uses part of it — the user can extend
//!   a trim at any time.
//! - **Cached** in the content-keyed cache in a versioned binary format
//!   ([`format`]), within the cache's size budget.

mod format;

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::cache::{Cache, ContentKey};
use crate::orchestrator::{Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand};
use crate::probe::MediaInfo;

pub use format::VERSION as FORMAT_VERSION;

/// Samples per bucket at the base level. At 48 kHz, 187.5 buckets a second —
/// finer than any timeline zoom that still shows a waveform rather than a
/// sample plot.
pub const BASE_SAMPLES_PER_BUCKET: u32 = 256;

/// Levels stop once a level has this few buckets or fewer: a whole file at
/// the coarsest zoom is a few hundred pixels.
const TOP_LEVEL_BUCKETS: u64 = 512;

/// How often generation progress is reported.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// One level of the pyramid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    pub samples_per_bucket: u32,
    pub buckets: u64,
    /// `buckets × channels × (min, max)`, interleaved in that order.
    pub data: Vec<i16>,
}

impl Level {
    /// `(min, max)` of `channel` in `bucket`.
    #[must_use]
    pub fn peak(&self, bucket: u64, channel: u16, channels: u16) -> Option<(i16, i16)> {
        let base = usize::try_from(bucket)
            .ok()?
            .checked_mul(usize::from(channels))?
            .checked_add(usize::from(channel))?
            .checked_mul(2)?;
        Some((*self.data.get(base)?, *self.data.get(base + 1)?))
    }
}

/// Peaks for one audio stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peaks {
    pub channels: u16,
    pub sample_rate: u32,
    pub base_samples_per_bucket: u32,
    /// Samples per channel in the whole stream.
    pub total_samples: u64,
    /// Finest first.
    pub levels: Vec<Level>,
}

impl Peaks {
    /// The level to draw at `samples_per_pixel`: the coarsest one that still
    /// has at least one bucket per pixel, so nothing is decimated at draw time
    /// and no detail the screen could show is thrown away.
    #[must_use]
    pub fn level_for(&self, samples_per_pixel: f64) -> Option<&Level> {
        self.levels
            .iter()
            .rev()
            .find(|level| f64::from(level.samples_per_bucket) <= samples_per_pixel)
            .or_else(|| self.levels.first())
    }

    /// The summed view: per bucket, the lowest minimum and highest maximum
    /// across all channels.
    pub fn summed<'a>(
        &'a self,
        level: &'a Level,
        buckets: std::ops::Range<u64>,
    ) -> impl Iterator<Item = (i16, i16)> + 'a {
        buckets.filter_map(move |bucket| {
            (0..self.channels)
                .filter_map(|channel| level.peak(bucket, channel, self.channels))
                .reduce(|(lo, hi), (min, max)| (lo.min(min), hi.max(max)))
        })
    }

    /// The stream's duration.
    #[must_use]
    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        #[allow(clippy::cast_precision_loss)]
        let seconds = self.total_samples as f64 / f64::from(self.sample_rate);
        Duration::from_secs_f64(seconds)
    }
}

/// Where a waveform stands, for the timeline to draw a placeholder until it
/// is ready.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "kebab-case")]
#[ts(export)]
pub enum WaveformStatus {
    /// Being generated; draw a placeholder, not an empty track.
    Pending {
        fraction: f64,
    },
    Ready,
}

#[derive(Debug, Error)]
pub enum WaveformError {
    #[error("stream {0} is not an audio stream of this file")]
    NoSuchStream(u32),
    #[error("could not decode the audio: {0}")]
    Engine(#[source] JobError),
    #[error("the file could not be read: {0}")]
    Io(#[source] std::io::Error),
}

/// Accumulates decoded samples into the base level.
#[derive(Debug)]
struct Builder {
    channels: u16,
    samples_per_bucket: u32,
    /// Current bucket's min and max per channel.
    current: Vec<(i16, i16)>,
    in_bucket: u32,
    base: Vec<i16>,
    buckets: u64,
    total_samples: u64,
    /// Bytes of an incomplete sample frame left over from the last chunk.
    carry: Vec<u8>,
}

impl Builder {
    fn new(channels: u16, samples_per_bucket: u32) -> Self {
        Self {
            channels,
            samples_per_bucket,
            current: vec![(i16::MAX, i16::MIN); usize::from(channels)],
            in_bucket: 0,
            base: Vec::new(),
            buckets: 0,
            total_samples: 0,
            carry: Vec::new(),
        }
    }

    /// Feed interleaved little-endian `f32` samples.
    fn feed(&mut self, bytes: &[u8]) {
        let frame = usize::from(self.channels) * 4;
        let mut data = std::mem::take(&mut self.carry);
        data.extend_from_slice(bytes);
        let whole = data.len() - data.len() % frame;
        for sample_frame in data.get(..whole).unwrap_or_default().chunks_exact(frame) {
            for (channel, raw) in sample_frame.as_chunks::<4>().0.iter().enumerate() {
                let quantised = quantise(f32::from_le_bytes(*raw));
                if let Some(peak) = self.current.get_mut(channel) {
                    peak.0 = peak.0.min(quantised);
                    peak.1 = peak.1.max(quantised);
                }
            }
            self.total_samples += 1;
            self.in_bucket += 1;
            if self.in_bucket == self.samples_per_bucket {
                self.close_bucket();
            }
        }
        self.carry = data.get(whole..).unwrap_or_default().to_vec();
    }

    fn close_bucket(&mut self) {
        for peak in &mut self.current {
            self.base.push(peak.0);
            self.base.push(peak.1);
            *peak = (i16::MAX, i16::MIN);
        }
        self.buckets += 1;
        self.in_bucket = 0;
    }

    fn finish(mut self, sample_rate: u32) -> Peaks {
        if self.in_bucket > 0 {
            self.close_bucket();
        }
        let channels = usize::from(self.channels);
        let mut levels = vec![Level {
            samples_per_bucket: self.samples_per_bucket,
            buckets: self.buckets,
            data: self.base,
        }];
        while let Some(finer) = levels.last()
            && finer.buckets > TOP_LEVEL_BUCKETS
        {
            let coarser = halve(finer, channels);
            levels.push(coarser);
        }
        Peaks {
            channels: self.channels,
            sample_rate,
            base_samples_per_bucket: self.samples_per_bucket,
            total_samples: self.total_samples,
            levels,
        }
    }
}

/// A float sample in `i16` range: clamped, so anything at or past full scale
/// lands exactly on it and a clipped waveform looks clipped.
fn quantise(value: f32) -> i16 {
    if value.is_nan() {
        return 0;
    }
    let scaled = (value * 32767.0).round().clamp(-32768.0, 32767.0);
    #[allow(clippy::cast_possible_truncation)]
    let sample = scaled as i16;
    sample
}

/// The next level up: each bucket is the union of two below it.
fn halve(finer: &Level, channels: usize) -> Level {
    let stride = channels * 2;
    let data: Vec<i16> = finer
        .data
        .chunks(stride * 2)
        .flat_map(|pair| {
            let (first, second) = pair.split_at(stride.min(pair.len()));
            (0..channels).flat_map(move |channel| {
                let at = channel * 2;
                let min = first.get(at).copied().unwrap_or(i16::MAX);
                let max = first.get(at + 1).copied().unwrap_or(i16::MIN);
                let (min, max) = match (second.get(at), second.get(at + 1)) {
                    (Some(&other_min), Some(&other_max)) => {
                        (min.min(other_min), max.max(other_max))
                    }
                    _ => (min, max),
                };
                [min, max]
            })
        })
        .collect();
    Level {
        samples_per_bucket: finer.samples_per_bucket.saturating_mul(2),
        buckets: finer.buckets.div_ceil(2),
        data,
    }
}

/// Generates and caches waveform peaks.
#[derive(Debug, Clone)]
pub struct Waveforms {
    orchestrator: Orchestrator,
    cache: Option<Cache>,
}

impl Waveforms {
    #[must_use]
    pub fn new(orchestrator: Orchestrator, cache: Option<Cache>) -> Self {
        Self {
            orchestrator,
            cache,
        }
    }

    /// Peaks for audio `stream` of `path`: from the cache if this version of
    /// the file was done before, otherwise decoded now at background priority,
    /// reporting the fraction done as it goes. Blocks: run it off the UI
    /// thread.
    ///
    /// Returns whether the answer came from the cache, for tests and for the
    /// status the timeline shows.
    ///
    /// # Errors
    ///
    /// The stream is not audio, or it could not be decoded.
    pub fn peaks(
        &self,
        path: &Path,
        info: &MediaInfo,
        stream: u32,
        mut on_progress: impl FnMut(f64) + Send + 'static,
    ) -> Result<(Arc<Peaks>, bool), WaveformError> {
        let (_, audio) = info
            .audio()
            .find(|(candidate, _)| candidate.index == stream)
            .ok_or(WaveformError::NoSuchStream(stream))?;
        let channels = audio
            .channels
            .and_then(|c| u16::try_from(c).ok())
            .filter(|&c| c > 0)
            .ok_or(WaveformError::NoSuchStream(stream))?;
        let sample_rate = audio.sample_rate.filter(|&r| r > 0).unwrap_or(48_000);

        let cache_path = match &self.cache {
            Some(cache) => {
                let key = ContentKey::of(path).map_err(WaveformError::Io)?;
                Some(cache.path("peaks", &key, &format!("-a{stream}.peaks")))
            }
            None => None,
        };
        if let (Some(cache), Some(cache_path)) = (&self.cache, &cache_path)
            && let Some(peaks) = cache
                .read(cache_path)
                .and_then(|bytes| format::decode(&bytes))
        {
            return Ok((Arc::new(peaks), true));
        }

        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let expected_samples = info.container.duration_seconds.map_or(0, |seconds| {
            (seconds * f64::from(sample_rate)).max(0.0) as u64
        });
        let builder = Arc::new(Mutex::new(Builder::new(channels, BASE_SAMPLES_PER_BUCKET)));
        let sink = Arc::clone(&builder);
        let mut last_report = Instant::now();
        self.orchestrator
            .run(
                SidecarCommand::ffmpeg()
                    .option("-loglevel", "error")
                    .input(path)
                    .option("-map", format!("0:{stream}"))
                    .flags(&["-vn", "-sn", "-dn"])
                    // Floats, at the source's own rate and channel count:
                    // resampling would move peaks, and a float can carry a
                    // sample past full scale for the clamp to catch.
                    .option("-c:a", "pcm_f32le")
                    .option("-f", "f32le")
                    .output_stdout(),
                Priority::Background,
                JobOptions::default().on_chunk(move |chunk| {
                    let mut builder = sink.lock().unwrap_or_else(PoisonError::into_inner);
                    builder.feed(chunk);
                    if expected_samples > 0 && last_report.elapsed() >= PROGRESS_INTERVAL {
                        last_report = Instant::now();
                        #[allow(clippy::cast_precision_loss)]
                        let fraction = builder.total_samples as f64 / expected_samples as f64;
                        on_progress(fraction.min(0.999));
                    }
                    Flow::Continue
                }),
            )
            .wait()
            .map_err(WaveformError::Engine)?;

        // The consumer, and its clone of the builder, is dropped with the
        // job's reader thread, which `wait` has joined.
        let builder = Arc::try_unwrap(builder).map_or_else(
            |shared| {
                let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
                std::mem::replace(&mut *guard, Builder::new(channels, BASE_SAMPLES_PER_BUCKET))
            },
            |mutex| mutex.into_inner().unwrap_or_else(PoisonError::into_inner),
        );
        let peaks = builder.finish(sample_rate);
        if let (Some(cache), Some(cache_path)) = (&self.cache, &cache_path) {
            let _ = cache.write(cache_path, &format::encode(&peaks));
        }
        Ok((Arc::new(peaks), false))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn feed_floats(builder: &mut Builder, samples: &[f32]) {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        // Split mid-frame, as a pipe does.
        let (a, b) = bytes.split_at((bytes.len() >> 1) + 1);
        builder.feed(a);
        builder.feed(b);
    }

    #[test]
    fn min_and_max_per_bucket_per_channel() {
        let mut builder = Builder::new(2, 4);
        // Two channels, eight frames: left ramps up, right is quiet.
        feed_floats(
            &mut builder,
            &[
                -0.5, 0.0, 0.25, 0.1, 0.5, -0.1, 0.0, 0.0, //
                1.0, 0.0, -1.0, 0.0, 0.0, 0.2, 0.0, 0.0,
            ],
        );
        let peaks = builder.finish(48_000);
        let base = &peaks.levels[0];
        assert_eq!(base.buckets, 2);
        assert_eq!(base.peak(0, 0, 2), Some((-16384, 16384)));
        assert_eq!(base.peak(0, 1, 2), Some((-3277, 3277)));
        assert_eq!(base.peak(1, 0, 2), Some((-32767, 32767)));
        assert_eq!(peaks.total_samples, 8);
    }

    #[test]
    fn clipping_reaches_full_scale_and_stays_there() {
        assert_eq!(quantise(1.0), 32767);
        assert_eq!(quantise(1.7), 32767);
        assert_eq!(quantise(-3.0), -32768);
        assert_eq!(quantise(f32::NAN), 0);
    }

    #[test]
    fn each_level_is_the_union_of_two_below() {
        let mut builder = Builder::new(1, 1);
        let samples: Vec<f32> = (0..2000)
            .map(|n| if n == 1234 { 1.0 } else { 0.0 })
            .collect();
        feed_floats(&mut builder, &samples);
        let peaks = builder.finish(48_000);
        assert!(peaks.levels.len() > 2);
        assert!(peaks.levels.last().expect("top").buckets <= TOP_LEVEL_BUCKETS);
        for level in &peaks.levels {
            // The one transient survives to every level: a pyramid of peaks,
            // not of averages.
            let loudest = level.data.iter().copied().max().expect("data");
            assert_eq!(
                loudest, 32767,
                "lost at {} samples/bucket",
                level.samples_per_bucket
            );
        }
    }

    #[test]
    fn the_level_for_a_zoom_has_at_least_one_bucket_per_pixel() {
        let mut builder = Builder::new(1, 256);
        feed_floats(&mut builder, &vec![0.0; 256 * 4096]);
        let peaks = builder.finish(48_000);
        assert_eq!(
            peaks.level_for(100.0).expect("level").samples_per_bucket,
            256
        );
        assert_eq!(
            peaks.level_for(256.0).expect("level").samples_per_bucket,
            256
        );
        assert_eq!(
            peaks.level_for(1000.0).expect("level").samples_per_bucket,
            512
        );
        // 4 096 base buckets halve to 512 at 2 048 samples per bucket, where
        // the pyramid stops: past that the coarsest level serves every zoom.
        assert_eq!(
            peaks.level_for(5000.0).expect("level").samples_per_bucket,
            2048
        );
    }

    #[test]
    fn the_summed_view_spans_every_channel() {
        let mut builder = Builder::new(2, 2);
        feed_floats(&mut builder, &[0.5, -0.25, 0.0, 0.0]);
        let peaks = builder.finish(48_000);
        let summed: Vec<_> = peaks.summed(&peaks.levels[0], 0..1).collect();
        assert_eq!(summed, vec![(-8192, 16384)]);
    }
}
