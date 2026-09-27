//! Copy a file with its H.264 or HEVC cropping window rewritten (#126).
//!
//! The research tool behind ADR-0020: the files it writes are the ones whose
//! playback the ADR records, and `tests/crop_window.rs` runs this same
//! [`remux`] on the corpus. It runs the export's own shape — a sidecar reads
//! the source's packets out as NUT, the engine rewrites the stream header and
//! any in-band SPS, a sidecar muxes the result — with every packet copied.
//!
//! ```text
//! cargo run -p blinkify-engine --example crop_window -- IN OUT X Y WIDTH HEIGHT
//! ```
//!
//! The rectangle is in the stored orientation of the coded picture, relative
//! to the picture the source shows now. An existing OUT is never overwritten.
//! The sidecar is found as the tests
//! find it: `BLINKIFY_SIDECAR_DIR`, or where `pnpm sidecar:fetch` puts it.
//!
//! An OUT ending in `.ts` is written as MPEG-TS, where parameter sets travel
//! only in band: the reader converts the packets to Annex B, which repeats
//! the SPS before every keyframe, and every one of them is rewritten. Any
//! other OUT keeps them out of band, in the `avcC`/`hvcC` record.
//!
//! A tool for short test files, not an exporter: it holds the whole file in
//! memory, and it copies every packet — an MP4 edit list's pre-roll is shown,
//! where the export trims it (#40); that is independent of the window.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use blinkify_engine::export::crop_window::{CropWindow, Rect, WindowError};
use blinkify_engine::export::nut::{self, NutError};
use blinkify_engine::orchestrator::{
    Flow, JobError, JobOptions, Limits, Orchestrator, Priority, SidecarCommand,
};
use blinkify_engine::probe::{Prober, StreamKind};
use blinkify_engine::{Sidecar, VideoCodec};

/// Why a file could not be copied with a new window.
#[derive(Debug, thiserror::Error)]
pub enum RemuxError {
    #[error(transparent)]
    Job(#[from] JobError),
    #[error(transparent)]
    Nut(#[from] NutError),
    #[error(transparent)]
    Crop(#[from] WindowError),
    #[error("the file has no H.264 or HEVC video stream")]
    NoVideo,
    #[error("the file could not be probed: {0}")]
    Probe(String),
}

/// What [`remux`] wrote.
#[derive(Debug, Clone)]
pub struct Remuxed {
    pub codec: VideoCodec,
    pub window: CropWindow,
    /// Packets that carried an SPS in band, rewritten.
    pub in_band: usize,
}

fn codec_of(fourcc: &[u8]) -> Option<VideoCodec> {
    match fourcc {
        b"H264" | b"avc1" => Some(VideoCodec::H264),
        b"HEVC" | b"hvc1" | b"hev1" => Some(VideoCodec::Hevc),
        _ => None,
    }
}

/// The video and audio packets of `input`, copied out as NUT, the video
/// through `filter` if one is given.
fn read(
    orchestrator: &Orchestrator,
    input: &Path,
    filter: Option<&'static str>,
) -> Result<Vec<u8>, RemuxError> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let mut reader = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .input(input)
        .option("-map", "0:v:0")
        .option("-map", "0:a?")
        .option("-c", "copy");
    if let Some(filter) = filter {
        reader = reader.option("-bsf:v", filter);
    }
    orchestrator
        .run(
            reader.option("-f", "nut").output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()?;
    Ok(std::mem::take(
        &mut *collected.lock().unwrap_or_else(PoisonError::into_inner),
    ))
}

/// Mux the NUT stream `packets` into `output`, every packet copied.
fn mux(
    orchestrator: &Orchestrator,
    packets: &[u8],
    output: &Path,
    rotation: u32,
    tag: Option<&'static str>,
) -> Result<(), RemuxError> {
    let (sender, chunks) = mpsc::channel();
    for chunk in packets.chunks(1 << 20) {
        // The receiver lives until the job ends; a send cannot fail here.
        let _ = sender.send(chunk.to_vec());
    }
    drop(sender);
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .option("-f", "nut");
    if rotation != 0 {
        command = command.option("-display_rotation:v:0", rotation.to_string());
    }
    let mut command = command
        .stdin_input()
        .option("-map", "0")
        .option("-c", "copy");
    if let Some(tag) = tag {
        command = command.option("-tag:v", tag);
    }
    orchestrator
        .run(
            command.output_file(output),
            Priority::Foreground,
            JobOptions::default().stdin(chunks),
        )
        .wait()?;
    Ok(())
}

/// Copy the first video stream of `input` and its audio into `output`, with
/// the video's cropping window set to show `rect`.
///
/// # Errors
///
/// The sidecar failed, the file has no H.264/HEVC video, or the window
/// cannot be written; see [`RemuxError`].
pub fn remux(
    orchestrator: &Orchestrator,
    input: &Path,
    output: &Path,
    rect: Rect,
) -> Result<Remuxed, RemuxError> {
    let info = Prober::new(orchestrator.clone())
        .probe(input)
        .map_err(|error| RemuxError::Probe(error.to_string()))?;
    let (rotation, codec_name) = info
        .streams
        .iter()
        .find_map(|stream| match &stream.kind {
            StreamKind::Video(video) => Some((video.rotation, stream.codec.clone())),
            _ => None,
        })
        .ok_or(RemuxError::NoVideo)?;
    let transport = output
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("ts"));

    let filter = transport.then_some(match codec_name.as_deref() {
        Some("hevc") => "hevc_mp4toannexb",
        _ => "h264_mp4toannexb",
    });
    let bytes = read(orchestrator, input, filter)?;

    let mut reader = nut::Reader::open(bytes.as_slice())?;
    let mut header = reader.header().clone();
    let (video, codec) = header
        .streams
        .iter()
        .enumerate()
        .find_map(|(index, stream)| {
            (stream.class == nut::Class::Video)
                .then(|| codec_of(&stream.fourcc).map(|codec| (index, codec)))
                .flatten()
        })
        .ok_or(RemuxError::NoVideo)?;
    let source = header.streams.get(video).ok_or(RemuxError::NoVideo)?;
    let window = CropWindow::new(codec, &source.extradata, rect)?;
    let rewritten = window.header(source);
    if let Some(stream) = header.streams.get_mut(video) {
        *stream = rewritten;
    }

    let mut written = Vec::with_capacity(bytes.len());
    let mut writer = nut::Writer::new(&mut written, &header)?;
    let mut in_band = 0;
    while let Some(mut packet) = reader.next_packet()? {
        if packet.stream == video {
            let data = window.packet(&packet.data)?;
            if data != packet.data {
                in_band += 1;
            }
            packet.data = data;
        }
        writer.write(&packet)?;
    }
    writer.finish()?;

    // An HEVC stream whose parameter sets are all in the record is `hvc1`,
    // as the source's was; the muxer's default, `hev1`, says they may not be.
    let tag = (codec == VideoCodec::Hevc && in_band == 0 && !transport).then_some("hvc1");
    mux(orchestrator, &written, output, rotation, tag)?;
    Ok(Remuxed {
        codec,
        window,
        in_band,
    })
}

fn sidecar_dir() -> PathBuf {
    std::env::var_os("BLINKIFY_SIDECAR_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src-tauri/binaries"),
        PathBuf::from,
    )
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let numbers: Vec<u32> = args.iter().skip(2).filter_map(|a| a.parse().ok()).collect();
    let (Some(input), Some(output), [x, y, width, height]) =
        (args.first(), args.get(1), numbers.as_slice())
    else {
        eprintln!("usage: crop_window IN OUT X Y WIDTH HEIGHT");
        return std::process::ExitCode::from(2);
    };
    let sidecar = match Sidecar::in_dir(&sidecar_dir()) {
        Ok(sidecar) => sidecar,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let orchestrator = Orchestrator::new(sidecar, Limits::for_this_machine());
    let rect = Rect {
        x: *x,
        y: *y,
        width: *width,
        height: *height,
    };
    match remux(&orchestrator, Path::new(input), Path::new(output), rect) {
        Ok(done) => {
            let source = done.window.source();
            println!(
                "{:?}: coded {}x{}, shown {}x{} at {},{}, unit {}x{} -> window {}x{}, offsets {:?} (left, right, top, bottom), {} packets with an in-band SPS",
                done.codec,
                source.coded_width,
                source.coded_height,
                source.shown.width,
                source.shown.height,
                source.shown.x,
                source.shown.y,
                source.unit_x,
                source.unit_y,
                done.window.size().0,
                done.window.size().1,
                done.window.offsets(),
                done.in_band,
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
