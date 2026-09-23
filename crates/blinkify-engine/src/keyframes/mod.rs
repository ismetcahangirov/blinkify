//! The per-stream keyframe index.
//!
//! The single most load-bearing structure in Blinkify: Epic #6 chooses between
//! a free stream copy and a smart-cut re-encode purely from whether a cut point
//! lands on a keyframe. An approximate index produces exports that are silently
//! re-encoded, or corrupt. So:
//!
//! - **Every keyframe is enumerated** from `ffprobe` packets, never inferred
//!   from a nominal GOP length — scene-cut detection puts keyframes wherever
//!   the content changes.
//! - **Timestamps are the stream's own**, in its time base: presentation *and*
//!   decode timestamp, and the byte position. Never a frame number, which VFR
//!   makes meaningless.
//! - **Open GOPs are identified** (see [`nal`]), because a keyframe whose
//!   leading pictures reference the previous GOP widens the window a cut
//!   depends on.
//! - **It is lazy.** A query into a region nobody has indexed reads that region
//!   — `ffprobe` seeks to the keyframe at or before it — and only that region.
//!   The rest of the file is completed in the background at background
//!   priority, a chunk at a time, and never blocks the interface.
//! - **It persists** in the content-keyed cache, so it survives a restart and
//!   is rebuilt when the file changes.
//! - **It knows every frame, not only the keyframes.** The same packet listing
//!   gives the presentation timestamp of every frame that is shown, so frame
//!   stepping (#28) and seeking a variable frame rate source (#29) move by
//!   real timestamps, never by a frame number times a nominal duration.

pub mod nal;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use nal::Codec;
pub use nal::PictureKind;

use crate::cache::{Cache, ContentKey};
use crate::orchestrator::{JobError, Orchestrator, Priority, SidecarCommand};
use crate::probe::{MediaInfo, Rational};

/// Seconds of media read per region query and per background step, unless
/// [`KeyframeIndex::with_chunk_seconds`] says otherwise.
const CHUNK_SECONDS: f64 = 60.0;

/// Packets after a keyframe that must be seen before its GOP is classified
/// without having seen the next keyframe. Leading pictures come straight after
/// their keyframe in decode order; a reorder depth beyond this does not occur
/// in practice.
const LEADING_WINDOW: u32 = 16;

/// The cache format version. A change to [`Stored`] bumps it, and an entry
/// with any other version is ignored and rebuilt rather than misread.
const FORMAT_VERSION: u32 = 2;

/// How far before a region's limit its frame list is trusted. Packets arrive
/// in decode order, so a frame shown just before the limit can be decoded
/// just after it and never read; reordering never reaches this far.
const REORDER_MARGIN_SECONDS: f64 = 2.0;

/// Whether a cut at a keyframe can depend on the previous GOP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum GopKind {
    /// Nothing at or after this keyframe references anything before it.
    Closed,
    /// Leading pictures after this keyframe reference the previous GOP. A
    /// stream copy starting here loses them; the safe cut window is wider.
    Open,
}

/// One keyframe of one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Keyframe {
    /// Presentation timestamp, in the stream's time base.
    #[ts(type = "number")]
    pub pts: i64,
    /// Decode timestamp, in the stream's time base.
    #[ts(type = "number | null")]
    pub dts: Option<i64>,
    /// Byte offset of the packet in the file, where the container knows it.
    #[ts(type = "number | null")]
    pub pos: Option<u64>,
    pub gop: GopKind,
    /// What the first coded picture is, where it could be read.
    pub picture: Option<PictureKind>,
    /// Pictures presented before this one follow it in decode order.
    pub has_leading_pictures: bool,
}

/// Why the index could not answer.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("stream {0} is not a video stream of this file")]
    NoSuchStream(u32),
    #[error("could not read packets: {0}")]
    Engine(#[source] JobError),
    #[error("the file could not be read: {0}")]
    Io(#[source] std::io::Error),
}

/// Background indexing progress for one file, the payload of the index
/// progress event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IndexProgress {
    pub path: String,
    /// Fraction of the file's duration indexed, over every video stream.
    pub fraction: f64,
    pub complete: bool,
}

/// What is known about one keyframe while its GOP is still being observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    keyframe: Keyframe,
    size: u64,
    /// Packets seen after it in decode order, up to [`LEADING_WINDOW`].
    observed_after: u32,
    /// The next keyframe, or the end of the stream, has been seen.
    closed_out: bool,
}

/// A set of half-open `[start, end)` tick ranges, kept sorted and merged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Coverage(Vec<(i64, i64)>);

impl Coverage {
    fn add(&mut self, start: i64, end: i64) {
        if end <= start {
            return;
        }
        self.0.push((start, end));
        self.0.sort_unstable();
        let mut merged: Vec<(i64, i64)> = Vec::with_capacity(self.0.len());
        for &(start, end) in &self.0 {
            match merged.last_mut() {
                Some(last) if start <= last.1 => last.1 = last.1.max(end),
                _ => merged.push((start, end)),
            }
        }
        self.0 = merged;
    }

    fn contains(&self, tick: i64) -> bool {
        self.0
            .iter()
            .any(|&(start, end)| start <= tick && tick < end)
    }

    /// The end of the covered range containing `tick`.
    fn end_of_range(&self, tick: i64) -> Option<i64> {
        self.0
            .iter()
            .find(|&&(start, end)| start <= tick && tick < end)
            .map(|&(_, end)| end)
    }

    /// Total ticks covered within `[0, limit)`.
    fn covered_within(&self, limit: i64) -> i64 {
        self.0
            .iter()
            .map(|&(start, end)| end.min(limit).saturating_sub(start.max(0)).max(0))
            .sum()
    }
}

/// The index of one video stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StreamIndex {
    stream: u32,
    time_base: Rational,
    /// Ticks from the start of the stream to its end, where known.
    end: Option<i64>,
    /// First keyframe at or before the stream's first packet. Coverage before
    /// it is implied: nothing precedes the first packet.
    entries: BTreeMap<i64, Entry>,
    coverage: Coverage,
    /// The presentation timestamp of every shown frame read so far — packets
    /// the container marks as discarded (an edit list's pre-roll) are not
    /// shown and are not here.
    frames: BTreeSet<i64>,
    /// Where `frames` is complete. Narrower than `coverage` at a region's
    /// end, by [`REORDER_MARGIN_SECONDS`].
    frame_coverage: Coverage,
}

impl StreamIndex {
    fn seconds_to_ticks(&self, seconds: f64) -> i64 {
        let Some(per_tick) = self.time_base.value().filter(|v| *v > 0.0) else {
            return 0;
        };
        let ticks = (seconds / per_tick).round();
        // Saturating: `as` from a float clamps to the integer's range, and a
        // non-finite value (a zero time base was filtered out above) is zero.
        #[allow(clippy::cast_possible_truncation)]
        let ticks = if ticks.is_finite() { ticks as i64 } else { 0 };
        ticks
    }

    fn ticks_to_seconds(&self, ticks: i64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let ticks = ticks as f64;
        ticks * self.time_base.value().unwrap_or(0.0)
    }

    fn complete(&self) -> bool {
        self.coverage.contains(0) && self.coverage.end_of_range(0) == Some(i64::MAX)
    }

    fn fraction(&self) -> f64 {
        if self.complete() {
            return 1.0;
        }
        match self.end.filter(|end| *end > 0) {
            #[allow(clippy::cast_precision_loss)]
            Some(end) => (self.coverage.covered_within(end) as f64 / end as f64).min(0.999),
            None => 0.0,
        }
    }

    fn keyframes(&self) -> impl Iterator<Item = Keyframe> + '_ {
        self.entries.values().map(|entry| entry.keyframe)
    }
}

/// One packet of `ffprobe -show_entries packet=pts,dts,pos,size,flags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Packet {
    pts: Option<i64>,
    dts: Option<i64>,
    pos: Option<u64>,
    size: u64,
    key: bool,
    /// The container says the packet is decoded but not shown.
    discard: bool,
}

/// `pts=…|dts=…|size=…|pos=…|flags=K__`, one packet per line.
fn parse_packets(text: &str) -> Vec<Packet> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut packet = Packet {
                pts: None,
                dts: None,
                pos: None,
                size: 0,
                key: false,
                discard: false,
            };
            for field in line.trim().split('|') {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                match key {
                    "pts" => packet.pts = value.parse().ok(),
                    "dts" => packet.dts = value.parse().ok(),
                    "pos" => packet.pos = value.parse().ok(),
                    "size" => packet.size = value.parse().unwrap_or(0),
                    "flags" => {
                        packet.key = value.starts_with('K');
                        packet.discard = value.contains('D');
                    }
                    _ => {}
                }
            }
            packet
        })
        .collect()
}

/// Everything learned from reading one region.
struct Region {
    packets: Vec<Packet>,
    /// The region ended at the end of the stream rather than at its limit.
    reached_end: bool,
    /// The requested end, in ticks.
    limit: i64,
    /// How far before `limit` the frame list is complete.
    margin: i64,
}

/// The keyframe index of every video stream of one file.
#[derive(Debug)]
pub struct KeyframeIndex {
    path: PathBuf,
    key: Option<ContentKey>,
    orchestrator: Orchestrator,
    cache: Option<Cache>,
    iso_bmff: bool,
    codecs: BTreeMap<u32, Option<Codec>>,
    streams: BTreeMap<u32, Mutex<StreamIndex>>,
    cancel_background: AtomicBool,
    /// Region reads performed, for tests: a restored index performs none.
    reads: AtomicU32,
    chunk_seconds: f64,
}

impl KeyframeIndex {
    /// The index for `path`, restored from `cache` if this exact content was
    /// indexed before, and otherwise empty, to be filled lazily.
    ///
    /// # Errors
    ///
    /// The file cannot be read to compute its content key.
    pub fn open(
        path: &Path,
        info: &MediaInfo,
        orchestrator: Orchestrator,
        cache: Option<Cache>,
    ) -> Result<Self, IndexError> {
        let key = if cache.is_some() {
            Some(ContentKey::of(path).map_err(IndexError::Io)?)
        } else {
            None
        };
        let stored = match (&cache, &key) {
            (Some(cache), Some(key)) => cache
                .read(&cache.path("keyframes", key, ".json"))
                .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
                .filter(|stored| stored.version == FORMAT_VERSION),
            _ => None,
        };
        let mut restored: BTreeMap<u32, StreamIndex> = stored
            .map(|stored| {
                stored
                    .streams
                    .into_iter()
                    .map(|stream| (stream.stream, stream))
                    .collect()
            })
            .unwrap_or_default();

        let mut streams = BTreeMap::new();
        let mut codecs = BTreeMap::new();
        for (stream, video) in info.video() {
            if video.is_attached_picture {
                continue;
            }
            let time_base = stream.time_base.unwrap_or(Rational { num: 1, den: 1 });
            let index = restored.remove(&stream.index).unwrap_or_else(|| {
                let mut index = StreamIndex {
                    stream: stream.index,
                    time_base,
                    end: None,
                    entries: BTreeMap::new(),
                    coverage: Coverage::default(),
                    frames: BTreeSet::new(),
                    frame_coverage: Coverage::default(),
                };
                let duration = stream.duration_seconds.or(info.container.duration_seconds);
                index.end = duration.map(|seconds| index.seconds_to_ticks(seconds));
                index
            });
            codecs.insert(stream.index, Codec::from_name(stream.codec.as_deref()));
            streams.insert(stream.index, Mutex::new(index));
        }

        Ok(Self {
            path: path.to_path_buf(),
            key,
            orchestrator,
            cache,
            iso_bmff: info.container.format_name.contains("mp4")
                || info.container.format_name.contains("mov"),
            codecs,
            streams,
            cancel_background: AtomicBool::new(false),
            reads: AtomicU32::new(0),
            chunk_seconds: CHUNK_SECONDS,
        })
    }

    /// Read regions of `seconds` rather than a minute. Smaller regions mean
    /// more region boundaries, which is what the exactness tests want to cross.
    #[must_use]
    pub fn with_chunk_seconds(mut self, seconds: f64) -> Self {
        if seconds.is_finite() && seconds > 0.0 {
            self.chunk_seconds = seconds;
        }
        self
    }

    /// The video streams this index covers.
    pub fn streams(&self) -> impl Iterator<Item = u32> + '_ {
        self.streams.keys().copied()
    }

    /// The nearest keyframe at or before `pts`, reading that region first if
    /// nobody has. `None` only before the stream's first keyframe.
    ///
    /// # Errors
    ///
    /// The stream does not exist, or its packets could not be read.
    pub fn at_or_before(&self, stream: u32, pts: i64) -> Result<Option<Keyframe>, IndexError> {
        self.ensure_covered(stream, pts, Priority::Interactive)?;
        Ok(self
            .lock(stream)?
            .entries
            .range(..=pts)
            .next_back()
            .map(|(_, entry)| entry.keyframe))
    }

    /// The nearest keyframe at or after `pts`. `None` after the last one.
    ///
    /// # Errors
    ///
    /// As [`KeyframeIndex::at_or_before`].
    pub fn at_or_after(&self, stream: u32, pts: i64) -> Result<Option<Keyframe>, IndexError> {
        let mut from = pts;
        loop {
            self.ensure_covered(stream, from, Priority::Interactive)?;
            let index = self.lock(stream)?;
            if let Some((_, entry)) = index.entries.range(pts..).next() {
                return Ok(Some(entry.keyframe));
            }
            match index.coverage.end_of_range(from) {
                Some(i64::MAX) | None => return Ok(None),
                Some(end) => from = end,
            }
        }
    }

    /// The frame shown at `pts`: the newest frame whose presentation
    /// timestamp is at or before it. `None` before the first frame.
    ///
    /// # Errors
    ///
    /// As [`KeyframeIndex::at_or_before`].
    pub fn frame_at_or_before(&self, stream: u32, pts: i64) -> Result<Option<i64>, IndexError> {
        self.ensure_frames_covered(stream, pts)?;
        Ok(self.lock(stream)?.frames.range(..=pts).next_back().copied())
    }

    /// The first frame shown strictly after `pts`. `None` after the last.
    ///
    /// # Errors
    ///
    /// As [`KeyframeIndex::at_or_before`].
    pub fn frame_after(&self, stream: u32, pts: i64) -> Result<Option<i64>, IndexError> {
        let mut from = pts.saturating_add(1);
        loop {
            self.ensure_frames_covered(stream, from)?;
            let index = self.lock(stream)?;
            if let Some(next) = index.frames.range(pts.saturating_add(1)..).next() {
                return Ok(Some(*next));
            }
            match index.frame_coverage.end_of_range(from) {
                Some(i64::MAX) | None => return Ok(None),
                Some(end) => from = end,
            }
        }
    }

    /// The last frame shown strictly before `pts`. `None` before the first.
    ///
    /// # Errors
    ///
    /// As [`KeyframeIndex::at_or_before`].
    pub fn frame_before(&self, stream: u32, pts: i64) -> Result<Option<i64>, IndexError> {
        self.frame_at_or_before(stream, pts.saturating_sub(1))
    }

    /// Whether `pts` is exactly a keyframe's presentation timestamp.
    ///
    /// # Errors
    ///
    /// As [`KeyframeIndex::at_or_before`].
    pub fn is_keyframe(&self, stream: u32, pts: i64) -> Result<bool, IndexError> {
        self.ensure_covered(stream, pts, Priority::Interactive)?;
        Ok(self.lock(stream)?.entries.contains_key(&pts))
    }

    /// Every keyframe indexed so far, in presentation order.
    ///
    /// # Errors
    ///
    /// The stream does not exist.
    pub fn keyframes(&self, stream: u32) -> Result<Vec<Keyframe>, IndexError> {
        Ok(self.lock(stream)?.keyframes().collect())
    }

    /// The stream's time base, for converting ticks to seconds.
    ///
    /// # Errors
    ///
    /// The stream does not exist.
    pub fn time_base(&self, stream: u32) -> Result<Rational, IndexError> {
        Ok(self.lock(stream)?.time_base)
    }

    /// `seconds` in the stream's ticks.
    ///
    /// # Errors
    ///
    /// The stream does not exist.
    pub fn ticks(&self, stream: u32, seconds: f64) -> Result<i64, IndexError> {
        Ok(self.lock(stream)?.seconds_to_ticks(seconds))
    }

    /// Whether every stream is indexed end to end.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.streams.values().all(|index| {
            index
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .complete()
        })
    }

    /// The fraction of the file indexed, averaged over its video streams.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        let fractions: Vec<f64> = self
            .streams
            .values()
            .map(|index| {
                index
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .fraction()
            })
            .collect();
        if fractions.is_empty() {
            return 1.0;
        }
        #[allow(clippy::cast_precision_loss)]
        let count = fractions.len() as f64;
        fractions.iter().sum::<f64>() / count
    }

    /// Region reads performed since this index was opened.
    #[must_use]
    pub fn reads(&self) -> u32 {
        self.reads.load(Ordering::SeqCst)
    }

    /// Index everything not yet indexed, a chunk at a time at background
    /// priority, reporting progress after each chunk, and persist the result.
    /// Returns early, without error, if [`KeyframeIndex::stop_background`] is
    /// called. Blocks: run it on a background thread.
    ///
    /// # Errors
    ///
    /// Packets could not be read.
    pub fn complete_in_background(
        &self,
        mut on_progress: impl FnMut(IndexProgress),
    ) -> Result<(), IndexError> {
        let streams: Vec<u32> = self.streams().collect();
        for stream in streams {
            let mut from = 0_i64;
            loop {
                if self.cancel_background.load(Ordering::SeqCst) {
                    self.persist();
                    return Ok(());
                }
                // The next tick not yet covered, from the start.
                let next = {
                    let index = self.lock(stream)?;
                    if index.complete() {
                        break;
                    }
                    let mut tick = from;
                    while let Some(end) = index.coverage.end_of_range(tick) {
                        if end == i64::MAX {
                            break;
                        }
                        tick = end;
                    }
                    tick
                };
                if self.lock(stream)?.coverage.end_of_range(next) == Some(i64::MAX) {
                    break;
                }
                self.read_region(stream, next, Priority::Background)?;
                from = next;
                on_progress(self.progress());
            }
        }
        self.persist();
        on_progress(self.progress());
        Ok(())
    }

    /// Stop [`KeyframeIndex::complete_in_background`] after its current chunk.
    pub fn stop_background(&self) {
        self.cancel_background.store(true, Ordering::SeqCst);
    }

    fn progress(&self) -> IndexProgress {
        IndexProgress {
            path: self.path.display().to_string(),
            fraction: self.fraction(),
            complete: self.is_complete(),
        }
    }

    /// Write the index to the cache — complete or not; a partial index still
    /// saves the regions already read.
    pub fn persist(&self) {
        let (Some(cache), Some(key)) = (&self.cache, &self.key) else {
            return;
        };
        let stored = Stored {
            version: FORMAT_VERSION,
            streams: self
                .streams
                .values()
                .map(|index| index.lock().unwrap_or_else(PoisonError::into_inner).clone())
                .collect(),
        };
        if let Ok(bytes) = serde_json::to_vec(&stored) {
            let _ = cache.write(&cache.path("keyframes", key, ".json"), &bytes);
        }
    }

    fn lock(&self, stream: u32) -> Result<std::sync::MutexGuard<'_, StreamIndex>, IndexError> {
        Ok(self
            .streams
            .get(&stream)
            .ok_or(IndexError::NoSuchStream(stream))?
            .lock()
            .unwrap_or_else(PoisonError::into_inner))
    }

    fn ensure_frames_covered(&self, stream: u32, pts: i64) -> Result<(), IndexError> {
        let covered = self.lock(stream)?.frame_coverage.contains(pts);
        if covered {
            return Ok(());
        }
        // A region read from `pts` covers frames from its keyframe to its
        // limit less the reorder margin, which always includes `pts`.
        self.read_region(stream, pts.max(0), Priority::Interactive)
    }

    fn ensure_covered(&self, stream: u32, pts: i64, priority: Priority) -> Result<(), IndexError> {
        let covered = {
            let index = self.lock(stream)?;
            index.coverage.contains(pts.max(0))
        };
        if covered {
            return Ok(());
        }
        self.read_region(stream, pts.max(0), priority)
    }

    /// Read `CHUNK_SECONDS` of packets starting at the keyframe at or before
    /// `from`, and merge them in. The lock is not held while `ffprobe` runs,
    /// so queries into different regions proceed concurrently.
    fn read_region(&self, stream: u32, from: i64, priority: Priority) -> Result<(), IndexError> {
        let (start_seconds, end_seconds, limit, stream_end, two_seconds, margin) = {
            let index = self.lock(stream)?;
            let start = index.ticks_to_seconds(from);
            let end = start + self.chunk_seconds;
            (
                start,
                end,
                index.seconds_to_ticks(end),
                index.end,
                index.seconds_to_ticks(2.0),
                index.seconds_to_ticks(REORDER_MARGIN_SECONDS.min(self.chunk_seconds / 2.0)),
            )
        };
        let output = self
            .orchestrator
            .run_to_end(
                SidecarCommand::ffprobe()
                    .option("-v", "error")
                    .option("-select_streams", stream.to_string())
                    .option("-show_entries", "packet=pts,dts,pos,size,flags")
                    .option("-of", "compact=p=0")
                    .option(
                        "-read_intervals",
                        interval(from, start_seconds, end_seconds),
                    )
                    .input(&self.path),
                priority,
            )
            .map_err(IndexError::Engine)?;
        self.reads.fetch_add(1, Ordering::SeqCst);

        let packets = parse_packets(&String::from_utf8_lossy(&output.stdout));
        // `ffprobe` stops *before* printing the first packet past the
        // interval's end, so the last timestamp read is always below the limit
        // and says nothing about whether the stream ended. The stream's known
        // length does. Without one, a region that stopped well short of its
        // limit ran out of packets.
        let last = packets.iter().filter_map(|packet| packet.pts).max();
        let reached_end = match stream_end {
            Some(stream_end) => limit >= stream_end,
            None => last.is_none_or(|last| last.saturating_add(two_seconds) < limit),
        };
        let region = Region {
            packets,
            reached_end,
            limit,
            margin,
        };
        let pictures = self.classify_pictures(stream, &region);
        let mut index = self.lock(stream)?;
        merge(&mut index, &region, from, &pictures);
        Ok(())
    }

    /// Read the first picture of each keyframe packet, where the container
    /// makes that possible. Done outside the lock: it is file I/O.
    fn classify_pictures(&self, stream: u32, region: &Region) -> BTreeMap<i64, PictureKind> {
        let Some(Some(codec)) = self.codecs.get(&stream).copied() else {
            return BTreeMap::new();
        };
        if !self.iso_bmff {
            return BTreeMap::new();
        }
        region
            .packets
            .iter()
            .filter(|packet| packet.key)
            .filter_map(|packet| {
                let kind = nal::picture_kind(&self.path, codec, packet.pos?, packet.size)?;
                Some((packet.pts?, kind))
            })
            .collect()
    }
}

/// The `-read_intervals` value for a region.
///
/// A region at the start of the stream reads from the beginning of the file
/// rather than seeking to zero: with an MP4 edit list the stream's first
/// keyframe has a negative timestamp, and a seek to zero lands on the *next*
/// keyframe, silently skipping every frame before it.
fn interval(from: i64, start_seconds: f64, end_seconds: f64) -> String {
    if from <= 0 {
        format!("%{end_seconds:.6}")
    } else {
        format!("{start_seconds:.6}%{end_seconds:.6}")
    }
}

/// Merge one region's packets into the index.
fn merge(
    index: &mut StreamIndex,
    region: &Region,
    from: i64,
    pictures: &BTreeMap<i64, PictureKind>,
) {
    let mut current: Option<i64> = None;
    let mut first_key: Option<i64> = None;
    for packet in &region.packets {
        if !packet.discard
            && let Some(pts) = packet.pts
        {
            index.frames.insert(pts);
        }
        if packet.key {
            let Some(pts) = packet.pts.or(packet.dts) else {
                continue;
            };
            if let Some(previous) = current.and_then(|p| index.entries.get_mut(&p)) {
                previous.closed_out = true;
            }
            first_key.get_or_insert(pts);
            let entry = index.entries.entry(pts).or_insert(Entry {
                keyframe: Keyframe {
                    pts,
                    dts: packet.dts,
                    pos: packet.pos,
                    gop: GopKind::Closed,
                    picture: pictures.get(&pts).copied(),
                    has_leading_pictures: false,
                },
                size: packet.size,
                observed_after: 0,
                closed_out: false,
            });
            // A keyframe seen again, from a later region that started on it,
            // is observed afresh: count its GOP from zero.
            entry.observed_after = 0;
            current = Some(pts);
        } else if let Some(entry) = current.and_then(|p| index.entries.get_mut(&p)) {
            entry.observed_after = entry.observed_after.saturating_add(1);
            if packet.pts.is_some_and(|pts| pts < entry.keyframe.pts) {
                entry.keyframe.has_leading_pictures = true;
            }
        }
    }
    if region.reached_end
        && let Some(last) = current.and_then(|p| index.entries.get_mut(&p))
    {
        last.closed_out = true;
    }
    for entry in index.entries.values_mut() {
        if entry.closed_out || entry.observed_after >= LEADING_WINDOW {
            entry.keyframe.gop =
                classify(entry.keyframe.picture, entry.keyframe.has_leading_pictures);
        }
    }

    // Coverage: from the first keyframe read (the one at or before `from`) —
    // or from zero when the read began at the start of the stream — to the
    // limit, or to the end of time when the stream ended first.
    let start = if from <= 0 {
        i64::MIN
    } else {
        first_key.unwrap_or(from).min(from)
    };
    let end = if region.reached_end {
        i64::MAX
    } else {
        region.limit
    };
    index.coverage.add(start, end);
    let frames_end = if region.reached_end {
        i64::MAX
    } else {
        region.limit.saturating_sub(region.margin)
    };
    index.frame_coverage.add(start, frames_end);
}

/// Open or closed, from what the picture is and what follows it.
fn classify(picture: Option<PictureKind>, has_leading_pictures: bool) -> GopKind {
    match picture {
        // IDR and BLA close the GOP whatever follows: `IDR_W_RADL` leading
        // pictures are decodable, and BLA discards its RASL pictures.
        Some(kind) if kind.closes_gop() => GopKind::Closed,
        // CRA, a recovery point, or a picture that could not be read: open
        // exactly when leading pictures follow. For an unreadable picture this
        // errs towards "open" — a wider cut window, never a corrupt cut.
        _ if has_leading_pictures => GopKind::Open,
        _ => GopKind::Closed,
    }
}

/// The cache file.
#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    version: u32,
    streams: Vec<StreamIndex>,
}

/// A shareable index, completed in the background on its own thread.
///
/// Returns the index at once; queries work immediately, reading what they
/// need, while the background thread fills in the rest.
pub fn spawn_background(
    index: Arc<KeyframeIndex>,
    on_progress: impl FnMut(IndexProgress) + Send + 'static,
) -> std::thread::JoinHandle<Result<(), IndexError>> {
    std::thread::spawn(move || index.complete_in_background(on_progress))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn coverage_merges_overlapping_and_adjacent_ranges() {
        let mut coverage = Coverage::default();
        coverage.add(10, 20);
        coverage.add(30, 40);
        coverage.add(15, 30);
        assert_eq!(coverage.0, vec![(10, 40)]);
        assert!(coverage.contains(39));
        assert!(!coverage.contains(40));
        assert_eq!(coverage.end_of_range(12), Some(40));
    }

    #[test]
    fn packets_parse_from_compact_output() {
        let packets = parse_packets(
            "pts=10752|dts=9728|size=9694|pos=83252|flags=K__\npts=N/A|dts=11|size=1|pos=N/A|flags=___\n",
        );
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].pts, Some(10752));
        assert!(packets[0].key);
        assert_eq!(packets[1].pts, None);
        assert_eq!(packets[1].pos, None);
    }

    #[test]
    fn an_idr_with_leading_pictures_is_still_closed() {
        assert_eq!(classify(Some(PictureKind::Idr), true), GopKind::Closed);
        assert_eq!(classify(Some(PictureKind::Cra), true), GopKind::Open);
        assert_eq!(classify(Some(PictureKind::Cra), false), GopKind::Closed);
        assert_eq!(
            classify(Some(PictureKind::RecoveryPoint), true),
            GopKind::Open
        );
        assert_eq!(classify(None, true), GopKind::Open);
        assert_eq!(classify(None, false), GopKind::Closed);
    }
}
