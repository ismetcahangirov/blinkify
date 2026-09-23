//! Where the preview's audio goes: the default output device, or — when there
//! is none, or it fails — nowhere, at real time.
//!
//! The silent sink is not a stub. It consumes the output buffer at exactly the
//! rate a device would, and reports when each frame "plays", so the playback
//! clock keeps running and video keeps presenting against it. A machine with
//! no audio device previews in silence rather than not at all (#28), and a
//! device that disappears mid-playback costs a moment of sound, never the
//! playback.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::OutputBuffer;

/// The buffer a sink pulls from. Replaced, not mutated, when the sample rate
/// changes, so a sink only ever sees one rate per buffer.
pub type BufferSlot = Arc<Mutex<Option<Arc<OutputBuffer>>>>;

/// The rate the silent sink runs at when there is no device to ask.
pub const SILENT_SAMPLE_RATE: u32 = 48_000;

/// How long opening a device may take before it is treated as unavailable.
const OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the silent sink wakes to consume what a device would have.
const SILENT_PERIOD: Duration = Duration::from_millis(5);

/// Where preview audio is going, as the renderer shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[ts(export)]
pub enum AudioOutputState {
    /// Playing through this device.
    Device { name: String },
    /// Playing in silence, with the reason — playback itself continues.
    Silent { reason: String },
}

/// The default output device, driven by a thread that owns its stream.
///
/// The stream lives on its own thread because a `cpal` stream is not `Send`
/// on every platform; everything else talks to the thread through the slot
/// and two flags.
#[derive(Debug)]
pub struct DeviceSink {
    name: String,
    sample_rate: u32,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DeviceSink {
    /// Open the default output device and start pulling from `slot`.
    ///
    /// # Errors
    ///
    /// There is no output device, or it cannot be opened in a format this
    /// sink can feed.
    pub fn open(slot: BufferSlot) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let (opened, result) = mpsc::channel();
        let thread = {
            let stop = Arc::clone(&stop);
            let failed = Arc::clone(&failed);
            thread::Builder::new()
                .name("audio-output".to_owned())
                .spawn(move || run_device(&slot, &stop, &failed, &opened))
                .map_err(|error| error.to_string())?
        };
        match result.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok((name, sample_rate))) => Ok(Self {
                name,
                sample_rate,
                stop,
                failed,
                thread: Some(thread),
            }),
            Ok(Err(reason)) => {
                let _ = thread.join();
                Err(reason)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("the audio device did not open in time".to_owned())
            }
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The device reported an error — it was unplugged, or the system took it
    /// away. The sink is then dead and must be replaced.
    #[must_use]
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }
}

impl Drop for DeviceSink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

type Opened = Result<(String, u32), String>;

fn run_device(
    slot: &BufferSlot,
    stop: &Arc<AtomicBool>,
    failed: &Arc<AtomicBool>,
    opened: &mpsc::Sender<Opened>,
) {
    let stream = match open_stream(slot, failed) {
        Ok((stream, name, rate)) => {
            let _ = opened.send(Ok((name, rate)));
            stream
        }
        Err(reason) => {
            let _ = opened.send(Err(reason));
            return;
        }
    };
    while !stop.load(Ordering::SeqCst) {
        thread::park_timeout(Duration::from_millis(100));
    }
    drop(stream);
}

fn open_stream(
    slot: &BufferSlot,
    failed: &Arc<AtomicBool>,
) -> Result<(cpal::Stream, String, u32), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "there is no audio output device".to_owned())?;
    let name = device.to_string();
    let supported = device
        .default_output_config()
        .map_err(|error| format!("{name} cannot report a format: {error}"))?;
    let rate = supported.sample_rate();
    let channels = usize::from(supported.channels());
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, channels, slot, failed),
        SampleFormat::I16 => build::<i16>(&device, config, channels, slot, failed),
        SampleFormat::U16 => build::<u16>(&device, config, channels, slot, failed),
        SampleFormat::I32 => build::<i32>(&device, config, channels, slot, failed),
        other => Err(format!("{name} uses an unsupported sample format, {other}")),
    }?;
    stream
        .play()
        .map_err(|error| format!("{name} could not start: {error}"))?;
    Ok((stream, name, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    slot: &BufferSlot,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let slot = Arc::clone(slot);
    let failed = Arc::clone(failed);
    let mut scratch: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info| {
                let stamp = info.timestamp();
                // When the first sample of this buffer reaches the speaker:
                // the device's own latency is what makes the clock honest.
                let plays_at = Instant::now() + stamp.playback.duration_since(stamp.callback);
                scratch.resize(data.len(), 0.0);
                let buffer = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
                match buffer {
                    Some(buffer) => buffer.pull(&mut scratch, channels, plays_at),
                    None => scratch.fill(0.0),
                }
                for (out, sample) in data.iter_mut().zip(&scratch) {
                    *out = T::from_sample(*sample);
                }
            },
            move |_error| failed.store(true, Ordering::SeqCst),
            None,
        )
        .map_err(|error| error.to_string())
}

/// A sink that plays nothing, at real time.
#[derive(Debug)]
pub struct SilentSink {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SilentSink {
    /// Start consuming `slot` at `sample_rate` frames per second.
    #[must_use]
    pub fn start(slot: BufferSlot, sample_rate: u32) -> Self {
        Self::spawn(slot, sample_rate, None)
    }

    /// As [`SilentSink::start`], keeping every sample consumed — silence for
    /// an underrun included — in `capture`.
    #[must_use]
    pub fn capturing(slot: BufferSlot, sample_rate: u32, capture: Arc<Mutex<Vec<f32>>>) -> Self {
        Self::spawn(slot, sample_rate, Some(capture))
    }

    fn spawn(slot: BufferSlot, sample_rate: u32, capture: Option<Arc<Mutex<Vec<f32>>>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            thread::Builder::new()
                .name("audio-silent".to_owned())
                .spawn(move || run_silent(&slot, sample_rate.max(1), &stop, capture.as_deref()))
                .ok()
        };
        Self { stop, thread }
    }
}

impl Drop for SilentSink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_silent(
    slot: &BufferSlot,
    sample_rate: u32,
    stop: &AtomicBool,
    capture: Option<&Mutex<Vec<f32>>>,
) {
    let started = Instant::now();
    let rate = f64::from(sample_rate);
    let mut pulled: u64 = 0;
    let mut scratch = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        thread::sleep(SILENT_PERIOD);
        let due = started.elapsed().as_secs_f64() * rate;
        // Whole frames only; the remainder is taken next time.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let due = due.floor() as u64;
        let frames = due.saturating_sub(pulled);
        if frames == 0 {
            continue;
        }
        // Each frame "plays" at the moment a device would have played it.
        #[allow(clippy::cast_precision_loss)]
        let plays_at = started + Duration::from_secs_f64(pulled as f64 / rate);
        scratch.resize(usize::try_from(frames).unwrap_or(0) * super::CHANNELS, 0.0);
        let buffer = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
        match buffer {
            Some(buffer) => buffer.pull(&mut scratch, super::CHANNELS, plays_at),
            None => scratch.fill(0.0),
        }
        if let Some(capture) = capture {
            capture
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend_from_slice(&scratch);
        }
        pulled = due;
    }
}

/// The identity of the system's default output device, or `None` when there
/// is none.
#[must_use]
pub fn default_device_id() -> Option<String> {
    let device = cpal::default_host().default_output_device()?;
    device.id().ok().map(|id| id.to_string())
}

/// Either kind of sink.
#[derive(Debug)]
pub enum Sink {
    Device(DeviceSink),
    Silent(SilentSink, u32, String),
}

impl Sink {
    /// The default output device, or a silent sink with the reason there is
    /// none.
    #[must_use]
    pub fn open(slot: &BufferSlot, want_device: bool) -> Self {
        if !want_device {
            return Self::silent(slot, "audio output is off".to_owned());
        }
        match DeviceSink::open(Arc::clone(slot)) {
            Ok(device) => Self::Device(device),
            Err(reason) => Self::silent(slot, reason),
        }
    }

    /// A silent sink, saying why.
    #[must_use]
    pub fn silent(slot: &BufferSlot, reason: String) -> Self {
        Self::Silent(
            SilentSink::start(Arc::clone(slot), SILENT_SAMPLE_RATE),
            SILENT_SAMPLE_RATE,
            reason,
        )
    }

    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Device(device) => device.sample_rate(),
            Self::Silent(_, rate, _) => *rate,
        }
    }

    #[must_use]
    pub fn has_failed(&self) -> bool {
        matches!(self, Self::Device(device) if device.has_failed())
    }

    #[must_use]
    pub fn state(&self) -> AudioOutputState {
        match self {
            Self::Device(device) => AudioOutputState::Device {
                name: device.name().to_owned(),
            },
            Self::Silent(_, _, reason) => AudioOutputState::Silent {
                reason: reason.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_silent_sink_consumes_at_real_time_and_reports_when_frames_play() {
        let buffer = Arc::new(OutputBuffer::new(1000));
        buffer.push(&[0.5; 2 * 1000]);
        let slot: BufferSlot = Arc::new(Mutex::new(Some(Arc::clone(&buffer))));
        let started = Instant::now();
        let sink = SilentSink::start(slot, 1000);
        thread::sleep(Duration::from_millis(300));
        let consumed = buffer.consumed();
        let heard = buffer.heard(Instant::now());
        drop(sink);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        #[allow(clippy::cast_precision_loss)]
        let consumed = consumed as f64;
        assert!(
            consumed > 250.0 && consumed <= elapsed + 1.0,
            "consumed {consumed} in {elapsed} ms"
        );
        assert!(
            (heard - consumed).abs() < 20.0,
            "heard {heard}, consumed {consumed}"
        );
    }

    #[test]
    fn a_sink_that_is_asked_not_to_use_a_device_is_silent_and_says_why() {
        let slot: BufferSlot = Arc::new(Mutex::new(None));
        let sink = Sink::open(&slot, false);
        assert_eq!(sink.sample_rate(), SILENT_SAMPLE_RATE);
        assert!(!sink.has_failed());
        assert_eq!(
            sink.state(),
            AudioOutputState::Silent {
                reason: "audio output is off".to_owned()
            }
        );
    }
}
