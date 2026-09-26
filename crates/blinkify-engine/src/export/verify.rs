//! The proof of losslessness (#45): the hash boundary, defined once.
//!
//! "Bit-identical" says nothing until it says **which bits**. Blinkify's
//! answer, which every test in Epic #6 cites:
//!
//! - **In**: every byte of every packet's payload — the coded picture or the
//!   coded sound.
//! - **Out**: everything the container says *about* a packet — its
//!   presentation and decode timestamps, its byte position, its stream index,
//!   its flags. An export rebases timestamps to zero by design (#40) and a
//!   constant speed rescales them deliberately (#42), so a hash that included
//!   them would fail by construction and prove nothing.
//! - **Out**: parameter sets — H.264 and HEVC VPS/SPS/PPS NAL units, AV1
//!   sequence header and temporal delimiter OBUs — carried inside a packet.
//!   They are the stream's configuration, not its pictures, and a smart-cut
//!   re-sends the source's own before the first copied keyframe after a seam
//!   (#41). Leaving them in would make every such keyframe differ; hashing the
//!   rest keeps every picture byte in.
//!
//! Packets are read as a stream copy reads them — FFmpeg's demuxer, then
//! [`nut`](super::nut) — so the payload compared is exactly what an export
//! would copy, whatever the container stored around it.

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::nut::{self, NutError};
use super::seam::is_parameter_set;
use crate::capability::VideoCodec;
use crate::orchestrator::{Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

/// Seconds a reader adds to every timestamp, so none is negative.
const OFFSET_SECONDS: f64 = 100.0;

/// Seconds read either side of a range.
const RANGE_MARGIN_SECONDS: f64 = 3.0;

/// One packet, by the hash boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHash {
    /// When it is shown, in microseconds of the file's own timeline — for
    /// finding a range, never compared across files.
    pub micros: i64,
    pub key: bool,
    /// SHA-256 of the payload, parameter sets excluded.
    pub sha256: [u8; 32],
}

/// Why packets could not be read or compared.
#[derive(Debug, Error)]
pub enum VerifyError {
    #[error(transparent)]
    Engine(#[from] JobError),
    #[error(transparent)]
    Nut(#[from] NutError),
    #[error("the file has no stream {0}")]
    NoStream(String),
    #[error("the output could not be read: {0}")]
    Probe(String),
}

/// Where two streams first disagree.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Divergence {
    #[error("the source shows no keyframe at the start of the range")]
    NoKeyframe,
    #[error("the output does not contain the source's first packet of the range")]
    Missing,
    #[error("packet {index} of the range differs from the source's")]
    Differs { index: usize },
    #[error("the output ends {missing} packets before the range does")]
    Short { missing: usize },
}

fn codec_of(fourcc: &[u8]) -> Option<VideoCodec> {
    match fourcc {
        b"H264" | b"avc1" => Some(VideoCodec::H264),
        b"HEVC" | b"hvc1" | b"hev1" => Some(VideoCodec::Hevc),
        b"AV01" | b"av01" => Some(VideoCodec::Av1),
        _ => None,
    }
}

/// The NAL units of a length-prefixed packet, or `None` if it is not one.
fn length_prefixed(data: &[u8], size: usize) -> Option<Vec<&[u8]>> {
    let mut units = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let length = data
            .get(at..at + size)?
            .iter()
            .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        units.push(data.get(at + size..at + size + length)?);
        at += size + length;
    }
    Some(units)
}

/// The OBUs of an AV1 temporal unit, each with its header.
fn obus(data: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut units = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let header = *data.get(at)?;
        let start = at;
        at += 1 + usize::from(header & 0x04 != 0);
        if header & 0x02 == 0 {
            // No size field: the OBU runs to the end.
            units.push(((header >> 3) & 0x0F, data.get(start..)?));
            return Some(units);
        }
        let mut size: usize = 0;
        let mut shift = 0;
        loop {
            let byte = *data.get(at)?;
            size |= usize::from(byte & 0x7F).checked_shl(shift)?;
            at += 1;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
            if shift > 56 {
                return None;
            }
        }
        at = at.checked_add(size)?;
        units.push(((header >> 3) & 0x0F, data.get(start..at)?));
    }
    Some(units)
}

/// The hash of `data` by the boundary: parameter sets out, every other
/// byte in. `framing` is the NAL length size, or `None` for Annex B or a
/// codec without NAL units.
fn hash(codec: Option<VideoCodec>, framing: Option<usize>, data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let mut whole = || {
        hasher.update(data);
    };
    match (codec, framing) {
        (Some(codec @ (VideoCodec::H264 | VideoCodec::Hevc)), Some(size)) => {
            match length_prefixed(data, size) {
                Some(units) => {
                    for unit in units.into_iter().filter(|u| !is_parameter_set(codec, u)) {
                        hasher.update((unit.len() as u64).to_be_bytes());
                        hasher.update(unit);
                    }
                }
                None => whole(),
            }
        }
        (Some(codec @ (VideoCodec::H264 | VideoCodec::Hevc)), None) => {
            for unit in super::sps::annex_b_units(data)
                .into_iter()
                .filter(|u| !is_parameter_set(codec, u))
            {
                hasher.update((unit.len() as u64).to_be_bytes());
                hasher.update(unit);
            }
        }
        (Some(VideoCodec::Av1), _) => match obus(data) {
            // Sequence headers (1) and temporal delimiters (2) out.
            Some(units) => {
                for (_, unit) in units.into_iter().filter(|(kind, _)| !matches!(kind, 1 | 2)) {
                    hasher.update(unit);
                }
            }
            None => whole(),
        },
        _ => whole(),
    }
    hasher.finalize().into()
}

/// Every packet of `path`'s stream `selector` (`v:0`, `a:1`), in the order
/// the file stores them — decode order — hashed by the boundary.
///
/// # Errors
///
/// The file or the stream cannot be read.
pub fn payload_hashes(
    orchestrator: &Orchestrator,
    path: &Path,
    selector: &str,
) -> Result<Vec<PacketHash>, VerifyError> {
    payload_hashes_between(orchestrator, path, selector, None)
}

/// [`payload_hashes`], read only around `range` — seconds of the file's own
/// timeline — where one is given: from the keyframe before its start to a
/// little past its end. What the export report (#52) reads of a long source
/// a short clip was taken from.
///
/// # Errors
///
/// The file or the stream cannot be read.
pub fn payload_hashes_between(
    orchestrator: &Orchestrator,
    path: &Path,
    selector: &str,
    range: Option<(f64, f64)>,
) -> Result<Vec<PacketHash>, VerifyError> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-copyts");
    if let Some((from, to)) = range {
        // The demuxer seeks to the keyframe before `-ss`; the margin keeps a
        // rounding from landing just after the one the range starts on.
        command = command
            .option(
                "-ss",
                format!("{:.6}", (from - RANGE_MARGIN_SECONDS).max(0.0)),
            )
            .option("-to", format!("{:.6}", to + RANGE_MARGIN_SECONDS));
    }
    orchestrator
        .run(
            command
                .input(path)
                .option("-map", format!("0:{selector}"))
                .option("-c", "copy")
                .option("-output_ts_offset", "100")
                .option("-f", "nut")
                .output_stdout(),
            Priority::Background,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()?;
    let bytes = std::mem::take(&mut *collected.lock().unwrap_or_else(PoisonError::into_inner));
    let mut reader = nut::Reader::open(bytes.as_slice())?;
    let stream = reader
        .header()
        .streams
        .first()
        .cloned()
        .ok_or_else(|| VerifyError::NoStream(selector.to_owned()))?;
    let codec = codec_of(&stream.fourcc);
    let framing = match codec {
        Some(VideoCodec::H264) if !super::sps::is_annex_b(&stream.extradata) => {
            stream.extradata.get(4).map(|b| usize::from(b & 0x03) + 1)
        }
        Some(VideoCodec::Hevc) if !super::sps::is_annex_b(&stream.extradata) => {
            stream.extradata.get(21).map(|b| usize::from(b & 0x03) + 1)
        }
        _ => None,
    };
    let base = stream.time_base;
    let mut packets = Vec::new();
    while let Some(packet) = reader.next_packet()? {
        let seconds = crate::time::seconds(packet.pts, base) - OFFSET_SECONDS;
        packets.push(PacketHash {
            micros: crate::time::from_seconds(
                seconds,
                crate::time::MICROSECONDS,
                crate::time::Rounding::Nearest,
            ),
            key: packet.key,
            sha256: hash(codec, framing, &packet.data),
        });
    }
    Ok(packets)
}

/// The source's packets a copy of `from..to` (microseconds of the source's
/// timeline) must contain, in decode order: from the keyframe shown at
/// `from` until the keyframe that ends the range, those shown inside it.
///
/// # Errors
///
/// [`Divergence::NoKeyframe`] if no keyframe is shown at `from`.
pub fn range_of(source: &[PacketHash], from: i64, to: i64) -> Result<Vec<[u8; 32]>, Divergence> {
    let start = source
        .iter()
        .position(|p| p.key && (p.micros - from).abs() <= 1)
        .ok_or(Divergence::NoKeyframe)?;
    Ok(source
        .iter()
        .skip(start)
        .enumerate()
        .take_while(|(i, p)| *i == 0 || !(p.key && p.micros >= to - 1))
        .map(|(_, p)| p)
        .filter(|p| p.micros >= from - 1 && p.micros < to - 1)
        .map(|p| p.sha256)
        .collect())
}

/// Whether `output` contains `expected` as one contiguous run, packet for
/// packet, by the hash boundary.
///
/// # Errors
///
/// Where the run is not there: its first packet missing, a packet that
/// differs, or the output ending early.
pub fn contains(output: &[PacketHash], expected: &[[u8; 32]]) -> Result<(), Divergence> {
    let Some(first) = expected.first() else {
        return Ok(());
    };
    let start = output
        .iter()
        .position(|p| p.sha256 == *first)
        .ok_or(Divergence::Missing)?;
    for (index, hash) in expected.iter().enumerate() {
        match output.get(start + index) {
            Some(packet) if packet.sha256 == *hash => {}
            Some(_) => return Err(Divergence::Differs { index }),
            None => {
                return Err(Divergence::Short {
                    missing: expected.len() - index,
                });
            }
        }
    }
    Ok(())
}

/// What FFmpeg writes to its error stream decoding every stream of `path` in
/// full: empty for a file that decodes cleanly. The exit code alone is not
/// enough — most decode errors are not fatal. Frames pass on their own
/// timestamps: the check is of the decoders, not of a frame-rate conversion.
///
/// # Errors
///
/// The file cannot be decoded at all.
pub fn decode_errors(orchestrator: &Orchestrator, path: &Path) -> Result<Vec<String>, VerifyError> {
    Ok(orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0")
                .option("-fps_mode", "passthrough")
                .option("-enc_time_base", "demux")
                .output_null(),
            Priority::Background,
        )?
        .stderr_tail)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parameter_sets_are_outside_the_boundary_and_pictures_inside() {
        let slice = [0, 0, 0, 3, 0x65, 0x88, 0x84];
        let mut with_sets = vec![0, 0, 0, 2, 0x67, 0x64, 0, 0, 0, 2, 0x68, 0xEB];
        with_sets.extend_from_slice(&slice);
        let h264 = Some(VideoCodec::H264);
        assert_eq!(hash(h264, Some(4), &slice), hash(h264, Some(4), &with_sets));
        let mut changed = slice;
        changed[6] ^= 1;
        assert_ne!(hash(h264, Some(4), &slice), hash(h264, Some(4), &changed));
    }

    #[test]
    fn a_run_is_found_and_a_changed_packet_is_named() {
        let packet = |n: u8| PacketHash {
            micros: i64::from(n),
            key: n == 0,
            sha256: [n; 32],
        };
        let output: Vec<PacketHash> = (0..6).map(packet).collect();
        assert_eq!(contains(&output, &[[2; 32], [3; 32], [4; 32]]), Ok(()));
        assert_eq!(
            contains(&output, &[[2; 32], [9; 32]]),
            Err(Divergence::Differs { index: 1 })
        );
        assert_eq!(
            contains(&output, &[[5; 32], [6; 32]]),
            Err(Divergence::Short { missing: 1 })
        );
        assert_eq!(contains(&output, &[[7; 32]]), Err(Divergence::Missing));
    }
}
