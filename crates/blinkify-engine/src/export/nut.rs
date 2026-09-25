//! NUT, the container the export pipes packets through (ADR-0010).
//!
//! The export is one pass to the output file, with no intermediate media file
//! (`CLAUDE.md` forbidden behaviour 1). The engine sits between sidecar
//! processes — readers that copy a source's packets out, encoders that make a
//! seam, and the one muxer that writes the file — and routes packets between
//! them over pipes. NUT is the format on those pipes because it is FFmpeg's
//! own: it carries any codec FFmpeg can copy, its codec configuration
//! unchanged, and **every timestamp exactly in the stream's own time base**,
//! which Matroska (milliseconds) and MPEG-TS (90 kHz) do not.
//!
//! [`Reader`] reads what FFmpeg's NUT muxer writes, whatever frame-code table
//! and elision headers it chose. [`Writer`] writes the simplest valid NUT:
//! one syncpoint before every frame, every timestamp coded in full, every
//! frame header checksummed — a few bytes a frame, and nothing for the
//! demuxer to predict or resynchronise.
//!
//! Every length in a NUT stream came from another process and is treated as
//! hostile: sizes are bounded before anything is allocated, and a malformed
//! stream is an error, never a panic.
//!
//! The format: `libavformat/nut.h`, `nutdec.c` and `nutenc.c` of the FFmpeg
//! the sidecar is built from.

use std::io::{self, Read, Write};

use thiserror::Error;

use crate::probe::Rational;

const MAIN_STARTCODE: u64 = 0x7A56_1F5F_04AD + ((((b'N' as u64) << 8) + b'M' as u64) << 48);
const STREAM_STARTCODE: u64 = 0x1140_5BF2_F9DB + ((((b'N' as u64) << 8) + b'S' as u64) << 48);
const SYNCPOINT_STARTCODE: u64 = 0xE4AD_EECA_4569 + ((((b'N' as u64) << 8) + b'K' as u64) << 48);
const INDEX_STARTCODE: u64 = 0xDD67_2F23_E64E + ((((b'N' as u64) << 8) + b'X' as u64) << 48);
const INFO_STARTCODE: u64 = 0xAB68_B596_BA78 + ((((b'N' as u64) << 8) + b'I' as u64) << 48);

const ID_STRING: &[u8] = b"nut/multimedia container\0";

const FLAG_KEY: u64 = 1;
const FLAG_CODED_PTS: u64 = 8;
const FLAG_STREAM_ID: u64 = 16;
const FLAG_SIZE_MSB: u64 = 32;
const FLAG_CHECKSUM: u64 = 64;
const FLAG_RESERVED: u64 = 128;
const FLAG_SM_DATA: u64 = 256;
const FLAG_HEADER_IDX: u64 = 1024;
const FLAG_MATCH_TIME: u64 = 2048;
const FLAG_CODED: u64 = 4096;
const FLAG_INVALID: u64 = 8192;

/// The largest header packet read: FFmpeg's are a few kilobytes; extradata
/// is the only large field and no codec's reaches this.
const MAX_HEADER: u64 = 16 << 20;

/// The largest frame accepted: a 16K intra frame is well below it.
const MAX_FRAME: u64 = 256 << 20;

const MAX_STREAMS: u64 = 64;

/// Why a NUT stream could not be read or written.
#[derive(Debug, Error)]
pub enum NutError {
    #[error("the NUT stream could not be read or written: {0}")]
    Io(#[from] io::Error),
    #[error("the NUT stream is malformed: {0}")]
    Malformed(String),
    #[error("the NUT stream ended before its headers")]
    NoHeaders,
}

fn malformed<T>(message: impl Into<String>) -> Result<T, NutError> {
    Err(NutError::Malformed(message.into()))
}

/// CRC-32, polynomial 0x04C11DB7, most significant bit first, initial value
/// zero and no final inversion: NUT's checksum.
fn crc(mut value: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        value ^= u32::from(byte) << 24;
        for _ in 0..8 {
            value = if value & 0x8000_0000 == 0 {
                value << 1
            } else {
                (value << 1) ^ 0x04C1_1DB7
            };
        }
    }
    value
}

/// What kind of stream, as NUT numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Video,
    Audio,
    Subtitle,
    Data,
}

/// One stream's header, kept whole so it can be written again unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamHeader {
    pub class: Class,
    /// The codec tag, as written: two or four bytes.
    pub fourcc: Vec<u8>,
    pub time_base: Rational,
    pub msb_pts_shift: u64,
    pub max_pts_distance: u64,
    /// Pictures the decoder holds before the first is shown: the reorder
    /// depth from which the demuxer derives decode timestamps.
    pub decode_delay: u64,
    pub flags: u64,
    /// The codec configuration — SPS and PPS for H.264 — unchanged.
    pub extradata: Vec<u8>,
    /// Width, height, sample aspect numerator and denominator, colourspace.
    pub video: Option<[u64; 5]>,
    /// Sample rate numerator and denominator, channels.
    pub audio: Option<[u64; 3]>,
    /// The stream's metadata: language, handler name, disposition.
    pub metadata: Vec<(String, String)>,
}

/// A chapter, in its own time base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub time_base: Rational,
    pub start: u64,
    pub length: u64,
    pub metadata: Vec<(String, String)>,
}

/// Everything before the first frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    pub streams: Vec<StreamHeader>,
    /// The file's metadata: creation time, title.
    pub metadata: Vec<(String, String)>,
    pub chapters: Vec<Chapter>,
}

/// One packet: a coded frame of one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub stream: usize,
    /// In the stream's time base.
    pub pts: i64,
    pub key: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameCode {
    flags: u64,
    stream: u64,
    size_mul: u64,
    size_lsb: u64,
    pts_delta: i64,
    reserved: u64,
    header_idx: usize,
}

/// A cursor over one packet's bytes.
struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn byte(&mut self) -> Result<u8, NutError> {
        let byte = self
            .data
            .get(self.at)
            .copied()
            .ok_or_else(|| NutError::Malformed("a header ends early".to_owned()))?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, count: u64) -> Result<&'a [u8], NutError> {
        let count = usize::try_from(count)
            .map_err(|_| NutError::Malformed("a length is out of range".to_owned()))?;
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.data.len())
            .ok_or_else(|| NutError::Malformed("a field runs past its header".to_owned()))?;
        let slice = self.data.get(self.at..end).unwrap_or_default();
        self.at = end;
        Ok(slice)
    }

    fn v(&mut self) -> Result<u64, NutError> {
        let mut value: u64 = 0;
        for _ in 0..10 {
            let byte = self.byte()?;
            value = (value << 7) | u64::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        malformed("a number is longer than 64 bits")
    }

    fn s(&mut self) -> Result<i64, NutError> {
        let v = self.v()?.wrapping_add(1);
        let magnitude = i64::try_from(v >> 1).unwrap_or(i64::MAX);
        Ok(if v & 1 == 1 { -magnitude } else { magnitude })
    }

    fn string(&mut self) -> Result<String, NutError> {
        let length = self.v()?;
        Ok(String::from_utf8_lossy(self.take(length)?)
            .trim_end_matches('\0')
            .to_owned())
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.at)
    }
}

fn put_v(out: &mut Vec<u8>, value: u64) {
    let mut groups = 1;
    while groups < 10 && value >> (7 * groups) != 0 {
        groups += 1;
    }
    for group in (1..groups).rev() {
        // Truncation to the low seven bits is the encoding.
        #[allow(clippy::cast_possible_truncation)]
        out.push(0x80 | ((value >> (7 * group)) as u8 & 0x7F));
    }
    #[allow(clippy::cast_possible_truncation)]
    out.push(value as u8 & 0x7F);
}

fn put_s(out: &mut Vec<u8>, value: i64) {
    let magnitude = value.unsigned_abs();
    put_v(out, (2 * magnitude).wrapping_sub(u64::from(value > 0)));
}

fn put_string(out: &mut Vec<u8>, text: &str) {
    put_v(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn rational(num: u64, den: u64) -> Result<Rational, NutError> {
    match (i64::try_from(num), i64::try_from(den)) {
        (Ok(num), Ok(den)) if num > 0 && den > 0 && num < (1 << 31) && den < (1 << 31) => {
            Ok(Rational { num, den })
        }
        _ => malformed(format!("{num}/{den} is not a time base")),
    }
}

/// `value` ticks of `from` in ticks of `to`, rounded down, as the NUT
/// demuxer resets its timestamp predictions at a syncpoint.
fn rescale_down(value: i64, from: Rational, to: Rational) -> i64 {
    crate::time::rescale(value, from, to, crate::time::Rounding::Down).unwrap_or(value)
}

/// The frame-code table of a main header: 256 entries, run-length coded.
fn frame_codes(bytes: &mut Bytes<'_>, streams: u64) -> Result<Vec<FrameCode>, NutError> {
    let mut codes = vec![FrameCode::default(); 256];
    let (mut pts, mut mul, mut stream, mut header_idx) = (0_i64, 1_u64, 0_u64, 0_usize);
    let mut i = 0_usize;
    while i < 256 {
        let flags = bytes.v()?;
        let fields = bytes.v()?;
        if fields > 0 {
            pts = bytes.s()?;
        }
        if fields > 1 {
            mul = bytes.v()?;
        }
        if fields > 2 {
            stream = bytes.v()?;
        }
        let size = if fields > 3 { bytes.v()? } else { 0 };
        let reserved = if fields > 4 { bytes.v()? } else { 0 };
        let count = if fields > 5 {
            bytes.v()?
        } else {
            mul.wrapping_sub(size)
        };
        if fields > 6 {
            bytes.s()?;
        }
        if fields > 7 {
            header_idx = usize::try_from(bytes.v()?).unwrap_or(usize::MAX);
        }
        for _ in 8..fields {
            bytes.v()?;
        }
        let room = 256 - usize::from(i <= usize::from(b'N')) - i;
        if count == 0 || count > room as u64 || stream >= streams {
            return malformed(format!("frame code table entry {i} is invalid"));
        }
        let mut j = 0;
        while j < count {
            let Some(code) = codes.get_mut(i) else {
                return malformed("the frame code table overruns");
            };
            if i == usize::from(b'N') {
                code.flags = FLAG_INVALID;
                i += 1;
                continue;
            }
            *code = FrameCode {
                flags,
                stream,
                size_mul: mul,
                size_lsb: size + j,
                pts_delta: pts,
                reserved,
                header_idx,
            };
            i += 1;
            j += 1;
        }
    }
    if let Some(code) = codes.get_mut(usize::from(b'N')) {
        code.flags = FLAG_INVALID;
    }
    Ok(codes)
}

/// Reads the NUT stream FFmpeg writes.
#[derive(Debug)]
pub struct Reader<R> {
    input: R,
    header: Header,
    time_bases: Vec<Rational>,
    codes: Vec<FrameCode>,
    elision: Vec<Vec<u8>>,
    last_pts: Vec<i64>,
    /// A startcode read while looking for a frame, to be handled next.
    pending: Option<u64>,
}

impl<R: Read> Reader<R> {
    /// Read the headers: the main header, every stream header, and the info
    /// packets before the first syncpoint.
    ///
    /// # Errors
    ///
    /// The input ends first, or a header is malformed.
    pub fn open(mut input: R) -> Result<Self, NutError> {
        let mut state: u64 = 0;
        loop {
            let mut byte = [0_u8];
            if input.read(&mut byte)? == 0 {
                return Err(NutError::NoHeaders);
            }
            state = (state << 8) | u64::from(byte[0]);
            if state == MAIN_STARTCODE {
                break;
            }
        }
        let mut reader = Self {
            input,
            header: Header::default(),
            time_bases: Vec::new(),
            codes: Vec::new(),
            elision: vec![Vec::new()],
            last_pts: Vec::new(),
            pending: None,
        };
        let main = reader.packet_body(MAIN_STARTCODE)?;
        let stream_count = reader.main_header(&main)?;
        let mut streams: Vec<Option<StreamHeader>> = vec![None; stream_count];
        loop {
            let startcode = reader.startcode()?;
            match startcode {
                STREAM_STARTCODE => {
                    let body = reader.packet_body(startcode)?;
                    let (id, stream) = reader.stream_header(&body)?;
                    match streams.get_mut(id) {
                        Some(slot) => *slot = Some(stream),
                        None => return malformed(format!("stream {id} is not declared")),
                    }
                }
                INFO_STARTCODE => {
                    let body = reader.packet_body(startcode)?;
                    reader.info(&body, &mut streams)?;
                }
                SYNCPOINT_STARTCODE => {
                    reader.pending = Some(startcode);
                    break;
                }
                _ => {
                    reader.packet_body(startcode)?;
                }
            }
        }
        reader.header.streams = streams
            .into_iter()
            .enumerate()
            .map(|(id, stream)| {
                stream.ok_or_else(|| NutError::Malformed(format!("stream {id} has no header")))
            })
            .collect::<Result<_, _>>()?;
        reader.last_pts = vec![0; reader.header.streams.len()];
        Ok(reader)
    }

    #[must_use]
    pub fn header(&self) -> &Header {
        &self.header
    }

    fn read_u64(&mut self) -> Result<Option<u64>, NutError> {
        let mut bytes = [0_u8; 8];
        let mut filled = 0;
        while filled < 8 {
            let Some(rest) = bytes.get_mut(filled..) else {
                break;
            };
            let read = self.input.read(rest)?;
            if read == 0 {
                return if filled == 0 {
                    Ok(None)
                } else {
                    malformed("the stream ends inside a startcode")
                };
            }
            filled += read;
        }
        Ok(Some(u64::from_be_bytes(bytes)))
    }

    fn startcode(&mut self) -> Result<u64, NutError> {
        if let Some(code) = self.pending.take() {
            return Ok(code);
        }
        self.read_u64()?.ok_or(NutError::NoHeaders)
    }

    fn read_byte(&mut self) -> Result<Option<u8>, NutError> {
        let mut byte = [0_u8];
        Ok((self.input.read(&mut byte)? != 0).then_some(byte[0]))
    }

    fn read_exact_vec(&mut self, count: u64, limit: u64) -> Result<Vec<u8>, NutError> {
        if count > limit {
            return malformed(format!("{count} bytes is more than a NUT field may hold"));
        }
        // Grown as the bytes arrive, not allocated from the claimed length:
        // a corrupt length must not reserve memory the stream never fills.
        let mut data = Vec::new();
        (&mut self.input).take(count).read_to_end(&mut data)?;
        if data.len() as u64 != count {
            return malformed("the stream ends inside a packet");
        }
        Ok(data)
    }

    fn read_v(&mut self, checksum: &mut Vec<u8>) -> Result<u64, NutError> {
        let mut value: u64 = 0;
        for _ in 0..10 {
            let byte = self
                .read_byte()?
                .ok_or_else(|| NutError::Malformed("the stream ends inside a number".to_owned()))?;
            checksum.push(byte);
            value = (value << 7) | u64::from(byte & 0x7F);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        malformed("a number is longer than 64 bits")
    }

    /// The body of the packet whose startcode was just read, checksums
    /// verified, without the trailing checksum.
    fn packet_body(&mut self, startcode: u64) -> Result<Vec<u8>, NutError> {
        let mut prefix = startcode.to_be_bytes().to_vec();
        let forward = self.read_v(&mut prefix)?;
        if forward > 4096 {
            let stored = self.read_exact_vec(4, 4)?;
            prefix.extend_from_slice(&stored);
            if crc(0, &prefix) != 0 {
                return malformed("a packet header's checksum does not match");
            }
        }
        if forward < 4 {
            return malformed("a packet is shorter than its checksum");
        }
        let body = self.read_exact_vec(forward, MAX_HEADER)?;
        if crc(0, &body) != 0 {
            return malformed("a packet's checksum does not match");
        }
        let mut body = body;
        body.truncate(body.len() - 4);
        Ok(body)
    }

    fn main_header(&mut self, body: &[u8]) -> Result<usize, NutError> {
        let mut bytes = Bytes::new(body);
        let version = bytes.v()?;
        if !(2..=4).contains(&version) {
            return malformed(format!("NUT version {version} is not supported"));
        }
        if version > 3 {
            bytes.v()?;
        }
        let streams = bytes.v()?;
        if streams == 0 || streams > MAX_STREAMS {
            return malformed(format!("{streams} streams"));
        }
        bytes.v()?; // max_distance: frames are read whole, not resynchronised.
        let time_bases = bytes.v()?;
        if time_bases == 0 || time_bases > 1024 {
            return malformed(format!("{time_bases} time bases"));
        }
        for _ in 0..time_bases {
            let num = bytes.v()?;
            let den = bytes.v()?;
            self.time_bases.push(rational(num, den)?);
        }
        let codes = frame_codes(&mut bytes, streams)?;
        self.codes = codes;
        if bytes.remaining() > 0 {
            let headers = bytes.v()?;
            if headers >= 128 {
                return malformed(format!("{headers} elision headers"));
            }
            for _ in 0..headers {
                let length = bytes.v()?;
                if length == 0 || length >= 256 {
                    return malformed("an elision header is out of range");
                }
                self.elision.push(bytes.take(length)?.to_vec());
            }
        }
        if self
            .codes
            .iter()
            .any(|code| code.flags & FLAG_INVALID == 0 && code.header_idx >= self.elision.len())
        {
            return malformed("a frame code names an elision header that does not exist");
        }
        usize::try_from(streams).or_else(|_| malformed("stream count"))
    }

    fn stream_header(&self, body: &[u8]) -> Result<(usize, StreamHeader), NutError> {
        let mut bytes = Bytes::new(body);
        let id = usize::try_from(bytes.v()?).unwrap_or(usize::MAX);
        let class = match bytes.v()? {
            0 => Class::Video,
            1 => Class::Audio,
            2 => Class::Subtitle,
            3 => Class::Data,
            other => return malformed(format!("stream class {other}")),
        };
        let fourcc_length = bytes.v()?;
        if fourcc_length != 2 && fourcc_length != 4 {
            return malformed("a codec tag is neither two nor four bytes");
        }
        let fourcc = bytes.take(fourcc_length)?.to_vec();
        let time_base = usize::try_from(bytes.v()?)
            .ok()
            .and_then(|index| self.time_bases.get(index).copied())
            .ok_or_else(|| NutError::Malformed("a stream names no time base".to_owned()))?;
        let msb_pts_shift = bytes.v()?;
        if msb_pts_shift >= 16 {
            return malformed("msb_pts_shift is out of range");
        }
        let max_pts_distance = bytes.v()?;
        let decode_delay = bytes.v()?;
        let flags = bytes.v()?;
        let extradata_length = bytes.v()?;
        let extradata = bytes.take(extradata_length)?.to_vec();
        let (mut video, mut audio) = (None, None);
        match class {
            Class::Video => {
                video = Some([bytes.v()?, bytes.v()?, bytes.v()?, bytes.v()?, bytes.v()?]);
            }
            Class::Audio => audio = Some([bytes.v()?, bytes.v()?, bytes.v()?]),
            Class::Subtitle | Class::Data => {}
        }
        Ok((
            id,
            StreamHeader {
                class,
                fourcc,
                time_base,
                msb_pts_shift,
                max_pts_distance,
                decode_delay,
                flags,
                extradata,
                video,
                audio,
                metadata: Vec::new(),
            },
        ))
    }

    fn info(&mut self, body: &[u8], streams: &mut [Option<StreamHeader>]) -> Result<(), NutError> {
        let mut bytes = Bytes::new(body);
        let stream_plus_one = bytes.v()?;
        let chapter = bytes.s()?;
        let start = bytes.v()?;
        let length = bytes.v()?;
        let count = bytes.v()?;
        let mut metadata = Vec::new();
        for _ in 0..count.min(4096) {
            let name = bytes.string()?;
            let value = bytes.s()?;
            match value {
                -1 => metadata.push((name, bytes.string()?)),
                -2 => {
                    let kind = bytes.string()?;
                    let text = bytes.string()?;
                    if kind == "UTF-8" {
                        metadata.push((name, text));
                    }
                }
                -3 => {
                    bytes.s()?;
                }
                -4 => {
                    bytes.v()?;
                }
                v if v < -4 => {
                    bytes.s()?;
                }
                _ => {}
            }
        }
        if stream_plus_one > 0 {
            let slot = usize::try_from(stream_plus_one - 1)
                .ok()
                .and_then(|id| streams.get_mut(id))
                .and_then(Option::as_mut);
            if let Some(stream) = slot {
                stream.metadata.extend(metadata);
            }
        } else if chapter != 0 {
            let count = self.time_bases.len() as u64;
            let time_base = usize::try_from(start % count)
                .ok()
                .and_then(|index| self.time_bases.get(index).copied())
                .ok_or_else(|| NutError::Malformed("a chapter names no time base".to_owned()))?;
            self.header.chapters.push(Chapter {
                time_base,
                start: start.div_euclid(count),
                length,
                metadata,
            });
        } else {
            self.header.metadata.extend(metadata);
        }
        Ok(())
    }

    fn syncpoint(&mut self) -> Result<(), NutError> {
        let body = self.packet_body(SYNCPOINT_STARTCODE)?;
        let mut bytes = Bytes::new(&body);
        let tt = bytes.v()?;
        let count = self.time_bases.len() as u64;
        let time_base = usize::try_from(tt % count)
            .ok()
            .and_then(|index| self.time_bases.get(index).copied())
            .ok_or_else(|| NutError::Malformed("a syncpoint names no time base".to_owned()))?;
        let ts = i64::try_from(tt.div_euclid(count)).unwrap_or(i64::MAX);
        for (last, stream) in self.last_pts.iter_mut().zip(&self.header.streams) {
            *last = rescale_down(ts, time_base, stream.time_base);
        }
        Ok(())
    }

    /// The next packet, in the order they were written (decode order), or
    /// `None` at the end.
    ///
    /// # Errors
    ///
    /// The stream is malformed or cannot be read.
    pub fn next_packet(&mut self) -> Result<Option<Packet>, NutError> {
        loop {
            let code = match self.pending.take() {
                Some(startcode) => {
                    self.startcode_packet(startcode)?;
                    continue;
                }
                None => match self.read_byte()? {
                    Some(code) => code,
                    None => return Ok(None),
                },
            };
            if code == b'N' {
                let mut rest = [0_u8; 7];
                self.input.read_exact(&mut rest)?;
                let mut startcode = u64::from(code);
                for byte in rest {
                    startcode = (startcode << 8) | u64::from(byte);
                }
                self.startcode_packet(startcode)?;
                continue;
            }
            return self.frame(code).map(Some);
        }
    }

    fn startcode_packet(&mut self, startcode: u64) -> Result<(), NutError> {
        match startcode {
            SYNCPOINT_STARTCODE => self.syncpoint(),
            MAIN_STARTCODE | STREAM_STARTCODE | INFO_STARTCODE | INDEX_STARTCODE => {
                // Repeated headers, metadata updates and the index: nothing a
                // linear reader needs.
                self.packet_body(startcode).map(|_| ())
            }
            other => malformed(format!("unknown startcode {other:016x}")),
        }
    }

    fn frame(&mut self, code: u8) -> Result<Packet, NutError> {
        let table = self
            .codes
            .get(usize::from(code))
            .copied()
            .ok_or_else(|| NutError::Malformed("frame code".to_owned()))?;
        let mut header = vec![code];
        let mut flags = table.flags;
        if flags & FLAG_INVALID != 0 {
            return malformed(format!("frame code {code} is invalid"));
        }
        if flags & FLAG_CODED != 0 {
            flags ^= self.read_v(&mut header)?;
        }
        let mut stream = table.stream;
        if flags & FLAG_STREAM_ID != 0 {
            stream = self.read_v(&mut header)?;
        }
        let stream = usize::try_from(stream)
            .ok()
            .filter(|s| *s < self.header.streams.len())
            .ok_or_else(|| NutError::Malformed(format!("frame of stream {stream}")))?;
        let shift = self
            .header
            .streams
            .get(stream)
            .map_or(0, |s| s.msb_pts_shift);
        let last = self.last_pts.get(stream).copied().unwrap_or(0);
        let pts = if flags & FLAG_CODED_PTS != 0 {
            let coded = self.read_v(&mut header)?;
            let full = 1_u64 << shift;
            if coded < full {
                let mask = i64::try_from(full - 1).unwrap_or(0);
                let delta = last - (mask >> 1);
                let lsb = i64::try_from(coded).unwrap_or(0);
                ((lsb - delta) & mask) + delta
            } else {
                i64::try_from(coded - full).unwrap_or(i64::MAX)
            }
        } else {
            last.saturating_add(table.pts_delta)
        };
        let mut size = table.size_lsb;
        if flags & FLAG_SIZE_MSB != 0 {
            size = size.saturating_add(table.size_mul.saturating_mul(self.read_v(&mut header)?));
        }
        if flags & FLAG_MATCH_TIME != 0 {
            self.read_v(&mut header)?;
        }
        let mut header_idx = table.header_idx;
        if flags & FLAG_HEADER_IDX != 0 {
            header_idx = usize::try_from(self.read_v(&mut header)?).unwrap_or(usize::MAX);
        }
        let mut reserved = table.reserved;
        if flags & FLAG_RESERVED != 0 {
            reserved = self.read_v(&mut header)?;
        }
        for _ in 0..reserved.min(256) {
            self.read_v(&mut header)?;
        }
        if flags & FLAG_SM_DATA != 0 {
            return malformed("frames with side data (NUT version 4) are not read");
        }
        if size > 4096 {
            header_idx = 0;
        }
        let elided =
            self.elision.get(header_idx).cloned().ok_or_else(|| {
                NutError::Malformed("an elision header does not exist".to_owned())
            })?;
        let size = size
            .checked_sub(elided.len() as u64)
            .ok_or_else(|| NutError::Malformed("a frame is shorter than its header".to_owned()))?;
        if flags & FLAG_CHECKSUM != 0 {
            let stored = self.read_exact_vec(4, 4)?;
            header.extend_from_slice(&stored);
            if crc(0, &header) != 0 {
                return malformed("a frame header's checksum does not match");
            }
        }
        let body = self.read_exact_vec(size, MAX_FRAME)?;
        if let Some(slot) = self.last_pts.get_mut(stream) {
            *slot = pts;
        }
        let mut data = elided;
        data.extend_from_slice(&body);
        Ok(Packet {
            stream,
            pts,
            key: flags & FLAG_KEY != 0,
            data,
        })
    }
}

/// The main header's body: version 3, the time bases, and a frame-code
/// table of two runs.
fn main_header(streams: usize, time_bases: &[Rational]) -> Vec<u8> {
    let mut main = Vec::new();
    put_v(&mut main, 3);
    put_v(&mut main, streams as u64);
    put_v(&mut main, 65_536);
    put_v(&mut main, time_bases.len() as u64);
    for time_base in time_bases {
        put_v(&mut main, time_base.num.unsigned_abs());
        put_v(&mut main, time_base.den.unsigned_abs());
    }
    // Two runs of the frame-code table: codes 0–128 (skipping 'N') are
    // non-keyframes, 129–255 keyframes; each run's first code has a size
    // remainder of zero, which is the only one this writer uses.
    for (flags, count) in [(FRAME_FLAGS, 128), (FRAME_FLAGS | FLAG_KEY, 127)] {
        put_v(&mut main, flags);
        put_v(&mut main, 6);
        put_s(&mut main, 0); // pts delta
        put_v(&mut main, 1); // size multiplier
        put_v(&mut main, 0); // stream
        put_v(&mut main, 0); // size remainder
        put_v(&mut main, 0); // reserved
        put_v(&mut main, count);
    }
    put_v(&mut main, 0); // no elision headers
    main
}

/// One stream header's body.
fn stream_header(id: usize, stream: &StreamHeader, time_base: u64) -> Vec<u8> {
    let mut body = Vec::new();
    put_v(&mut body, id as u64);
    put_v(
        &mut body,
        match stream.class {
            Class::Video => 0,
            Class::Audio => 1,
            Class::Subtitle => 2,
            Class::Data => 3,
        },
    );
    put_v(&mut body, stream.fourcc.len() as u64);
    body.extend_from_slice(&stream.fourcc);
    put_v(&mut body, time_base);
    put_v(&mut body, stream.msb_pts_shift);
    put_v(&mut body, stream.max_pts_distance);
    put_v(&mut body, stream.decode_delay);
    put_v(&mut body, stream.flags);
    put_v(&mut body, stream.extradata.len() as u64);
    body.extend_from_slice(&stream.extradata);
    for value in stream
        .video
        .iter()
        .flatten()
        .chain(stream.audio.iter().flatten())
    {
        put_v(&mut body, *value);
    }
    body
}

/// Writes a NUT stream the FFmpeg demuxer reads.
#[derive(Debug)]
pub struct Writer<W: Write> {
    output: W,
    streams: Vec<StreamHeader>,
    time_bases: Vec<Rational>,
    /// Bytes written so far, for the syncpoints' back pointers.
    written: u64,
    last_syncpoint: u64,
}

/// Frame code for a non-keyframe: every field coded, lsb of size zero.
const CODE_FRAME: u8 = 0;
/// Frame code for a keyframe.
const CODE_KEY: u8 = 129;
/// The flags of both: stream, pts, size and a header checksum, all coded.
const FRAME_FLAGS: u64 = FLAG_STREAM_ID | FLAG_CODED_PTS | FLAG_SIZE_MSB | FLAG_CHECKSUM;

impl<W: Write> Writer<W> {
    /// Write the headers of `header`.
    ///
    /// # Errors
    ///
    /// The output cannot be written, or `header` has no streams.
    pub fn new(output: W, header: &Header) -> Result<Self, NutError> {
        if header.streams.is_empty() {
            return malformed("a NUT stream needs at least one stream");
        }
        let mut time_bases: Vec<Rational> = Vec::new();
        let mut header = header.clone();
        // The demuxer refuses a time base not in lowest terms.
        for stream in &mut header.streams {
            stream.time_base = crate::project::settings::reduced(stream.time_base);
        }
        for chapter in &mut header.chapters {
            chapter.time_base = crate::project::settings::reduced(chapter.time_base);
        }
        let header = &header;
        for time_base in header
            .streams
            .iter()
            .map(|s| s.time_base)
            .chain(header.chapters.iter().map(|c| c.time_base))
        {
            if !time_bases.contains(&time_base) {
                time_bases.push(time_base);
            }
        }
        let mut writer = Self {
            output,
            streams: header.streams.clone(),
            time_bases,
            written: 0,
            last_syncpoint: 0,
        };
        writer.raw(ID_STRING)?;

        let main = main_header(header.streams.len(), &writer.time_bases);
        writer.packet(MAIN_STARTCODE, &main)?;

        for (id, stream) in header.streams.iter().enumerate() {
            let body = stream_header(id, stream, writer.time_base_index(stream.time_base));
            writer.packet(STREAM_STARTCODE, &body)?;
        }

        writer.info(0, 0, 0, 0, &header.metadata)?;
        for (id, stream) in header.streams.iter().enumerate() {
            if !stream.metadata.is_empty() {
                writer.info(id as u64 + 1, 0, 0, 0, &stream.metadata)?;
            }
        }
        for (id, chapter) in header.chapters.iter().enumerate() {
            let tt = chapter
                .start
                .saturating_mul(writer.time_bases.len() as u64)
                .saturating_add(writer.time_base_index(chapter.time_base));
            writer.info(
                0,
                i64::try_from(id).unwrap_or(0) + 1,
                tt,
                chapter.length,
                &chapter.metadata,
            )?;
        }
        Ok(writer)
    }

    fn time_base_index(&self, time_base: Rational) -> u64 {
        self.time_bases
            .iter()
            .position(|known| *known == time_base)
            .unwrap_or(0) as u64
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), NutError> {
        self.output.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn packet(&mut self, startcode: u64, body: &[u8]) -> Result<(), NutError> {
        let mut head = startcode.to_be_bytes().to_vec();
        let forward = body.len() as u64 + 4;
        put_v(&mut head, forward);
        if forward > 4096 {
            let check = crc(0, &head);
            head.extend_from_slice(&check.to_be_bytes());
        }
        self.raw(&head)?;
        self.raw(body)?;
        self.raw(&crc(0, body).to_be_bytes())
    }

    fn info(
        &mut self,
        stream_plus_one: u64,
        chapter: i64,
        start: u64,
        length: u64,
        metadata: &[(String, String)],
    ) -> Result<(), NutError> {
        let mut body = Vec::new();
        put_v(&mut body, stream_plus_one);
        put_s(&mut body, chapter);
        put_v(&mut body, start);
        put_v(&mut body, length);
        put_v(&mut body, metadata.len() as u64);
        for (name, value) in metadata {
            put_string(&mut body, name);
            put_s(&mut body, -1);
            put_string(&mut body, value);
        }
        self.packet(INFO_STARTCODE, &body)
    }

    /// Write one packet. Its `pts` must not be negative: NUT cannot carry one.
    ///
    /// # Errors
    ///
    /// The output cannot be written, the stream does not exist, or the
    /// timestamp is negative.
    pub fn write(&mut self, packet: &Packet) -> Result<(), NutError> {
        let Some(stream) = self.streams.get(packet.stream) else {
            return malformed(format!("no stream {}", packet.stream));
        };
        let Ok(pts) = u64::try_from(packet.pts) else {
            return malformed(format!("a negative timestamp ({})", packet.pts));
        };
        let shift = stream.msb_pts_shift;
        let time_base = stream.time_base;

        let mut sync = Vec::new();
        let count = self.time_bases.len() as u64;
        put_v(
            &mut sync,
            pts.saturating_mul(count)
                .saturating_add(self.time_base_index(time_base)),
        );
        put_v(&mut sync, (self.written - self.last_syncpoint) >> 4);
        self.last_syncpoint = self.written;
        self.packet(SYNCPOINT_STARTCODE, &sync)?;

        let mut header = vec![if packet.key { CODE_KEY } else { CODE_FRAME }];
        put_v(&mut header, packet.stream as u64);
        put_v(&mut header, pts.saturating_add(1 << shift));
        put_v(&mut header, packet.data.len() as u64);
        let check = crc(0, &header);
        header.extend_from_slice(&check.to_be_bytes());
        self.raw(&header)?;
        self.raw(&packet.data)
    }

    /// Flush and hand back the output.
    ///
    /// # Errors
    ///
    /// The output cannot be flushed.
    pub fn finish(mut self) -> Result<W, NutError> {
        self.output.flush()?;
        Ok(self.output)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            streams: vec![
                StreamHeader {
                    class: Class::Video,
                    fourcc: b"H264".to_vec(),
                    time_base: Rational {
                        num: 1,
                        den: 61_440,
                    },
                    msb_pts_shift: 7,
                    max_pts_distance: 61_440,
                    decode_delay: 2,
                    flags: 0,
                    extradata: vec![1, 100, 0, 31, 0xFF],
                    video: Some([1920, 1080, 1, 1, 0]),
                    audio: None,
                    metadata: vec![("language".to_owned(), "aze".to_owned())],
                },
                StreamHeader {
                    class: Class::Audio,
                    fourcc: vec![0xFF, 0x00],
                    time_base: Rational {
                        num: 1,
                        den: 48_000,
                    },
                    msb_pts_shift: 14,
                    max_pts_distance: 48_000,
                    decode_delay: 0,
                    flags: 0,
                    extradata: vec![0x11, 0x90],
                    video: None,
                    audio: Some([48_000, 1, 2]),
                    metadata: Vec::new(),
                },
            ],
            metadata: vec![(
                "creation_time".to_owned(),
                "2026-09-25T10:00:00.000000Z".to_owned(),
            )],
            chapters: vec![Chapter {
                time_base: Rational { num: 1, den: 1000 },
                start: 2000,
                length: 1500,
                metadata: vec![("title".to_owned(), "Middle".to_owned())],
            }],
        }
    }

    #[test]
    fn numbers_round_trip_through_their_encodings() {
        for value in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            u64::from(u32::MAX),
            u64::MAX,
        ] {
            let mut out = Vec::new();
            put_v(&mut out, value);
            assert_eq!(Bytes::new(&out).v().expect("v"), value, "{value}");
        }
        for value in [0, 1, -1, 63, -64, 1 << 40, -(1 << 40)] {
            let mut out = Vec::new();
            put_s(&mut out, value);
            assert_eq!(Bytes::new(&out).s().expect("s"), value, "{value}");
        }
    }

    #[test]
    fn the_checksum_of_data_with_its_checksum_appended_is_zero() {
        let data = b"nut/multimedia container";
        let mut with = data.to_vec();
        with.extend_from_slice(&crc(0, data).to_be_bytes());
        assert_eq!(crc(0, &with), 0);
        assert_ne!(crc(0, data), 0);
    }

    #[test]
    fn what_the_writer_writes_the_reader_reads_back_exactly() {
        let header = header();
        let packets = vec![
            Packet {
                stream: 0,
                pts: 614_400,
                key: true,
                data: vec![0, 0, 1, 0x65, 1, 2, 3],
            },
            Packet {
                stream: 1,
                pts: 480_000,
                key: true,
                data: vec![0x21; 300],
            },
            Packet {
                stream: 0,
                pts: 620_544,
                key: false,
                data: vec![7; 9000],
            },
            Packet {
                stream: 0,
                pts: 616_448,
                key: false,
                data: Vec::new(),
            },
        ];
        let mut writer = Writer::new(Vec::new(), &header).expect("writer");
        for packet in &packets {
            writer.write(packet).expect("write");
        }
        let bytes = writer.finish().expect("finish");
        let mut reader = Reader::open(bytes.as_slice()).expect("open");
        assert_eq!(reader.header(), &header);
        let mut read = Vec::new();
        while let Some(packet) = reader.next_packet().expect("packet") {
            read.push(packet);
        }
        assert_eq!(read, packets);
    }

    #[test]
    fn a_corrupted_stream_is_an_error_not_a_panic() {
        let mut writer = Writer::new(Vec::new(), &header()).expect("writer");
        writer
            .write(&Packet {
                stream: 0,
                pts: 1,
                key: true,
                data: vec![9; 64],
            })
            .expect("write");
        let bytes = writer.finish().expect("finish");
        for cut in 0..bytes.len() {
            // Every truncation, and every single flipped byte, either reads or
            // fails; none may panic or allocate without bound.
            let _ =
                Reader::open(&bytes[..cut]).map(|mut r| while let Ok(Some(_)) = r.next_packet() {});
            let mut flipped = bytes.clone();
            flipped[cut] ^= 0x5A;
            let _ = Reader::open(flipped.as_slice())
                .map(|mut r| while let Ok(Some(_)) = r.next_packet() {});
        }
        assert!(matches!(
            Reader::open(&b"not a nut stream"[..]),
            Err(NutError::NoHeaders)
        ));
    }

    #[test]
    fn a_negative_timestamp_is_refused() {
        let mut writer = Writer::new(Vec::new(), &header()).expect("writer");
        assert!(
            writer
                .write(&Packet {
                    stream: 0,
                    pts: -1,
                    key: true,
                    data: Vec::new(),
                })
                .is_err()
        );
    }
}
