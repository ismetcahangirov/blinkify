//! What kind of picture a keyframe packet starts with.
//!
//! A container's "keyframe" flag says only that decoding can start there. It
//! does not say whether the pictures around it depend on anything before it,
//! which is what decides whether a cut at that keyframe is clean:
//!
//! - **H.264**: an IDR picture (NAL type 5) closes the GOP. A non-IDR I picture
//!   marked as a keyframe is a recovery point, and B-pictures after it in
//!   decode order may reference the previous GOP.
//! - **HEVC**: IDR (types 19, 20) closes the GOP — including `IDR_W_RADL`,
//!   whose leading pictures are decodable. CRA (21) opens it: its RASL leading
//!   pictures reference the previous GOP and are dropped by a decoder that
//!   starts there. BLA (16–18) discards its RASL pictures by definition, so it
//!   behaves as closed.
//!
//! So the first VCL NAL unit of the keyframe packet is read from the file. Only
//! for ISO-BMFF (MP4, MOV), where `ffprobe`'s packet position is exactly the
//! sample's first byte and NAL units are length-prefixed; in other containers
//! the position points at container framing, and the caller falls back to
//! looking for leading pictures alone.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Bytes read from the start of a keyframe packet. Parameter sets and SEI
/// come first, and together are rarely more than a few hundred bytes.
const HEAD: u64 = 4096;

/// The first coded picture of a keyframe packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PictureKind {
    /// H.264 IDR, HEVC `IDR_W_RADL` or `IDR_N_LP`: closes the GOP.
    Idr,
    /// HEVC broken-link access: its RASL pictures are discarded, so it closes
    /// the GOP too.
    Bla,
    /// HEVC clean random access: opens the GOP when RASL pictures follow.
    Cra,
    /// H.264 non-IDR intra picture flagged as a keyframe: a recovery point.
    RecoveryPoint,
}

impl PictureKind {
    /// Whether the picture by its nature closes the GOP, whatever follows it.
    #[must_use]
    pub const fn closes_gop(self) -> bool {
        matches!(self, Self::Idr | Self::Bla)
    }
}

/// A NAL unit: a coded picture (of a random-access kind, or not), or
/// something else — parameter sets, SEI, delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Picture(Option<PictureKind>),
    NotAPicture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub(super) fn from_name(name: Option<&str>) -> Option<Self> {
        match name? {
            "h264" => Some(Self::H264),
            "hevc" => Some(Self::Hevc),
            _ => None,
        }
    }

    /// What a NAL unit is, from the first byte of its header.
    fn unit(self, header: u8) -> Unit {
        match self {
            Self::H264 => match header & 0x1f {
                5 => Unit::Picture(Some(PictureKind::Idr)),
                1..=4 => Unit::Picture(Some(PictureKind::RecoveryPoint)),
                _ => Unit::NotAPicture,
            },
            Self::Hevc => match (header >> 1) & 0x3f {
                19 | 20 => Unit::Picture(Some(PictureKind::Idr)),
                16..=18 => Unit::Picture(Some(PictureKind::Bla)),
                21 => Unit::Picture(Some(PictureKind::Cra)),
                0..=15 | 22..=31 => Unit::Picture(None),
                _ => Unit::NotAPicture,
            },
        }
    }
}

/// Read the keyframe packet at `pos` and classify its first coded picture.
/// `None` when the bytes cannot be read or do not parse — the caller then
/// decides from leading pictures alone.
pub(super) fn picture_kind(path: &Path, codec: Codec, pos: u64, size: u64) -> Option<PictureKind> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(pos)).ok()?;
    let mut head = Vec::new();
    file.take(size.min(HEAD)).read_to_end(&mut head).ok()?;
    classify_length_prefixed(codec, &head)
}

/// Walk 4-byte length-prefixed NAL units (the ISO-BMFF convention, and what
/// every encoder in practice writes) to the first VCL unit.
fn classify_length_prefixed(codec: Codec, packet: &[u8]) -> Option<PictureKind> {
    let mut rest = packet;
    loop {
        let length = usize::try_from(u32::from_be_bytes(rest.get(0..4)?.try_into().ok()?)).ok()?;
        let unit = rest.get(4..)?;
        let header = *unit.first()?;
        if length == 0 {
            return None;
        }
        if let Unit::Picture(kind) = codec.unit(header) {
            return kind;
        }
        // Not a picture yet; skip it. A length that runs past the bytes read
        // ends the walk rather than being trusted.
        rest = unit.get(length..)?;
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn unit(header: &[u8], body_len: usize) -> Vec<u8> {
        let length = u32::try_from(header.len() + body_len).unwrap_or(0);
        let mut out = length.to_be_bytes().to_vec();
        out.extend_from_slice(header);
        out.extend(std::iter::repeat_n(0xAB, body_len));
        out
    }

    #[test]
    fn h264_idr_after_parameter_sets() {
        // SPS (7), PPS (8), SEI (6), then an IDR slice (5).
        let packet = [
            unit(&[0x67], 20),
            unit(&[0x68], 4),
            unit(&[0x06], 30),
            unit(&[0x65], 100),
        ]
        .concat();
        assert_eq!(
            classify_length_prefixed(Codec::H264, &packet),
            Some(PictureKind::Idr)
        );
    }

    #[test]
    fn h264_non_idr_intra_is_a_recovery_point() {
        let packet = [unit(&[0x06], 10), unit(&[0x41], 100)].concat();
        assert_eq!(
            classify_length_prefixed(Codec::H264, &packet),
            Some(PictureKind::RecoveryPoint)
        );
    }

    #[test]
    fn hevc_idr_cra_and_bla() {
        let hevc = |nal_type: u8| {
            let packet = [unit(&[32 << 1, 1], 10), unit(&[nal_type << 1, 1], 50)].concat();
            classify_length_prefixed(Codec::Hevc, &packet)
        };
        assert_eq!(hevc(19), Some(PictureKind::Idr));
        assert_eq!(hevc(20), Some(PictureKind::Idr));
        assert_eq!(hevc(21), Some(PictureKind::Cra));
        assert_eq!(hevc(17), Some(PictureKind::Bla));
        assert_eq!(hevc(1), None, "a trailing picture is not a keyframe kind");
    }

    #[test]
    fn a_length_past_the_end_is_not_believed() {
        let mut packet = unit(&[0x67], 4);
        packet[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(classify_length_prefixed(Codec::H264, &packet), None);
        assert_eq!(classify_length_prefixed(Codec::H264, &[]), None);
        assert_eq!(
            classify_length_prefixed(Codec::H264, &[0, 0, 0, 0, 0x65]),
            None
        );
    }

    #[test]
    fn only_idr_and_bla_close_by_themselves() {
        assert!(PictureKind::Idr.closes_gop());
        assert!(PictureKind::Bla.closes_gop());
        assert!(!PictureKind::Cra.closes_gop());
        assert!(!PictureKind::RecoveryPoint.closes_gop());
    }
}
