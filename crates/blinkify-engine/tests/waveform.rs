//! Waveform peaks against an independently decoded reference, and the cache
//! behaviours #25 names.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use blinkify_engine::cache::{Cache, ContentKey};
use blinkify_engine::orchestrator::{
    Flow, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::probe::Prober;
use blinkify_engine::waveform::{BASE_SAMPLES_PER_BUCKET, Peaks, Waveforms};

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

/// Write a WAV from an `aevalsrc` expression.
fn signal(dir: &Path, name: &str, expression: &str, codec: &str) -> PathBuf {
    let path = dir.join(name);
    orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(expression)
                .option("-c:a", codec)
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("signal written");
    path
}

fn peaks(path: &Path, cache: Option<Cache>) -> (Arc<Peaks>, bool) {
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let stream = info.audio().next().expect("audio").0.index;
    Waveforms::new(orchestrator, cache)
        .peaks(path, &info, stream, |_| {})
        .expect("peaks")
}

/// The base level computed here, from samples decoded by a separate FFmpeg
/// run: the reference the engine's peaks must equal exactly.
fn reference(path: &Path, channels: usize) -> Vec<i16> {
    let decoded: Arc<Mutex<Vec<u8>>> = Arc::default();
    let sink = Arc::clone(&decoded);
    orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .input(path)
                .option("-c:a", "pcm_f32le")
                .option("-f", "f32le")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decode");
    #[allow(clippy::cast_possible_truncation)]
    let quantise = |v: f32| (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
    let decoded = decoded.lock().expect("lock");
    let samples: Vec<f32> = decoded
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    let bucket = BASE_SAMPLES_PER_BUCKET as usize;
    let mut out = Vec::new();
    for frames in samples.chunks(bucket * channels) {
        for channel in 0..channels {
            let values = frames
                .iter()
                .skip(channel)
                .step_by(channels)
                .map(|&v| quantise(v));
            let (min, max) =
                values.fold((i16::MAX, i16::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            out.push(min);
            out.push(max);
        }
    }
    out
}

#[test]
fn peaks_equal_an_independently_decoded_reference() {
    let dir = common::scratch("waveform-reference");
    // Left at half scale, right at a quarter, different frequencies.
    let path = signal(
        &dir,
        "stereo.wav",
        "aevalsrc=0.5*sin(2*PI*440*t)|0.25*sin(2*PI*97*t):s=48000:d=3",
        "pcm_f32le",
    );
    let (peaks, from_cache) = peaks(&path, None);
    assert!(!from_cache);
    assert_eq!(peaks.channels, 2);
    assert_eq!(peaks.sample_rate, 48_000);
    assert_eq!(peaks.total_samples, 144_000);
    assert_eq!(peaks.levels[0].data, reference(&path, 2));

    // And the values are the signal's: per channel, not mixed.
    let base = &peaks.levels[0];
    let left_max = (0..base.buckets)
        .filter_map(|b| base.peak(b, 0, 2))
        .map(|p| p.1)
        .max();
    let right_max = (0..base.buckets)
        .filter_map(|b| base.peak(b, 1, 2))
        .map(|p| p.1)
        .max();
    assert!(
        (16_300..=16_384).contains(&left_max.expect("left")),
        "{left_max:?}"
    );
    assert!(
        (8_150..=8_192).contains(&right_max.expect("right")),
        "{right_max:?}"
    );
}

#[test]
fn clipping_in_the_source_is_visible_in_the_peaks() {
    let dir = common::scratch("waveform-clipping");
    // A float source driven 50 percent past full scale.
    let path = signal(
        &dir,
        "hot.wav",
        "aevalsrc=1.5*sin(2*PI*50*t):s=48000:d=1",
        "pcm_f32le",
    );
    let (peaks, _) = peaks(&path, None);
    for level in &peaks.levels {
        let max = level.data.iter().copied().max().expect("data");
        let min = level.data.iter().copied().min().expect("data");
        assert_eq!(
            (min, max),
            (-32768, 32767),
            "at {} samples/bucket",
            level.samples_per_bucket
        );
    }
    // Most buckets are pinned at full scale — the flat top a user looks for.
    let summed: Vec<_> = peaks
        .summed(&peaks.levels[0], 0..peaks.levels[0].buckets)
        .collect();
    let pinned = summed.iter().filter(|(_, max)| *max == 32767).count();
    assert!(pinned * 3 > summed.len(), "{pinned} of {}", summed.len());
}

#[test]
fn a_multichannel_source_has_a_peak_track_per_channel() {
    let path = common::corpus("multi-audio.mkv");
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let (surround, _) = info.audio().nth(1).expect("the 5.1 track");
    let (peaks, _) = Waveforms::new(orchestrator, None)
        .peaks(&path, &info, surround.index, |_| {})
        .expect("peaks");
    assert_eq!(peaks.channels, 6);
    let base = &peaks.levels[0];
    assert_eq!(
        base.data.len() as u64,
        base.buckets * 6 * 2,
        "six (min, max) pairs per bucket"
    );
}

#[test]
fn peaks_survive_a_restart_and_are_regenerated_when_the_source_changes() {
    let dir = common::scratch("waveform-cache");
    let cache = Cache::new(dir.join("cache"), 256 * 1024 * 1024);
    let path = dir.join("voice.wav");
    std::fs::copy(
        signal(
            &dir,
            "a.wav",
            "aevalsrc=0.3*sin(2*PI*300*t):s=48000:d=2",
            "pcm_s16le",
        ),
        &path,
    )
    .expect("copy");

    let (first, from_cache) = peaks(&path, Some(cache.clone()));
    assert!(!from_cache);
    let (again, from_cache) = peaks(&path, Some(cache.clone()));
    assert!(from_cache, "a second session reads the cache");
    assert_eq!(*first, *again);

    std::fs::copy(
        signal(
            &dir,
            "b.wav",
            "aevalsrc=0.9*sin(2*PI*300*t):s=48000:d=2",
            "pcm_s16le",
        ),
        &path,
    )
    .expect("replace");
    let (changed, from_cache) = peaks(&path, Some(cache));
    assert!(!from_cache, "a changed source is decoded afresh");
    assert_ne!(*changed, *first);
}

#[test]
fn a_cache_file_of_another_format_version_is_rejected_and_rebuilt() {
    let dir = common::scratch("waveform-stale");
    let cache = Cache::new(dir.join("cache"), 256 * 1024 * 1024);
    let path = signal(
        &dir,
        "a.wav",
        "aevalsrc=0.3*sin(2*PI*300*t):s=48000:d=1",
        "pcm_s16le",
    );
    let (good, _) = peaks(&path, Some(cache.clone()));

    // Rewrite the cached file as if an older Blinkify had written it.
    let entry = cache.path(
        "peaks",
        &ContentKey::of(&path).expect("key"),
        &format!("-a{}.peaks", 0),
    );
    let mut bytes = std::fs::read(&entry).expect("cached");
    bytes[8..10].copy_from_slice(&0_u16.to_le_bytes());
    std::fs::write(&entry, &bytes).expect("stale");

    let (rebuilt, from_cache) = peaks(&path, Some(cache));
    assert!(!from_cache, "a stale version must not be read");
    assert_eq!(*rebuilt, *good);
}

#[test]
fn the_cache_stays_within_its_budget() {
    let dir = common::scratch("waveform-budget");
    let one = signal(
        &dir,
        "one.wav",
        "aevalsrc=0.5*sin(2*PI*1*t):s=48000:d=20",
        "pcm_s16le",
    );
    let two = signal(
        &dir,
        "two.wav",
        "aevalsrc=0.5*sin(2*PI*2*t):s=48000:d=20",
        "pcm_s16le",
    );
    let three = signal(
        &dir,
        "three.wav",
        "aevalsrc=0.5*sin(2*PI*3*t):s=48000:d=20",
        "pcm_s16le",
    );

    let probe_size = {
        let cache = Cache::new(dir.join("measure"), u64::MAX);
        let _ = peaks(&one, Some(cache.clone()));
        cache.size_on_disk()
    };
    // Room for two entries, not three.
    let cache = Cache::new(dir.join("cache"), probe_size * 2 + (probe_size >> 1));
    for path in [&one, &two, &three] {
        let _ = peaks(path, Some(cache.clone()));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(cache.size_on_disk() <= probe_size * 2 + (probe_size >> 1));
    let (_, oldest_cached) = peaks(&one, Some(cache.clone()));
    assert!(!oldest_cached, "the least recently used entry was evicted");
}

#[test]
fn progress_is_reported_while_generating() {
    let dir = common::scratch("waveform-progress");
    let path = signal(
        &dir,
        "long.wav",
        "aevalsrc=0.5*sin(2*PI*100*t):s=48000:d=600",
        "pcm_s16le",
    );
    let orchestrator = orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let seen: Arc<Mutex<Vec<f64>>> = Arc::default();
    let sink = Arc::clone(&seen);
    Waveforms::new(orchestrator, None)
        .peaks(&path, &info, 0, move |fraction| {
            sink.lock().expect("lock").push(fraction);
        })
        .expect("peaks");
    let seen = seen.lock().expect("lock");
    assert!(!seen.is_empty(), "no progress reported");
    assert!(seen.windows(2).all(|w| w[0] <= w[1]));
    assert!(seen.iter().all(|&f| (0.0..1.0).contains(&f)));
}

/// One hour of 8 kHz mono, generated once: small enough for a temp directory,
/// long enough that decimating the base level per repaint would show.
fn one_hour() -> &'static Arc<Peaks> {
    static PEAKS: OnceLock<Arc<Peaks>> = OnceLock::new();
    PEAKS.get_or_init(|| {
        let dir = common::scratch("waveform-hour");
        let path = signal(
            &dir,
            "hour.wav",
            "aevalsrc=0.5*sin(2*PI*50*t)*sin(2*PI*0.01*t):s=8000:d=3600",
            "pcm_s16le",
        );
        peaks(&path, None).0
    })
}

#[test]
fn an_hour_draws_at_any_zoom_within_a_frame() {
    let peaks = one_hour();
    let width_px: u64 = 2000;
    let total = peaks.total_samples;
    // From 1 sample per pixel to the whole hour across the screen.
    let mut zoom = 1.0_f64;
    let mut slowest = Duration::ZERO;
    #[allow(clippy::cast_precision_loss)]
    while zoom <= total as f64 / 100.0 {
        let started = Instant::now();
        let level = peaks.level_for(zoom).expect("level");
        // A window in the middle of the file, one screen wide.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let buckets_per_px = (zoom / f64::from(level.samples_per_bucket)).max(1.0) as u64;
        let first = level.buckets >> 1;
        let last = (first + width_px * buckets_per_px).min(level.buckets);
        let drawn = peaks.summed(level, first..last).count();
        slowest = slowest.max(started.elapsed());
        assert!(drawn > 0);
        zoom *= 2.0;
    }
    // One frame at 60 fps. Debug build, so the release build has room.
    assert!(
        slowest < Duration::from_millis(16),
        "slowest zoom level took {slowest:?}"
    );
}
