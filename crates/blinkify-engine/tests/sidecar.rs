//! The bundled sidecar, run for real.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

mod common;

use std::process::Command;

use blinkify_engine::VideoCodec;
use blinkify_engine::capability::{self, EncoderSource};
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
