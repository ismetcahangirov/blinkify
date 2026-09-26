//! The bundled sidecar, run for real.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

mod common;

use std::collections::BTreeSet;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use blinkify_engine::VideoCodec;
use blinkify_engine::cache::Cache;
use blinkify_engine::capability::{self, EncodeTrial, EncoderSource, Trial};
use blinkify_engine::capability_cache::{self, ProfileKey};
use blinkify_engine::orchestrator::{Limits, Orchestrator};

fn version_line(program: &std::path::Path, path_env: Option<&std::ffi::OsStr>) -> String {
    let mut command = Command::new(program);
    command.args(["-hide_banner", "-version"]);
    if let Some(path) = path_env {
        command.env("PATH", path);
    }
    let output = command.output().expect("the sidecar runs");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn the_bundled_build_is_the_one_that_answers() {
    let sidecar = common::sidecar();
    let ffmpeg = version_line(sidecar.ffmpeg(), None);
    let ffprobe = version_line(sidecar.ffprobe(), None);
    // `--extra-version=blinkify` is in the committed build script; any other
    // FFmpeg reports something else here.
    assert!(ffmpeg.contains("blinkify"), "{ffmpeg}");
    assert!(ffprobe.contains("blinkify"), "{ffprobe}");
}

#[test]
fn a_different_ffmpeg_first_on_path_is_ignored() {
    // #21: "verified on a machine with a different FFmpeg on PATH — the
    // bundled one must still be used". Put a decoy called ffmpeg.exe first on
    // PATH for the child, and check the bundled build still answers.
    let decoy_dir = std::env::temp_dir().join("blinkify-decoy-ffmpeg-on-path");
    std::fs::create_dir_all(&decoy_dir).expect("temp dir");
    let system = std::env::var_os("SystemRoot").map_or_else(
        || std::path::PathBuf::from(r"C:\Windows"),
        std::path::PathBuf::from,
    );
    std::fs::copy(
        system.join(r"System32\whoami.exe"),
        decoy_dir.join("ffmpeg.exe"),
    )
    .expect("decoy");

    let mut path = std::ffi::OsString::from(decoy_dir.as_os_str());
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());

    let sidecar = common::sidecar();
    let answer = version_line(sidecar.ffmpeg(), Some(&path));
    assert!(answer.contains("blinkify"), "{answer}");
}

#[test]
fn the_capability_probe_finds_the_software_encoders_on_any_machine() {
    let orchestrator = Orchestrator::new(common::sidecar(), Limits::for_this_machine());
    let capabilities = capability::probe(&orchestrator);

    // These are inside the sidecar and need no GPU, so every machine has them.
    let av1: Vec<_> = capabilities
        .encoders_for(VideoCodec::Av1)
        .iter()
        .filter(|encoder| encoder.source == EncoderSource::Software)
        .map(|encoder| encoder.encoder.as_str())
        .collect();
    assert!(av1.contains(&"libsvtav1"), "{av1:?}");
    let vp9 = capabilities.encoders_for(VideoCodec::Vp9);
    let libvpx = vp9
        .iter()
        .find(|encoder| encoder.encoder == "libvpx-vp9")
        .expect("libvpx-vp9 works everywhere");
    let depths: Vec<u8> = libvpx.profiles.iter().map(|p| p.bit_depth).collect();
    assert_eq!(depths, [8, 10]);

    // ADR-0003: whatever the machine, H.264 and HEVC come only from hardware.
    for codec in [VideoCodec::H264, VideoCodec::Hevc] {
        assert!(
            capabilities
                .encoders_for(codec)
                .iter()
                .all(|encoder| encoder.source != EncoderSource::Software)
        );
    }
}

/// The real sidecar, counting every process the probe starts.
struct Counted {
    orchestrator: Orchestrator,
    processes: AtomicUsize,
}

impl EncodeTrial for Counted {
    fn advertised(&self) -> BTreeSet<String> {
        self.processes.fetch_add(1, Ordering::SeqCst);
        self.orchestrator.advertised()
    }
    fn succeeds(&self, trial: &Trial<'_>) -> bool {
        self.processes.fetch_add(1, Ordering::SeqCst);
        self.orchestrator.succeeds(trial)
    }
}

#[test]
fn a_second_launch_on_an_unchanged_machine_starts_no_encoder_trial() {
    // #83, on this machine's real sidecar and real display drivers.
    let sidecar = common::sidecar();
    let key = ProfileKey::of_this_machine(&sidecar).expect("key");

    // The key's hash is the binary's, as its provenance records it.
    let lock: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/ffmpeg-sidecar/sidecar.lock.json"
    ))
    .expect("lock");
    assert_eq!(
        Some(key.sidecar_sha256.as_str()),
        lock["binaries"]["ffmpeg.exe"].as_str()
    );

    let cache = Cache::new(common::scratch("capability-cache"), 1 << 20);
    let launch = || {
        let machine = Counted {
            orchestrator: Orchestrator::new(sidecar.clone(), Limits::for_this_machine()),
            processes: AtomicUsize::new(0),
        };
        let started = Instant::now();
        let found = capability_cache::cached_or_probe(Some(&cache), Some(&key), &machine);
        (found, machine.processes.into_inner(), started.elapsed())
    };

    let (probed, first, probing) = launch();
    assert!(first > 0, "the first launch probes");
    let (reported, second, reading) = launch();
    assert_eq!(second, 0, "the second starts no process at all");
    assert_eq!(reported, probed);
    assert!(
        reading < probing,
        "reading the cache ({reading:?}) is the point of it, the probe took {probing:?}"
    );
}
