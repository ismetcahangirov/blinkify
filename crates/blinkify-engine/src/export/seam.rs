//! Smart-cut (#41): a cut between keyframes costs one small re-encoded window
//! instead of the whole clip.
//!
//! The algorithm is `skeskinen/smartcut`'s (MIT; see `THIRD_PARTY.md`), and
//! the names follow it so a fix upstream can be read against this port:
//!
//! - A clip is split at its keyframes. The whole GOPs inside it are
//!   **remuxed** — copied packet for packet. The partial GOP at a cut that is
//!   not on a keyframe is **recoded**: decoded from its keyframe and encoded
//!   from the cut to the next keyframe (the in-point), or from the last
//!   keyframe to the cut (the out-point). Upstream's `CutSegment` with
//!   `require_recode` is [`Piece`] here.
//! - An open GOP's **leading pictures** — shown before their keyframe,
//!   decoded after it, referencing the GOP before — cannot follow a cut.
//!   After a discontinuity they are recoded and the keyframe and its trailing
//!   pictures remuxed (upstream's `hybrid_recode_cra_segment`). Where a copy
//!   *ends* on such a keyframe, its leading pictures are found in the packets
//!   and recoded as the window before it.
//! - The windows are computed from the frames' real dependencies — the packet
//!   order the file actually has — not from a nominal GOP length.
//!
//! What differs from upstream, and why:
//!
//! - **One pass, over pipes** (ADR-0010). Upstream muxes in-process with
//!   `PyAV`. Here the seam encoder is a sidecar process writing NUT, like every
//!   other producer, and its packets join the copied ones in the router.
//! - **The encoder must match the source or the seam is refused** (#44,
//!   ADR-0003). Upstream uses `libx264`/`libx265` and sets `sps-id=3` to avoid
//!   an id collision; neither ships here, and a hardware encoder cannot move
//!   its SPS id. So the parameter sets are sent **in-band** instead: the
//!   seam's own before its first picture, and the source's again before the
//!   first copied keyframe after it, exactly as `h264_mp4toannexb` does for
//!   every keyframe upstream. The container is told parameter sets may
//!   change in-band (`avc3` / `hev1`).
//! - **The join is validated before a seam packet is written** (#44):
//!   profile, chroma, bit depth and level, from both sides' SPS.

use super::plan::{Cause, SeamWindow, Segment};
use super::sps::{annex_b_units, is_annex_b};
use crate::capability::VideoCodec;

/// One piece of a smart-cut clip, in source ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    /// Encoded: the pictures shown in `from..to`.
    Recode { from: i64, to: i64 },
    /// Copied: from the keyframe at `from`, the pictures shown before `to`.
    /// `to` is a keyframe or the clip's end; where `leading` is set, the
    /// pictures shown just before `to` are decoded after it, and the recode
    /// that follows starts where they do.
    Remux { from: i64, to: i64, leading: bool },
}

/// The pieces of a smart-cut `segment` of `from..to` (its source ticks), from
/// the plan's causes and windows.
#[must_use]
pub fn pieces(segment: &Segment, from: i64, to: i64) -> Vec<Piece> {
    let head = segment
        .causes
        .iter()
        .any(|c| matches!(c, Cause::InPointNotKeyframe { .. }));
    let tail_mid_gop = segment
        .causes
        .iter()
        .any(|c| matches!(c, Cause::OutPointNotKeyframe { .. }));
    let tail_open = segment
        .causes
        .iter()
        .any(|c| matches!(c, Cause::OpenGopAtOutPoint { .. }));
    let windows: &[SeamWindow] = &segment.windows;
    let copy_from = if head {
        windows.first().map_or(to, |w| w.to)
    } else {
        from
    };
    let copy_to = if tail_mid_gop {
        windows.last().map_or(from, |w| w.from)
    } else {
        to
    };
    if copy_from >= copy_to || (copy_from >= to) {
        return vec![Piece::Recode { from, to }];
    }
    let mut pieces = Vec::with_capacity(3);
    if head {
        pieces.push(Piece::Recode {
            from,
            to: copy_from,
        });
    }
    let tail = tail_mid_gop || tail_open;
    pieces.push(Piece::Remux {
        from: copy_from,
        to: copy_to,
        leading: tail,
    });
    if tail {
        // Where the recode starts is known once the copy has read its end:
        // the placeholder is the copy's end, moved back to the first leading
        // picture if there is one.
        pieces.push(Piece::Recode { from: copy_to, to });
    }
    pieces
}

/// How NAL units are delimited in the output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    /// Big-endian length prefixes of this many bytes (`avcC`, `hvcC`).
    Length(usize),
    /// Start codes.
    AnnexB,
}

/// Puts a seam's packets and the copied packets around it into one stream a
/// decoder can follow across both joins.
#[derive(Debug)]
pub struct Stitch {
    codec: VideoCodec,
    framing: Framing,
    /// The source's parameter sets, framed for the output.
    source_sets: Vec<u8>,
    /// The seam's, framed for the output, while a seam is being written.
    seam_sets: Option<Vec<u8>>,
    /// The last packet written came from an encoder: the next copied
    /// keyframe must carry the source's parameter sets again.
    after_seam: bool,
    /// Whether any seam was written: the container must allow parameter
    /// sets in-band.
    stitched: bool,
}

fn frame_units(framing: Framing, units: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for unit in units {
        match framing {
            Framing::AnnexB => out.extend_from_slice(&[0, 0, 0, 1]),
            Framing::Length(size) => {
                let length = unit.len().to_be_bytes();
                out.extend_from_slice(
                    length
                        .get(length.len().saturating_sub(size)..)
                        .unwrap_or_default(),
                );
            }
        }
        out.extend_from_slice(unit);
    }
    out
}

/// The parameter-set NAL units of an `avcC`/`hvcC` record or an Annex B
/// configuration, in order.
fn parameter_units(codec: VideoCodec, record: &[u8]) -> Vec<Vec<u8>> {
    if is_annex_b(record) {
        return annex_b_units(record)
            .into_iter()
            .filter(|unit| is_parameter_set(codec, unit))
            .map(<[u8]>::to_vec)
            .collect();
    }
    let mut units = Vec::new();
    match codec {
        VideoCodec::H264 => {
            let mut at = 5;
            for count_mask in [0x1F_u8, 0xFF] {
                let Some(&count) = record.get(at) else {
                    return units;
                };
                at += 1;
                for _ in 0..(count & count_mask) {
                    let (Some(&high), Some(&low)) = (record.get(at), record.get(at + 1)) else {
                        return units;
                    };
                    let length = usize::from(u16::from_be_bytes([high, low]));
                    if let Some(unit) = record.get(at + 2..at + 2 + length) {
                        units.push(unit.to_vec());
                    }
                    at += 2 + length;
                }
            }
        }
        VideoCodec::Hevc => {
            let Some(&arrays) = record.get(22) else {
                return units;
            };
            let mut at = 23;
            for _ in 0..arrays {
                let (Some(&high), Some(&low)) = (record.get(at + 1), record.get(at + 2)) else {
                    return units;
                };
                let count = u16::from_be_bytes([high, low]);
                at += 3;
                for _ in 0..count {
                    let (Some(&high), Some(&low)) = (record.get(at), record.get(at + 1)) else {
                        return units;
                    };
                    let length = usize::from(u16::from_be_bytes([high, low]));
                    if let Some(unit) = record
                        .get(at + 2..at + 2 + length)
                        .filter(|unit| is_parameter_set(codec, unit))
                    {
                        units.push(unit.to_vec());
                    }
                    at += 2 + length;
                }
            }
        }
        VideoCodec::Av1 => {
            // av1C: four bytes of header, then the configuration OBUs.
            if record.first() == Some(&0x81) {
                if let Some(obus) = record.get(4..).filter(|o| !o.is_empty()) {
                    units.push(obus.to_vec());
                }
            } else if !record.is_empty() {
                units.push(record.to_vec());
            }
        }
        VideoCodec::Vp9 => {}
    }
    units
}

/// Whether a NAL unit is a VPS, SPS or PPS.
#[must_use]
pub fn is_parameter_set(codec: VideoCodec, unit: &[u8]) -> bool {
    let Some(&first) = unit.first() else {
        return false;
    };
    match codec {
        VideoCodec::H264 => matches!(first & 0x1F, 7 | 8),
        VideoCodec::Hevc => matches!((first >> 1) & 0x3F, 32..=34),
        VideoCodec::Vp9 | VideoCodec::Av1 => false,
    }
}

/// Whether an AV1 temporal unit already starts with a sequence header OBU.
fn starts_with_sequence_header(data: &[u8]) -> bool {
    let mut at = 0;
    // A temporal delimiter may come first.
    while let Some(&header) = data.get(at) {
        let kind = (header >> 3) & 0x0F;
        if kind == 1 {
            return true;
        }
        if kind != 2 || header & 0x02 == 0 {
            return false;
        }
        at += 1 + usize::from(header & 0x04 != 0);
        let mut size: usize = 0;
        let mut shift = 0;
        loop {
            let Some(&byte) = data.get(at) else {
                return false;
            };
            size |= usize::from(byte & 0x7F) << shift;
            at += 1;
            shift += 7;
            if byte & 0x80 == 0 || shift > 56 {
                break;
            }
        }
        at += size;
    }
    false
}

impl Stitch {
    /// A stitch for a stream whose codec configuration is `extradata`.
    #[must_use]
    pub fn new(codec: VideoCodec, extradata: &[u8]) -> Self {
        let framing = match codec {
            VideoCodec::H264 if !is_annex_b(extradata) => {
                Framing::Length(usize::from(extradata.get(4).map_or(3, |b| b & 0x03)) + 1)
            }
            VideoCodec::Hevc if !is_annex_b(extradata) => {
                Framing::Length(usize::from(extradata.get(21).map_or(3, |b| b & 0x03)) + 1)
            }
            _ => Framing::AnnexB,
        };
        let units = parameter_units(codec, extradata);
        let refs: Vec<&[u8]> = units.iter().map(Vec::as_slice).collect();
        let source_sets = match codec {
            VideoCodec::H264 | VideoCodec::Hevc => frame_units(framing, &refs),
            VideoCodec::Av1 => refs.concat(),
            VideoCodec::Vp9 => Vec::new(),
        };
        Self {
            codec,
            framing,
            source_sets,
            seam_sets: None,
            after_seam: false,
            stitched: false,
        }
    }

    /// A seam starts, made by an encoder whose configuration is `extradata`.
    pub fn begin_seam(&mut self, extradata: &[u8]) {
        let units = parameter_units(self.codec, extradata);
        let refs: Vec<&[u8]> = units.iter().map(Vec::as_slice).collect();
        self.seam_sets = Some(match self.codec {
            VideoCodec::H264 | VideoCodec::Hevc => frame_units(self.framing, &refs),
            VideoCodec::Av1 => refs.concat(),
            VideoCodec::Vp9 => Vec::new(),
        });
        self.stitched = true;
    }

    /// A packet of the seam, framed for the output. The first carries the
    /// seam's parameter sets.
    pub fn seam(&mut self, data: &[u8]) -> Vec<u8> {
        self.after_seam = true;
        let sets = self.seam_sets.take().unwrap_or_default();
        match self.codec {
            VideoCodec::H264 | VideoCodec::Hevc => {
                let body = if is_annex_b(data) {
                    frame_units(self.framing, &annex_b_units(data))
                } else {
                    data.to_vec()
                };
                [sets, body].concat()
            }
            VideoCodec::Av1 if !starts_with_sequence_header(data) => [sets, data.to_vec()].concat(),
            VideoCodec::Av1 | VideoCodec::Vp9 => data.to_vec(),
        }
    }

    /// A copied packet. The first keyframe after a seam carries the source's
    /// parameter sets again; every other packet is untouched.
    pub fn copied(&mut self, data: Vec<u8>, key: bool) -> Vec<u8> {
        if !(self.after_seam && key) {
            return data;
        }
        self.after_seam = false;
        match self.codec {
            VideoCodec::H264 | VideoCodec::Hevc => [self.source_sets.clone(), data].concat(),
            VideoCodec::Av1 if !starts_with_sequence_header(&data) => {
                [self.source_sets.clone(), data].concat()
            }
            VideoCodec::Av1 | VideoCodec::Vp9 => data,
        }
    }

    /// Whether any seam was written, so the container must allow parameter
    /// sets to change in-band.
    #[must_use]
    pub fn stitched(&self) -> bool {
        self.stitched
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::export::plan::Media;
    use crate::tier::{ExportTier, SeamReason};

    /// The corpus's High 3.0 `avcC`: one SPS, one PPS, four-byte lengths.
    const AVCC: [u8; 47] = [
        0x01, 0x64, 0x00, 0x1E, 0xFF, 0xE1, 0x00, 0x1A, 0x67, 0x64, 0x00, 0x1E, 0xAC, 0xD9, 0x40,
        0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C,
        0x0F, 0x16, 0x2D, 0x96, 0x01, 0x00, 0x06, 0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0, 0xFD, 0xF8,
        0xF8, 0x00,
    ];

    fn segment(causes: Vec<Cause>, windows: Vec<SeamWindow>) -> Segment {
        Segment {
            media: Media::Video,
            start: 0,
            length: 100,
            sources: Vec::new(),
            tier: ExportTier::SmartCut {
                reason: SeamReason::InPointNotKeyframeAligned,
            },
            causes,
            windows,
            decline: None,
            alternative: None,
            encoder: None,
        }
    }

    #[test]
    fn a_clip_is_recoded_to_its_first_keyframe_and_copied_from_there() {
        let head = segment(
            vec![Cause::InPointNotKeyframe {
                keyframe_before: Some(0),
                keyframe_after: Some(30),
            }],
            vec![SeamWindow { from: 12, to: 30 }],
        );
        assert_eq!(
            pieces(&head, 12, 90),
            vec![
                Piece::Recode { from: 12, to: 30 },
                Piece::Remux {
                    from: 30,
                    to: 90,
                    leading: false
                }
            ]
        );
    }

    #[test]
    fn both_ends_off_keyframes_recode_both_windows() {
        let both = segment(
            vec![
                Cause::InPointNotKeyframe {
                    keyframe_before: Some(0),
                    keyframe_after: Some(30),
                },
                Cause::OutPointNotKeyframe {
                    keyframe_before: Some(60),
                    keyframe_after: Some(90),
                },
            ],
            vec![
                SeamWindow { from: 12, to: 30 },
                SeamWindow { from: 60, to: 75 },
            ],
        );
        assert_eq!(
            pieces(&both, 12, 75),
            vec![
                Piece::Recode { from: 12, to: 30 },
                Piece::Remux {
                    from: 30,
                    to: 60,
                    leading: true
                },
                Piece::Recode { from: 60, to: 75 },
            ]
        );
    }

    #[test]
    fn a_clip_inside_one_gop_is_recoded_whole() {
        let inside = segment(
            vec![
                Cause::InPointNotKeyframe {
                    keyframe_before: Some(0),
                    keyframe_after: Some(30),
                },
                Cause::OutPointNotKeyframe {
                    keyframe_before: Some(0),
                    keyframe_after: Some(30),
                },
            ],
            vec![SeamWindow { from: 5, to: 20 }],
        );
        assert_eq!(
            pieces(&inside, 5, 20),
            vec![Piece::Recode { from: 5, to: 20 }]
        );
    }

    #[test]
    fn the_source_sets_come_back_before_the_first_copied_keyframe_after_a_seam() {
        let mut stitch = Stitch::new(VideoCodec::H264, &AVCC);
        // A copied packet before any seam is untouched.
        let slice = vec![0, 0, 0, 3, 0x65, 0x88, 0x84];
        assert_eq!(stitch.copied(slice.clone(), true), slice);
        // The seam, from an encoder writing Annex B with its own parameter
        // sets, is framed with four-byte lengths and carries its sets first.
        let seam_sets = [
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1E, 0xAA, 0, 0, 0, 1, 0x68, 0xCE,
        ];
        stitch.begin_seam(&seam_sets);
        let seam = stitch.seam(&[0, 0, 0, 1, 0x65, 0x11, 0x22]);
        assert_eq!(
            seam,
            vec![
                0, 0, 0, 5, 0x67, 0x64, 0x00, 0x1E, 0xAA, 0, 0, 0, 2, 0x68, 0xCE, 0, 0, 0, 3, 0x65,
                0x11, 0x22
            ]
        );
        assert_eq!(
            stitch.seam(&[0, 0, 1, 0x41, 0x9A]),
            vec![0, 0, 0, 2, 0x41, 0x9A]
        );
        // A copied non-keyframe cannot follow a seam in a valid plan, but is
        // passed through untouched if it does; the keyframe gets the sets.
        let after = stitch.copied(slice.clone(), true);
        assert_eq!(&after[..4], &[0, 0, 0, 26]);
        assert_eq!(&after[4..30], &AVCC[8..34]);
        assert_eq!(&after[30..34], &[0, 0, 0, 6]);
        assert!(after.ends_with(&slice));
        // Only once.
        assert_eq!(stitch.copied(slice.clone(), true), slice);
        assert!(stitch.stitched());
    }
}
