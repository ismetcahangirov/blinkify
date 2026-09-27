//! Cropping an H.264 or HEVC stream through the codec's own cropping window,
//! without changing a picture byte (#126). **A research prototype: nothing in
//! the planner or the executor calls it.** ADR-0020 records what players did
//! with its output, and why the planner never chooses it: many show the
//! wrong rectangle.
//!
//! Both codecs code whole macroblocks or coding blocks, so a picture whose
//! size is not a multiple of the block is coded larger and the sequence
//! parameter set names the part to show: H.264's `frame_cropping_flag` and
//! `frame_crop_{left,right,top,bottom}_offset`, HEVC's
//! `conformance_window_flag` and `conf_win_*_offset`. It is how 1080 rows
//! travel in 1088. Rewriting those offsets changes the shown rectangle and
//! nothing else: the slices still code the whole picture, which is still
//! decoded whole, and the file is no smaller.
//!
//! The rewrite reads the SPS up to the window with the parser of
//! [`sps`](super::sps), writes the new window, copies every remaining bit of
//! the payload unchanged, re-aligns the trailing bits and re-applies
//! emulation prevention. It is applied where the SPS travels: out of band in
//! the `avcC`/`hvcC` record (or an Annex B configuration), and in band in
//! any packet that repeats it.
//!
//! The constraints are the codec's, and each is an error here rather than a
//! rounded result:
//!
//! - offsets are counted in chroma units — `CropUnitX`/`CropUnitY` in H.264
//!   (doubled vertically for field-coded streams), `SubWidthC`/`SubHeightC`
//!   in HEVC — so a 4:2:0 window starts and ends on even luma samples;
//! - the window can only shrink what is shown, never reach outside it;
//! - every SPS in the stream must describe the same picture, because one
//!   window is written into all of them.
//!
//! The rectangle is in the **stored** orientation of the coded picture. A
//! phone clip stored landscape with a 90° display matrix is cropped in its
//! landscape coordinates; mapping from what the user sees is the caller's.
//!
//! Every input came from a file or another process: reads are bounded and a
//! malformed parameter set is an error, never a panic.

use thiserror::Error;

use super::nut::StreamHeader;
use super::sps::{Bits, annex_b_units, is_annex_b, profile_tier_level};
use crate::capability::VideoCodec;

/// A rectangle in luma samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What a sequence parameter set says about the picture's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// The coded picture, in luma samples: whole macroblocks or coding
    /// blocks.
    pub coded_width: u32,
    pub coded_height: u32,
    /// The part of the coded picture a decoder shows, in its coordinates.
    pub shown: Rect,
    /// The step of the window, in luma samples, horizontally and vertically.
    pub unit_x: u32,
    pub unit_y: u32,
}

/// Why a window cannot be written.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WindowError {
    #[error("the stream's sequence parameter set could not be read")]
    Unreadable,
    #[error("{0:?} has no cropping window in its parameter sets")]
    Unsupported(VideoCodec),
    #[error("the crop must start and end on multiples of {unit_x} × {unit_y} luma samples")]
    Misaligned { unit_x: u32, unit_y: u32 },
    #[error("the crop is empty or reaches outside the picture that is shown")]
    OutsidePicture,
    #[error("the stream's sequence parameter sets describe different pictures")]
    ParameterSetsChange,
    #[error("a rewritten parameter set does not fit its length field")]
    TooLong,
}

/// An SPS read as far as its window.
struct Parsed {
    /// NAL header bytes: 1 for H.264, 2 for HEVC.
    header: usize,
    /// The payload, emulation prevention removed.
    rbsp: Vec<u8>,
    geometry: Geometry,
    /// Bit positions in `rbsp`: the window's flag, and just past its
    /// offsets.
    window_from: usize,
    window_to: usize,
}

/// Luma samples per chroma sample, horizontally and vertically, for a
/// `ChromaArrayType` (0 when the colour planes are coded separately).
fn subsampling(chroma_array_type: u32) -> Option<(u32, u32)> {
    match chroma_array_type {
        0 | 3 => Some((1, 1)),
        1 => Some((2, 2)),
        2 => Some((2, 1)),
        _ => None,
    }
}

fn geometry(
    coded_width: u32,
    coded_height: u32,
    offsets: [u32; 4],
    unit_x: u32,
    unit_y: u32,
) -> Option<Geometry> {
    let [left, right, top, bottom] = offsets;
    let left = left.checked_mul(unit_x)?;
    let right = right.checked_mul(unit_x)?;
    let top = top.checked_mul(unit_y)?;
    let bottom = bottom.checked_mul(unit_y)?;
    let width = coded_width.checked_sub(left.checked_add(right)?)?;
    let height = coded_height.checked_sub(top.checked_add(bottom)?)?;
    (width > 0 && height > 0).then_some(Geometry {
        coded_width,
        coded_height,
        shown: Rect {
            x: left,
            y: top,
            width,
            height,
        },
        unit_x,
        unit_y,
    })
}

fn window_offsets(bits: &mut Bits) -> Option<[u32; 4]> {
    if bits.bit()? == 1 {
        Some([bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?])
    } else {
        Some([0; 4])
    }
}

/// H.264 profiles whose SPS carries the chroma format and bit depths.
const H264_HIGH: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn skip_scaling_list(bits: &mut Bits, size: usize) -> Option<()> {
    let mut last = 8_i64;
    let mut next = 8_i64;
    for _ in 0..size {
        if next != 0 {
            next = (last + bits.se()? + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Some(())
}

/// An H.264 SPS NAL unit, header byte included.
fn parse_h264(nal: &[u8]) -> Option<Parsed> {
    if nal.first()? & 0x1F != 7 {
        return None;
    }
    let mut bits = Bits::new(nal.get(1..)?);
    let profile = bits.bits(8)?;
    bits.skip(16)?; // constraint flags, level
    bits.ue()?; // seq_parameter_set_id
    let mut chroma = 1;
    let mut separate = false;
    if H264_HIGH.contains(&profile) {
        chroma = bits.ue()?;
        if chroma == 3 {
            separate = bits.bit()? == 1;
        }
        bits.ue()?; // bit_depth_luma_minus8
        bits.ue()?; // bit_depth_chroma_minus8
        bits.skip(1)?; // qpprime_y_zero_transform_bypass_flag
        if bits.bit()? == 1 {
            let lists = if chroma == 3 { 12 } else { 8 };
            for list in 0..lists {
                if bits.bit()? == 1 {
                    skip_scaling_list(&mut bits, if list < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    bits.ue()?; // log2_max_frame_num_minus4
    match bits.ue()? {
        0 => {
            bits.ue()?; // log2_max_pic_order_cnt_lsb_minus4
        }
        1 => {
            bits.skip(1)?; // delta_pic_order_always_zero_flag
            bits.se()?; // offset_for_non_ref_pic
            bits.se()?; // offset_for_top_to_bottom_field
            let cycle = bits.ue()?;
            if cycle > 255 {
                return None;
            }
            for _ in 0..cycle {
                bits.se()?;
            }
        }
        2 => {}
        _ => return None,
    }
    bits.ue()?; // max_num_ref_frames
    bits.skip(1)?; // gaps_in_frame_num_value_allowed_flag
    let width_in_mbs = bits.ue()?.checked_add(1)?;
    let height_in_map_units = bits.ue()?.checked_add(1)?;
    let frame_mbs_only = bits.bit()? == 1;
    if !frame_mbs_only {
        bits.skip(1)?; // mb_adaptive_frame_field_flag
    }
    bits.skip(1)?; // direct_8x8_inference_flag
    let window_from = bits.position();
    let offsets = window_offsets(&mut bits)?;
    let window_to = bits.position();
    let fields = if frame_mbs_only { 1 } else { 2 };
    let (sub_width, sub_height) = subsampling(if separate { 0 } else { chroma })?;
    let geometry = geometry(
        width_in_mbs.checked_mul(16)?,
        height_in_map_units.checked_mul(16)?.checked_mul(fields)?,
        offsets,
        sub_width,
        sub_height * fields,
    )?;
    Some(Parsed {
        header: 1,
        rbsp: bits.rbsp().to_vec(),
        geometry,
        window_from,
        window_to,
    })
}

/// An HEVC SPS NAL unit, both header bytes included.
fn parse_hevc(nal: &[u8]) -> Option<Parsed> {
    if (nal.first()? >> 1) & 0x3F != 33 {
        return None;
    }
    let mut bits = Bits::new(nal.get(2..)?);
    bits.skip(4)?; // sps_video_parameter_set_id
    let sub_layers = bits.bits(3)?;
    bits.skip(1)?; // sps_temporal_id_nesting_flag
    profile_tier_level(&mut bits, sub_layers)?;
    bits.ue()?; // sps_seq_parameter_set_id
    let chroma = bits.ue()?;
    let separate = chroma == 3 && bits.bit()? == 1;
    let width = bits.ue()?;
    let height = bits.ue()?;
    let window_from = bits.position();
    let offsets = window_offsets(&mut bits)?;
    let window_to = bits.position();
    let (unit_x, unit_y) = subsampling(if separate { 0 } else { chroma })?;
    let geometry = geometry(width, height, offsets, unit_x, unit_y)?;
    Some(Parsed {
        header: 2,
        rbsp: bits.rbsp().to_vec(),
        geometry,
        window_from,
        window_to,
    })
}

fn parse(codec: VideoCodec, nal: &[u8]) -> Result<Parsed, WindowError> {
    match codec {
        VideoCodec::H264 => parse_h264(nal),
        VideoCodec::Hevc => parse_hevc(nal),
        VideoCodec::Vp9 | VideoCodec::Av1 => return Err(WindowError::Unsupported(codec)),
    }
    .ok_or(WindowError::Unreadable)
}

/// The geometry an H.264 or HEVC SPS NAL unit describes.
///
/// # Errors
///
/// The unit is not a readable SPS of `codec`.
pub fn sps_geometry(codec: VideoCodec, nal: &[u8]) -> Result<Geometry, WindowError> {
    parse(codec, nal).map(|parsed| parsed.geometry)
}

/// The window offsets, in the codec's units, that show `rect` of the picture
/// `geometry` currently shows.
///
/// # Errors
///
/// `rect` is empty or reaches outside the picture shown, or does not start
/// and end on the codec's units.
pub fn window_for(geometry: &Geometry, rect: Rect) -> Result<[u32; 4], WindowError> {
    let shown = geometry.shown;
    let fits_x = rect
        .x
        .checked_add(rect.width)
        .is_some_and(|end| end <= shown.width);
    let fits_y = rect
        .y
        .checked_add(rect.height)
        .is_some_and(|end| end <= shown.height);
    if rect.width == 0 || rect.height == 0 || !fits_x || !fits_y {
        return Err(WindowError::OutsidePicture);
    }
    let (unit_x, unit_y) = (geometry.unit_x, geometry.unit_y);
    let misaligned = WindowError::Misaligned { unit_x, unit_y };
    if [rect.x, rect.width].iter().any(|v| v % unit_x != 0)
        || [rect.y, rect.height].iter().any(|v| v % unit_y != 0)
    {
        return Err(misaligned);
    }
    // In the coded picture's coordinates; `fits_*` bounds every sum.
    let left = shown.x + rect.x;
    let top = shown.y + rect.y;
    let right = geometry.coded_width - left - rect.width;
    let bottom = geometry.coded_height - top - rect.height;
    let units = |value: u32, unit: u32| value.checked_div(unit).ok_or(WindowError::Unreadable);
    Ok([
        units(left, unit_x)?,
        units(right, unit_x)?,
        units(top, unit_y)?,
        units(bottom, unit_y)?,
    ])
}

/// Bits written most significant first.
#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    fn bit(&mut self, bit: bool) {
        if self.used.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if bit && let Some(last) = self.bytes.last_mut() {
            *last |= 0x80 >> (self.used % 8);
        }
        self.used += 1;
    }

    fn ue(&mut self, value: u32) {
        let code = u64::from(value) + 1;
        let length = 64 - code.leading_zeros();
        for _ in 1..length {
            self.bit(false);
        }
        for shift in (0..length).rev() {
            self.bit((code >> shift) & 1 == 1);
        }
    }
}

fn bit_at(bytes: &[u8], at: usize) -> Option<bool> {
    let byte = bytes.get(at >> 3)?;
    Some((byte >> (7 - (at & 7))) & 1 == 1)
}

/// Emulation prevention: a 3 wherever two zero bytes would be followed by a
/// byte of 3 or less.
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + (rbsp.len() >> 6) + 1);
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// The position of the RBSP stop bit: the payload's last one bit.
fn stop_bit(rbsp: &[u8]) -> Option<usize> {
    let (last, byte) = rbsp
        .iter()
        .enumerate()
        .rev()
        .find(|(_, byte)| **byte != 0)?;
    Some(last * 8 + 7 - usize::try_from(byte.trailing_zeros()).ok()?)
}

/// `nal` with its window replaced by `offsets`, every other bit unchanged.
fn rewrite(nal: &[u8], parsed: &Parsed, offsets: [u32; 4]) -> Result<Vec<u8>, WindowError> {
    let rbsp = &parsed.rbsp;
    let stop = stop_bit(rbsp).ok_or(WindowError::Unreadable)?;
    if stop < parsed.window_to {
        return Err(WindowError::Unreadable);
    }
    let mut out = BitWriter::default();
    for at in 0..parsed.window_from {
        out.bit(bit_at(rbsp, at).ok_or(WindowError::Unreadable)?);
    }
    if offsets == [0; 4] {
        out.bit(false);
    } else {
        out.bit(true);
        for offset in offsets {
            out.ue(offset);
        }
    }
    for at in parsed.window_to..=stop {
        out.bit(bit_at(rbsp, at).ok_or(WindowError::Unreadable)?);
    }
    let header = nal.get(..parsed.header).ok_or(WindowError::Unreadable)?;
    Ok([header, &escape(&out.bytes)].concat())
}

/// How NAL units are delimited in packets and in the configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    AnnexB,
    /// A big-endian length of this many bytes before each unit.
    Length(usize),
}

fn is_sps(codec: VideoCodec, nal: &[u8]) -> bool {
    nal.first().is_some_and(|&b| match codec {
        VideoCodec::H264 => b & 0x1F == 7,
        VideoCodec::Hevc => (b >> 1) & 0x3F == 33,
        VideoCodec::Vp9 | VideoCodec::Av1 => false,
    })
}

fn push_length(out: &mut Vec<u8>, length: usize, size: usize) -> Result<(), WindowError> {
    let bytes = length.to_be_bytes();
    let (high, low) = bytes.split_at(bytes.len().saturating_sub(size));
    if high.iter().any(|&b| b != 0) || low.len() != size {
        return Err(WindowError::TooLong);
    }
    out.extend_from_slice(low);
    Ok(())
}

fn read_u16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes([
        *bytes.get(at)?,
        *bytes.get(at + 1)?,
    ])))
}

/// A stream copied with a new cropping window: the rewritten configuration,
/// and the rewrite of every packet that repeats the SPS in band.
#[derive(Debug, Clone)]
pub struct CropWindow {
    codec: VideoCodec,
    framing: Framing,
    source: Geometry,
    offsets: [u32; 4],
    size: (u32, u32),
    extradata: Vec<u8>,
}

impl CropWindow {
    /// Show `rect` of the picture a stream configured by `extradata` (an
    /// `avcC`/`hvcC` record, or Annex B parameter sets) shows now.
    ///
    /// # Errors
    ///
    /// The codec has no window, the configuration has no readable SPS, its
    /// SPSs describe different pictures, or `rect` is not a window the codec
    /// can express; see [`WindowError`].
    pub fn new(codec: VideoCodec, extradata: &[u8], rect: Rect) -> Result<Self, WindowError> {
        if matches!(codec, VideoCodec::Vp9 | VideoCodec::Av1) {
            return Err(WindowError::Unsupported(codec));
        }
        let framing = if is_annex_b(extradata) {
            Framing::AnnexB
        } else {
            let at = if codec == VideoCodec::H264 { 4 } else { 21 };
            Framing::Length(usize::from(extradata.get(at).ok_or(WindowError::Unreadable)? & 3) + 1)
        };
        let first = Self::record_units(codec, extradata)?
            .into_iter()
            .find(|nal| is_sps(codec, nal))
            .ok_or(WindowError::Unreadable)?;
        let source = sps_geometry(codec, first)?;
        let offsets = window_for(&source, rect)?;
        let mut window = Self {
            codec,
            framing,
            source,
            offsets,
            size: (rect.width, rect.height),
            extradata: Vec::new(),
        };
        window.extradata = window.rewrite_record(extradata)?;
        Ok(window)
    }

    /// Every NAL unit of a configuration record, in order.
    fn record_units(codec: VideoCodec, record: &[u8]) -> Result<Vec<&[u8]>, WindowError> {
        if is_annex_b(record) {
            return Ok(annex_b_units(record));
        }
        let mut units = Vec::new();
        let unreadable = || WindowError::Unreadable;
        match codec {
            VideoCodec::H264 => {
                let mut at = 5;
                for mask in [0x1F_u8, 0xFF] {
                    let count = record.get(at).ok_or_else(unreadable)? & mask;
                    at += 1;
                    for _ in 0..count {
                        let length = read_u16(record, at).ok_or_else(unreadable)?;
                        units.push(record.get(at + 2..at + 2 + length).ok_or_else(unreadable)?);
                        at += 2 + length;
                    }
                }
            }
            VideoCodec::Hevc | VideoCodec::Vp9 | VideoCodec::Av1 => {
                let arrays = *record.get(22).ok_or_else(unreadable)?;
                let mut at = 23;
                for _ in 0..arrays {
                    let count = read_u16(record, at + 1).ok_or_else(unreadable)?;
                    at += 3;
                    for _ in 0..count {
                        let length = read_u16(record, at).ok_or_else(unreadable)?;
                        units.push(record.get(at + 2..at + 2 + length).ok_or_else(unreadable)?);
                        at += 2 + length;
                    }
                }
            }
        }
        Ok(units)
    }

    /// One SPS with the window, if it describes the source's picture.
    fn rewrite_sps(&self, nal: &[u8]) -> Result<Vec<u8>, WindowError> {
        let parsed = parse(self.codec, nal)?;
        if parsed.geometry != self.source {
            return Err(WindowError::ParameterSetsChange);
        }
        rewrite(nal, &parsed, self.offsets)
    }

    fn rewrite_unit(&self, nal: &[u8]) -> Result<Vec<u8>, WindowError> {
        if is_sps(self.codec, nal) {
            self.rewrite_sps(nal)
        } else {
            Ok(nal.to_vec())
        }
    }

    /// The configuration record with every SPS rewritten, every other byte
    /// kept.
    fn rewrite_record(&self, record: &[u8]) -> Result<Vec<u8>, WindowError> {
        let unreadable = || WindowError::Unreadable;
        if is_annex_b(record) {
            let mut out = Vec::with_capacity(record.len() + 8);
            for nal in annex_b_units(record) {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(&self.rewrite_unit(nal)?);
            }
            return Ok(out);
        }
        let mut out = Vec::with_capacity(record.len() + 8);
        match self.codec {
            VideoCodec::H264 => {
                // Version, profile, compatibility, level, length size; then
                // the SPS list. The PPS list and any High-profile extension
                // after it are copied as they are.
                out.extend_from_slice(record.get(..6).ok_or_else(unreadable)?);
                let count = record.get(5).ok_or_else(unreadable)? & 0x1F;
                let mut at = 6;
                for _ in 0..count {
                    let length = read_u16(record, at).ok_or_else(unreadable)?;
                    let nal = record.get(at + 2..at + 2 + length).ok_or_else(unreadable)?;
                    let sps = self.rewrite_unit(nal)?;
                    push_length(&mut out, sps.len(), 2)?;
                    out.extend_from_slice(&sps);
                    at += 2 + length;
                }
                out.extend_from_slice(record.get(at..).ok_or_else(unreadable)?);
            }
            VideoCodec::Hevc | VideoCodec::Vp9 | VideoCodec::Av1 => {
                out.extend_from_slice(record.get(..23).ok_or_else(unreadable)?);
                let arrays = *record.get(22).ok_or_else(unreadable)?;
                let mut at = 23;
                for _ in 0..arrays {
                    out.extend_from_slice(record.get(at..at + 3).ok_or_else(unreadable)?);
                    let count = read_u16(record, at + 1).ok_or_else(unreadable)?;
                    at += 3;
                    for _ in 0..count {
                        let length = read_u16(record, at).ok_or_else(unreadable)?;
                        let nal = record.get(at + 2..at + 2 + length).ok_or_else(unreadable)?;
                        let unit = self.rewrite_unit(nal)?;
                        push_length(&mut out, unit.len(), 2)?;
                        out.extend_from_slice(&unit);
                        at += 2 + length;
                    }
                }
                out.extend_from_slice(record.get(at..).ok_or_else(unreadable)?);
            }
        }
        Ok(out)
    }

    /// The configuration to write in place of the source's.
    pub fn extradata(&self) -> &[u8] {
        &self.extradata
    }

    /// The size a decoder that honours the window shows.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The source's geometry, before the window.
    pub fn source(&self) -> Geometry {
        self.source
    }

    /// The window's offsets, in the codec's units: left, right, top, bottom.
    pub fn offsets(&self) -> [u32; 4] {
        self.offsets
    }

    /// A NUT stream header for the cropped stream: the new configuration,
    /// and the window's size where the header states one — the muxer writes
    /// the container's picture size from it.
    pub fn header(&self, stream: &StreamHeader) -> StreamHeader {
        let mut header = stream.clone();
        header.extradata.clone_from(&self.extradata);
        if let Some(video) = header.video.as_mut() {
            video[0] = u64::from(self.size.0);
            video[1] = u64::from(self.size.1);
        }
        header
    }

    /// A packet with any SPS it carries in band rewritten. Every other NAL
    /// unit — every slice — is copied byte for byte.
    ///
    /// # Errors
    ///
    /// The packet is malformed, or carries an SPS describing a different
    /// picture from the configuration's: the parameter sets change
    /// mid-stream, and one window cannot mean the same rectangle in both.
    pub fn packet(&self, data: &[u8]) -> Result<Vec<u8>, WindowError> {
        match self.framing {
            Framing::AnnexB => {
                if !annex_b_units(data)
                    .iter()
                    .any(|nal| is_sps(self.codec, nal))
                {
                    return Ok(data.to_vec());
                }
                let mut out = Vec::with_capacity(data.len() + 8);
                for nal in annex_b_units(data) {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(&self.rewrite_unit(nal)?);
                }
                Ok(out)
            }
            Framing::Length(size) => {
                let mut out = Vec::with_capacity(data.len() + 8);
                let mut at = 0;
                while at < data.len() {
                    let field = data.get(at..at + size).ok_or(WindowError::Unreadable)?;
                    let length = field
                        .iter()
                        .fold(0_usize, |n, &b| (n << 8) | usize::from(b));
                    let nal = data
                        .get(at + size..at + size + length)
                        .ok_or(WindowError::Unreadable)?;
                    if is_sps(self.codec, nal) {
                        let sps = self.rewrite_sps(nal)?;
                        push_length(&mut out, sps.len(), size)?;
                        out.extend_from_slice(&sps);
                    } else {
                        out.extend_from_slice(field);
                        out.extend_from_slice(nal);
                    }
                    at += size + length;
                }
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // Real sequence parameter sets, as the corpus's `avcC`/`hvcC` records
    // carry them (`pnpm corpus`).

    /// x264, High, 640x360 (`h264-high-closed-gop.mp4`): coded 640x368, the
    /// bottom 8 rows cropped.
    const H264_360: [u8; 26] = [
        0x67, 0x64, 0x00, 0x1E, 0xAC, 0xD9, 0x40, 0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00, 0x00, 0x03,
        0x00, 0x01, 0x00, 0x00, 0x03, 0x00, 0x3C, 0x0F, 0x16, 0x2D, 0x96,
    ];
    /// x264, High, 1280x720 (`portrait-phone.mp4`): no window.
    const H264_720: [u8; 26] = [
        0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9, 0x40, 0x50, 0x05, 0xBB, 0x01, 0x10, 0x00, 0x00, 0x03,
        0x00, 0x10, 0x00, 0x00, 0x03, 0x03, 0xC0, 0xF1, 0x83, 0x19, 0x60,
    ];
    /// x265, Main 10, 640x360 (`hevc-main10.mp4`): no window.
    const HEVC_MAIN10: [u8; 46] = [
        0x42, 0x01, 0x01, 0x02, 0x20, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x03, 0x00, 0x3F, 0xA0, 0x05, 0x02, 0x01, 0x69, 0x36, 0x59, 0x59, 0xA4, 0x93, 0x2B, 0xC0,
        0x5A, 0x81, 0x01, 0x00, 0x82, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x3C,
        0x10,
    ];

    fn full(width: u32, height: u32) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    fn border(width: u32, height: u32) -> Rect {
        Rect {
            x: 16,
            y: 16,
            width: width - 32,
            height: height - 32,
        }
    }

    /// The payload's bits before the window, and from after it to the stop
    /// bit: everything a rewrite must leave alone.
    fn untouched(parsed: &Parsed) -> (Vec<Option<bool>>, Vec<Option<bool>>) {
        let stop = stop_bit(&parsed.rbsp).expect("stop bit");
        (
            (0..parsed.window_from)
                .map(|at| bit_at(&parsed.rbsp, at))
                .collect(),
            (parsed.window_to..=stop)
                .map(|at| bit_at(&parsed.rbsp, at))
                .collect(),
        )
    }

    /// An `avcC` record around one SPS and a PPS.
    fn avcc(sps: &[u8]) -> Vec<u8> {
        let mut record = vec![1, sps[1], sps[2], sps[3], 0xFF, 0xE1, 0];
        record.push(u8::try_from(sps.len()).expect("short"));
        record.extend_from_slice(sps);
        record.extend_from_slice(&[1, 0, 2, 0x68, 0xEB]);
        record
    }

    #[test]
    fn real_parameter_sets_read_their_coded_size_and_window() {
        let h264 = sps_geometry(VideoCodec::H264, &H264_360).expect("h264");
        assert_eq!((h264.coded_width, h264.coded_height), (640, 368));
        assert_eq!(h264.shown, full(640, 360));
        assert_eq!((h264.unit_x, h264.unit_y), (2, 2));
        let hevc = sps_geometry(VideoCodec::Hevc, &HEVC_MAIN10).expect("hevc");
        assert_eq!((hevc.coded_width, hevc.coded_height), (640, 360));
        assert_eq!(hevc.shown, full(640, 360));
        assert_eq!((hevc.unit_x, hevc.unit_y), (2, 2));
    }

    #[test]
    fn the_same_window_writes_the_same_bytes() {
        let parsed = parse_h264(&H264_360).expect("h264");
        let offsets = window_for(&parsed.geometry, full(640, 360)).expect("window");
        assert_eq!(offsets, [0, 0, 0, 4]);
        assert_eq!(
            rewrite(&H264_360, &parsed, offsets).expect("rewrite"),
            H264_360
        );
        let parsed = parse_hevc(&HEVC_MAIN10).expect("hevc");
        let offsets = window_for(&parsed.geometry, full(640, 360)).expect("window");
        assert_eq!(offsets, [0; 4]);
        assert_eq!(
            rewrite(&HEVC_MAIN10, &parsed, offsets).expect("rewrite"),
            HEVC_MAIN10
        );
    }

    #[test]
    fn a_new_window_changes_the_window_and_no_other_bit() {
        for (codec, nal) in [
            (VideoCodec::H264, &H264_360[..]),
            (VideoCodec::H264, &H264_720[..]),
            (VideoCodec::Hevc, &HEVC_MAIN10[..]),
        ] {
            let parsed = parse(codec, nal).expect("sps");
            let shown = parsed.geometry.shown;
            let rect = border(shown.width, shown.height);
            let offsets = window_for(&parsed.geometry, rect).expect("window");
            let written = rewrite(nal, &parsed, offsets).expect("rewrite");
            let again = parse(codec, &written).expect("reads back");
            assert_eq!(again.geometry.shown, rect, "{codec:?}");
            assert_eq!(
                (again.geometry.coded_width, again.geometry.coded_height),
                (parsed.geometry.coded_width, parsed.geometry.coded_height)
            );
            assert_eq!(untouched(&parsed), untouched(&again), "{codec:?}");
            assert_eq!(written[..parsed.header], nal[..parsed.header]);
        }
    }

    #[test]
    fn the_window_composes_with_the_one_the_stream_has() {
        // The source already hides its bottom 8 coded rows: a crop 16 rows
        // above the bottom of what is shown hides 24, 12 chroma rows.
        let geometry = sps_geometry(VideoCodec::H264, &H264_360).expect("h264");
        let offsets = window_for(&geometry, border(640, 360)).expect("window");
        assert_eq!(offsets, [8, 8, 8, 12]);
    }

    #[test]
    fn odd_offsets_are_refused_in_4_2_0() {
        let geometry = sps_geometry(VideoCodec::H264, &H264_360).expect("h264");
        let hevc = sps_geometry(VideoCodec::Hevc, &HEVC_MAIN10).expect("hevc");
        for rect in [
            Rect {
                x: 1,
                y: 0,
                width: 600,
                height: 360,
            },
            Rect {
                x: 0,
                y: 3,
                width: 640,
                height: 300,
            },
            full(639, 360),
            full(640, 355),
        ] {
            let misaligned = Err(WindowError::Misaligned {
                unit_x: 2,
                unit_y: 2,
            });
            assert_eq!(window_for(&geometry, rect), misaligned, "{rect:?}");
            assert_eq!(window_for(&hevc, rect), misaligned, "{rect:?}");
        }
    }

    #[test]
    fn a_window_outside_the_shown_picture_is_refused() {
        let geometry = sps_geometry(VideoCodec::H264, &H264_360).expect("h264");
        for rect in [
            // Into the 8 coded rows under the picture: larger than it.
            full(640, 368),
            full(642, 360),
            Rect {
                x: 32,
                y: 0,
                width: 640,
                height: 360,
            },
            full(0, 360),
            Rect {
                x: u32::MAX - 1,
                y: 0,
                width: 2,
                height: 2,
            },
        ] {
            assert_eq!(
                window_for(&geometry, rect),
                Err(WindowError::OutsidePicture),
                "{rect:?}"
            );
        }
    }

    #[test]
    fn emulation_prevention_is_applied_to_the_new_payload() {
        assert_eq!(
            escape(&[0, 0, 1, 0, 0, 0, 0, 4]),
            vec![0, 0, 3, 1, 0, 0, 3, 0, 0, 4]
        );
        assert_eq!(escape(&[0; 6]), vec![0, 0, 3, 0, 0, 3, 0, 0]);
        assert_eq!(escape(&[0, 0, 4]), vec![0, 0, 4]);
    }

    #[test]
    fn exp_golomb_writes_what_the_reader_reads() {
        let mut writer = BitWriter::default();
        let values = [0, 1, 2, 3, 7, 8, 254, 255, 1000, 65_535];
        for value in values {
            writer.ue(value);
        }
        writer.bit(true);
        let mut bits = Bits::new(&escape(&writer.bytes));
        for value in values {
            assert_eq!(bits.ue(), Some(value));
        }
    }

    #[test]
    fn an_sps_repeated_in_band_is_rewritten_and_the_slices_are_not() {
        let window =
            CropWindow::new(VideoCodec::H264, &avcc(&H264_360), border(640, 360)).expect("window");
        let slice = [0, 0, 0, 3, 0x65, 0x88, 0x84];
        let mut packet = vec![0, 0, 0, 26];
        packet.extend_from_slice(&H264_360);
        packet.extend_from_slice(&slice);
        let out = window.packet(&packet).expect("packet");
        assert!(out.ends_with(&slice));
        let length = usize::from(u16::from_be_bytes([out[2], out[3]]));
        let sps = &out[4..4 + length];
        assert_eq!(out.len(), 4 + length + slice.len());
        assert_eq!(sps, &window.extradata()[8..8 + length]);
        // A packet without an SPS is returned as it was.
        let plain = [0, 0, 0, 2, 0x41, 0x9A];
        assert_eq!(window.packet(&plain).expect("slice"), plain);
    }

    #[test]
    fn parameter_sets_that_change_mid_stream_are_refused() {
        let window =
            CropWindow::new(VideoCodec::H264, &avcc(&H264_360), border(640, 360)).expect("window");
        // The 1280x720 SPS of another corpus file arrives in band.
        let mut packet = vec![0, 0, 0, 26];
        packet.extend_from_slice(&H264_720);
        assert_eq!(
            window.packet(&packet),
            Err(WindowError::ParameterSetsChange)
        );
        // A record whose SPSs disagree is refused before anything is written.
        let mut record = avcc(&H264_360);
        record[5] = 0xE2;
        record.splice(34..34, [[0, 26].as_slice(), &H264_720].concat());
        assert_eq!(
            CropWindow::new(VideoCodec::H264, &record, border(640, 360)).err(),
            Some(WindowError::ParameterSetsChange)
        );
    }

    #[test]
    fn an_unchanged_window_leaves_the_record_as_it_was() {
        let record = avcc(&H264_360);
        let window = CropWindow::new(VideoCodec::H264, &record, full(640, 360)).expect("window");
        assert_eq!(window.extradata(), record);
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        let record = avcc(&H264_360);
        for cut in 0..record.len() {
            let _ = sps_geometry(VideoCodec::H264, &H264_360[..cut.min(H264_360.len())]);
            let _ = sps_geometry(VideoCodec::Hevc, &HEVC_MAIN10[..cut.min(HEVC_MAIN10.len())]);
            let _ = CropWindow::new(VideoCodec::H264, &record[..cut], full(2, 2));
            let _ = CropWindow::new(VideoCodec::Hevc, &record[..cut], full(2, 2));
        }
        assert_eq!(
            CropWindow::new(VideoCodec::Vp9, &[], full(2, 2)).err(),
            Some(WindowError::Unsupported(VideoCodec::Vp9))
        );
    }
}
