//! Reading a sequence parameter set far enough to know whether two streams
//! can share a decoder (#44): profile, level, chroma format and bit depth.
//!
//! The configuration of a copied stream arrives as a container record
//! (`avcC`, `hvcC`, `av1C`); an encoder writing NUT hands over the same
//! parameter sets in Annex B form. Both are reduced to their sequence
//! parameter set, and the same parser reads either, so the comparison cannot
//! depend on the packaging.
//!
//! The input came from a file or another process: every read is bounded and
//! a short or malformed record is `None`, never a panic.

/// What a sequence parameter set says about the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequence {
    pub profile: u8,
    pub level: u8,
    /// `chroma_format_idc`: 1 is 4:2:0. `None` where not read.
    pub chroma: Option<u8>,
    pub bit_depth: Option<u8>,
}

/// Reads bits, most significant first, from a NAL unit's payload with its
/// emulation-prevention bytes removed.
struct Bits {
    bytes: Vec<u8>,
    at: usize,
}

impl Bits {
    fn new(nal: &[u8]) -> Self {
        let mut bytes = Vec::with_capacity(nal.len());
        let mut zeros = 0;
        for &byte in nal {
            if zeros >= 2 && byte == 3 {
                zeros = 0;
                continue;
            }
            zeros = if byte == 0 { zeros + 1 } else { 0 };
            bytes.push(byte);
        }
        Self { bytes, at: 0 }
    }

    fn bit(&mut self) -> Option<u32> {
        let byte = *self.bytes.get(self.at >> 3)?;
        let shift = 7 - (self.at & 7);
        self.at += 1;
        Some(u32::from((byte >> shift) & 1))
    }

    fn bits(&mut self, count: u32) -> Option<u32> {
        let mut value = 0_u32;
        for _ in 0..count.min(32) {
            value = (value << 1) | self.bit()?;
        }
        Some(value)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        self.at = self.at.checked_add(count)?;
        (self.at <= self.bytes.len() * 8).then_some(())
    }

    /// Unsigned Exp-Golomb.
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1_u32 << zeros) - 1 + self.bits(zeros)?)
    }
}

fn byte(value: u32) -> Option<u8> {
    u8::try_from(value).ok()
}

/// An H.264 SPS NAL unit (header byte included).
fn h264(nal: &[u8]) -> Option<Sequence> {
    let mut bits = Bits::new(nal.get(1..)?);
    let profile = bits.bits(8)?;
    bits.skip(8)?; // constraint flags
    let level = bits.bits(8)?;
    bits.ue()?; // seq_parameter_set_id
    let (chroma, bit_depth) = if matches!(
        profile,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        let chroma = bits.ue()?;
        if chroma == 3 {
            bits.skip(1)?;
        }
        let depth = bits.ue()? + 8;
        (byte(chroma)?, byte(depth)?)
    } else {
        (1, 8)
    };
    Some(Sequence {
        profile: byte(profile)?,
        level: byte(level)?,
        chroma: Some(chroma),
        bit_depth: Some(bit_depth),
    })
}

/// An HEVC SPS NAL unit (two header bytes included).
fn hevc(nal: &[u8]) -> Option<Sequence> {
    let mut bits = Bits::new(nal.get(2..)?);
    bits.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = bits.bits(3)?;
    bits.skip(1)?; // temporal_id_nesting
    bits.skip(3)?; // profile_space, tier
    let profile = bits.bits(5)?;
    bits.skip(32 + 48)?; // compatibility and constraint flags
    let level = bits.bits(8)?;
    let mut profile_present = Vec::new();
    let mut level_present = Vec::new();
    for _ in 0..sub_layers {
        profile_present.push(bits.bit()? == 1);
        level_present.push(bits.bit()? == 1);
    }
    if sub_layers > 0 {
        for _ in sub_layers..8 {
            bits.skip(2)?;
        }
    }
    for (profile, level) in profile_present.into_iter().zip(level_present) {
        if profile {
            bits.skip(88)?;
        }
        if level {
            bits.skip(8)?;
        }
    }
    bits.ue()?; // sps_seq_parameter_set_id
    let chroma = bits.ue()?;
    if chroma == 3 {
        bits.skip(1)?;
    }
    bits.ue()?; // width
    bits.ue()?; // height
    if bits.bit()? == 1 {
        for _ in 0..4 {
            bits.ue()?; // conformance window
        }
    }
    let depth = bits.ue()? + 8;
    Some(Sequence {
        profile: byte(profile)?,
        level: byte(level)?,
        chroma: Some(byte(chroma)?),
        bit_depth: Some(byte(depth)?),
    })
}

/// The NAL units of an Annex B byte stream.
fn annex_b(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream.get(i..i + 3) == Some(&[0, 0, 1]) {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .filter_map(|(n, &start)| {
            let end = starts.get(n + 1).map_or(stream.len(), |next| {
                // The next start code, less its leading zero, if any.
                let mut end = next - 3;
                while end > start && stream.get(end - 1) == Some(&0) {
                    end -= 1;
                }
                end
            });
            stream.get(start..end)
        })
        .collect()
}

fn is_annex_b(record: &[u8]) -> bool {
    record.starts_with(&[0, 0, 1]) || record.starts_with(&[0, 0, 0, 1])
}

/// The SPS of an `avcC` record.
fn avcc_sps(record: &[u8]) -> Option<&[u8]> {
    if record.get(5)?.trailing_zeros() >= 5 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([*record.get(6)?, *record.get(7)?]));
    record.get(8..8 + length)
}

/// The SPS of an `hvcC` record: in the array of NAL type 33.
fn hvcc_sps(record: &[u8]) -> Option<&[u8]> {
    let arrays = *record.get(22)?;
    let mut at = 23;
    for _ in 0..arrays {
        let kind = record.get(at)? & 0x3F;
        let count = usize::from(u16::from_be_bytes([
            *record.get(at + 1)?,
            *record.get(at + 2)?,
        ]));
        at += 3;
        for _ in 0..count {
            let length = usize::from(u16::from_be_bytes([*record.get(at)?, *record.get(at + 1)?]));
            let nal = record.get(at + 2..at + 2 + length)?;
            if kind == 33 {
                return Some(nal);
            }
            at += 2 + length;
        }
    }
    None
}

/// The H.264 sequence an `avcC` record or an Annex B stream carries.
#[must_use]
pub fn h264_sequence(record: &[u8]) -> Option<Sequence> {
    if is_annex_b(record) {
        annex_b(record)
            .into_iter()
            .find(|nal| nal.first().is_some_and(|b| b & 0x1F == 7))
            .and_then(h264)
    } else {
        avcc_sps(record).and_then(h264)
    }
}

/// The HEVC sequence an `hvcC` record or an Annex B stream carries.
#[must_use]
pub fn hevc_sequence(record: &[u8]) -> Option<Sequence> {
    if is_annex_b(record) {
        annex_b(record)
            .into_iter()
            .find(|nal| nal.first().is_some_and(|b| (b >> 1) & 0x3F == 33))
            .and_then(hevc)
    } else {
        hvcc_sps(record).and_then(hevc)
    }
}

/// The AV1 sequence of an `av1C` record or a sequence header OBU: profile
/// and level only; its colour configuration is not reached.
#[must_use]
pub fn av1_sequence(record: &[u8]) -> Option<Sequence> {
    let first = *record.first()?;
    if first == 0x81 {
        // av1C: marker and version, then profile and level.
        let second = *record.get(1)?;
        return Some(Sequence {
            profile: second >> 5,
            level: second & 0x1F,
            chroma: None,
            bit_depth: None,
        });
    }
    // A sequence header OBU (type 1), optionally with a size field.
    if (first >> 3) & 0x0F != 1 {
        return None;
    }
    let mut at = 1;
    if first & 0x02 != 0 {
        while record.get(at)? & 0x80 != 0 {
            at += 1;
        }
        at += 1;
    }
    let mut bits = Bits {
        bytes: record.get(at..)?.to_vec(),
        at: 0,
    };
    let profile = bits.bits(3)?;
    bits.skip(1)?; // still_picture
    let reduced = bits.bit()? == 1;
    let level = if reduced {
        bits.bits(5)?
    } else {
        if bits.bit()? == 1 {
            // Timing information: not written by the encoders Blinkify runs.
            return None;
        }
        bits.skip(1)?; // initial_display_delay_present
        bits.skip(5)?; // operating_points_cnt_minus_1: the first is read
        bits.skip(12)?; // operating_point_idc
        bits.bits(5)?
    };
    Some(Sequence {
        profile: byte(profile)?,
        level: byte(level)?,
        chroma: None,
        bit_depth: None,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// A real High-profile level 3.0 SPS (x264, 640x360, 4:2:0, 8-bit).
    const SPS: [u8; 26] = [
        0x67, 0x64, 0x00, 0x1E, 0xAC, 0xD9, 0x40, 0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00, 0x00, 0x03,
        0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C, 0x0F, 0x16, 0x2D, 0x96,
    ];

    #[test]
    fn an_sps_reads_the_same_from_a_record_and_from_annex_b() {
        let mut record = vec![1, 0x64, 0, 0x1E, 0xFF, 0xE1, 0, 26];
        record.extend_from_slice(&SPS);
        record.extend_from_slice(&[1, 0, 1, 0x68]);
        let mut stream = vec![0, 0, 0, 1];
        stream.extend_from_slice(&SPS);
        stream.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xEB]);
        let from_record = h264_sequence(&record).expect("record");
        assert_eq!(from_record, h264_sequence(&stream).expect("annex b"));
        assert_eq!(from_record.profile, 100);
        assert_eq!(from_record.level, 30);
        assert_eq!(from_record.chroma, Some(1));
        assert_eq!(from_record.bit_depth, Some(8));
    }

    #[test]
    fn a_truncated_record_is_none_not_a_panic() {
        for cut in 0..SPS.len() {
            let _ = h264(&SPS[..cut]);
            let _ = hevc(&SPS[..cut]);
            let _ = av1_sequence(&SPS[..cut]);
            let _ = hevc_sequence(&SPS[..cut]);
        }
        assert_eq!(h264_sequence(&[]), None);
    }

    #[test]
    fn emulation_prevention_bytes_are_removed() {
        let bits = Bits::new(&[0, 0, 3, 1, 0, 0, 3, 0]);
        assert_eq!(bits.bytes, vec![0, 0, 1, 0, 0, 0]);
    }
}
