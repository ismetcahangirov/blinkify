//! What a media file contains.
//!
//! The export planner in Epic #6 decides copy versus re-encode from this, so a
//! probe that is wrong or incomplete makes every downstream guarantee false.
//! The facts that editors most often get wrong are the ones this module works
//! hardest at:
//!
//! - **Rotation** lives in side data, not in the dimensions. A portrait phone
//!   video probes as 1920x1080 with a 90-degree display matrix; report the
//!   display size too, or every phone video previews sideways.
//! - **Variable frame rate** is detected from packet timestamps, not assumed
//!   from `r_frame_rate`, which for a VFR source is merely a base.
//! - **Edit lists** are read from the MP4 boxes, because `ffprobe` applies them
//!   silently and does not say so.
//! - **HDR** is reported only when present — transfer characteristics, and the
//!   mastering display and content light level metadata, from the stream or
//!   from the first frame's side data, wherever the file keeps them.
//!
//! Every fact comes from `ffprobe` run through the [`Orchestrator`], never from
//! a separate shell-out.

pub mod edit_list;
mod ffprobe;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

pub use edit_list::{EditList, EditListEntry};

use crate::orchestrator::{JobError, Orchestrator, Priority, SidecarCommand};

/// A ratio as FFmpeg writes it: `30000/1001`, `16:9`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Rational {
    // Numbers, not `bigint`, in TypeScript: every ratio FFmpeg prints fits a
    // double exactly, and a `bigint` cannot be divided by a `number`.
    #[ts(type = "number")]
    pub num: i64,
    #[ts(type = "number")]
    pub den: i64,
}

impl Rational {
    /// The value as a float, or `None` for a zero denominator — which FFmpeg
    /// uses to mean "unknown".
    #[must_use]
    pub fn value(self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        (self.den != 0).then(|| self.num as f64 / self.den as f64)
    }
}

/// Everything the probe learned about a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MediaInfo {
    pub container: ContainerInfo,
    pub streams: Vec<StreamInfo>,
}

impl MediaInfo {
    /// The video streams, in file order.
    pub fn video(&self) -> impl Iterator<Item = (&StreamInfo, &VideoInfo)> {
        self.streams.iter().filter_map(|stream| match &stream.kind {
            StreamKind::Video(video) => Some((stream, video.as_ref())),
            _ => None,
        })
    }

    /// The audio streams, in file order.
    pub fn audio(&self) -> impl Iterator<Item = (&StreamInfo, &AudioInfo)> {
        self.streams.iter().filter_map(|stream| match &stream.kind {
            StreamKind::Audio(audio) => Some((stream, audio)),
            _ => None,
        })
    }
}

/// Container-level facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContainerInfo {
    /// FFmpeg's demuxer names, e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format_name: String,
    pub format_long_name: Option<String>,
    pub duration_seconds: Option<f64>,
    pub bit_rate: Option<f64>,
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// Every MP4 or `QuickTime` edit list, by track. Empty for other containers.
    pub edit_lists: Vec<EditList>,
    /// Whether any edit list shifts the presentation timeline.
    pub has_edit_list: bool,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Chapter {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub title: Option<String>,
}

/// One stream of the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StreamInfo {
    pub index: u32,
    /// FFmpeg's codec name: `h264`, `hevc`, `aac`.
    pub codec: Option<String>,
    pub profile: Option<String>,
    /// The codec's level as FFmpeg reports it (`41` for H.264 4.1, `153` for
    /// HEVC 5.1), or `None` where the codec has none.
    pub level: Option<i32>,
    pub bit_rate: Option<f64>,
    pub duration_seconds: Option<f64>,
    pub start_seconds: Option<f64>,
    pub time_base: Option<Rational>,
    pub is_default: bool,
    /// `SHA256:…` of the stream's codec configuration record — for H.264
    /// the `avcC` with its SPS and PPS. Absent when the codec keeps its
    /// configuration in-band (VP9) or the file has none.
    #[serde(default)]
    pub extradata_hash: Option<String>,
    pub kind: StreamKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[ts(export)]
pub enum StreamKind {
    // Boxed: a video stream carries colour, HDR and frame-rate detail that the
    // other kinds do not, and every stream should not pay for it.
    Video(Box<VideoInfo>),
    Audio(AudioInfo),
    Subtitle,
    /// A file carried inside the container — fonts in Matroska, usually.
    Attachment {
        filename: Option<String>,
        mimetype: Option<String>,
    },
    Data,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VideoInfo {
    /// Coded dimensions, as stored.
    pub width: u32,
    pub height: u32,
    /// Dimensions as displayed, after rotation. What the preview must show.
    pub display_width: u32,
    pub display_height: u32,
    /// Counter-clockwise display rotation in degrees, normalised to 0, 90,
    /// 180 or 270 — the angle `ffprobe` reports from the display matrix.
    pub rotation: u32,
    pub pixel_format: Option<String>,
    pub bit_depth: Option<u8>,
    pub chroma_subsampling: Option<ChromaSubsampling>,
    pub sample_aspect_ratio: Option<Rational>,
    pub frame_rate: FrameRate,
    /// `progressive`, `tt`, `bb`, `tb`, `bt`, as FFmpeg names them.
    pub field_order: Option<String>,
    pub has_b_frames: bool,
    pub color: Color,
    /// Present only when the stream is HDR.
    pub hdr: Option<Hdr>,
    /// A cover image rather than a moving picture.
    pub is_attached_picture: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ChromaSubsampling {
    #[serde(rename = "4:2:0")]
    #[ts(rename = "4:2:0")]
    Yuv420,
    #[serde(rename = "4:2:2")]
    #[ts(rename = "4:2:2")]
    Yuv422,
    #[serde(rename = "4:4:4")]
    #[ts(rename = "4:4:4")]
    Yuv444,
    #[serde(rename = "4:0:0")]
    #[ts(rename = "4:0:0")]
    Gray,
    #[serde(rename = "rgb")]
    #[ts(rename = "rgb")]
    Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FrameRate {
    /// Frames over duration.
    pub average: Option<Rational>,
    /// FFmpeg's `r_frame_rate`: the lowest rate that represents every
    /// timestamp exactly. For a VFR source this is a base, not a rate.
    pub real_base: Option<Rational>,
    pub mode: FrameRateMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum FrameRateMode {
    /// Every sampled frame lasts the same time, within one timestamp tick.
    Constant,
    /// Frame durations vary. Timestamp arithmetic must use real timestamps,
    /// never a frame number times a nominal duration.
    Variable,
    /// Too few frames to tell.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Color {
    /// `tv` (limited) or `pc` (full).
    pub range: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    /// Matrix coefficients; FFmpeg calls this `color_space`.
    pub matrix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Hdr {
    pub transfer: HdrTransfer,
    pub mastering_display: Option<MasteringDisplay>,
    pub content_light: Option<ContentLight>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum HdrTransfer {
    /// SMPTE ST 2084 — HDR10, Dolby Vision base layers.
    Pq,
    /// ARIB STD-B67 — broadcast HDR, many phones.
    Hlg,
    /// Mastering metadata is present but the transfer is not an HDR one.
    Other,
}

/// SMPTE ST 2086 mastering display colour volume. Chromaticities are CIE 1931
/// x and y; luminance is in candelas per square metre.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MasteringDisplay {
    pub red: [f64; 2],
    pub green: [f64; 2],
    pub blue: [f64; 2],
    pub white_point: [f64; 2],
    pub min_luminance: f64,
    pub max_luminance: f64,
}

/// CTA-861.3 content light level, in candelas per square metre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ContentLight {
    pub max_content: u32,
    pub max_frame_average: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioInfo {
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub channel_layout: Option<String>,
    pub sample_format: Option<String>,
    pub bits_per_sample: Option<u32>,
}

/// Why a file could not be probed. Each message names the reason in terms the
/// user can act on.
#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("{} does not exist", .0.display())]
    NotFound(PathBuf),
    #[error("{} is empty (0 bytes)", .0.display())]
    Empty(PathBuf),
    #[error("{} could not be read as media: {reason}", path.display())]
    Unreadable { path: PathBuf, reason: String },
    #[error("{} contains no audio or video", .0.display())]
    NoMediaStreams(PathBuf),
    #[error("{} disappeared or changed while it was being read", .0.display())]
    ChangedDuringProbe(PathBuf),
    #[error("{} could not be opened: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the media engine could not run ffprobe: {0}")]
    Engine(#[source] JobError),
}

/// The identity of one version of a file: probe results are immutable for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Version {
    path: PathBuf,
    size: u64,
    modified: Option<SystemTime>,
}

impl Version {
    fn of(path: &Path) -> Result<Self, ProbeError> {
        let metadata = std::fs::metadata(path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProbeError::NotFound(path.to_path_buf())
            } else {
                ProbeError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            size: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

/// How many probe results are kept in memory.
const CACHE_ENTRIES: usize = 512;

/// Probe results by file version, oldest first for eviction.
#[derive(Debug, Default)]
struct ProbeCache {
    results: HashMap<Version, Arc<MediaInfo>>,
    order: VecDeque<Version>,
}

/// Probes files, and remembers the answer for each version of each file.
#[derive(Debug)]
pub struct Prober {
    orchestrator: Orchestrator,
    cache: Mutex<ProbeCache>,
}

impl Prober {
    #[must_use]
    pub fn new(orchestrator: Orchestrator) -> Self {
        Self {
            orchestrator,
            cache: Mutex::new(ProbeCache::default()),
        }
    }

    /// Probe `path`, or return the cached result for this exact version of it
    /// — same path, same size, same modification time.
    ///
    /// # Errors
    ///
    /// See [`ProbeError`].
    pub fn probe(&self, path: &Path) -> Result<Arc<MediaInfo>, ProbeError> {
        let version = Version::of(path)?;
        if let Some(hit) = self.lock().results.get(&version) {
            return Ok(Arc::clone(hit));
        }
        let info = Arc::new(probe_uncached(&self.orchestrator, path, &version)?);
        let mut cache = self.lock();
        if cache
            .results
            .insert(version.clone(), Arc::clone(&info))
            .is_none()
        {
            cache.order.push_back(version);
        }
        while cache.order.len() > CACHE_ENTRIES {
            if let Some(oldest) = cache.order.pop_front() {
                cache.results.remove(&oldest);
            }
        }
        Ok(info)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ProbeCache> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn probe_uncached(
    orchestrator: &Orchestrator,
    path: &Path,
    version: &Version,
) -> Result<MediaInfo, ProbeError> {
    if version.size == 0 {
        return Err(ProbeError::Empty(path.to_path_buf()));
    }

    let run = |command: SidecarCommand| {
        orchestrator
            .run_to_end(command, Priority::Interactive)
            .map_err(|error| classify_failure(path, version, error))
    };

    let output = run(SidecarCommand::ffprobe()
        .option("-v", "error")
        .option("-print_format", "json")
        // The hash of each stream's codec configuration (SPS/PPS, avcC,
        // hvcC …): two sources whose packets are concatenated by a copy
        // must agree on it byte for byte (#39).
        .option("-show_data_hash", "sha256")
        .flags(&["-show_format", "-show_streams", "-show_chapters"])
        .input(path))?;
    let parsed: ffprobe::Output =
        serde_json::from_slice(&output.stdout).map_err(|error| ProbeError::Unreadable {
            path: path.to_path_buf(),
            reason: format!("ffprobe returned output that could not be parsed ({error})"),
        })?;

    let mut streams = Vec::with_capacity(parsed.streams.len());
    for stream in &parsed.streams {
        let mut info = ffprobe::stream_info(stream);
        if let StreamKind::Video(video) = &mut info.kind
            && !video.is_attached_picture
        {
            let packets = run(SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", stream.index.to_string())
                .option("-show_entries", "packet=pts")
                .option("-read_intervals", "%+30")
                .option("-of", "csv=p=0")
                .input(path))?;
            video.frame_rate.mode = ffprobe::frame_rate_mode(&packets.stdout);

            let frames = run(SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", stream.index.to_string())
                .option("-read_intervals", "%+#1")
                // `frame=:side_data` selects every key of each side-data entry;
                // `frame=side_data_list` selects the list and none of its keys.
                .option("-show_entries", "frame=:side_data")
                .option("-print_format", "json")
                .input(path))?;
            let first_frame: ffprobe::Frames =
                serde_json::from_slice(&frames.stdout).unwrap_or_default();
            video.hdr = ffprobe::hdr(stream, &first_frame, &video.color);
        }
        streams.push(info);
    }

    if !streams
        .iter()
        .any(|s| matches!(s.kind, StreamKind::Video(_) | StreamKind::Audio(_)))
    {
        return Err(ProbeError::NoMediaStreams(path.to_path_buf()));
    }

    let edit_lists = edit_list::read_edit_lists(path).map_err(|source| ProbeError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    // The answer is only for this version of the file. If it changed while
    // being read, the answer may describe neither version.
    if Version::of(path).ok().as_ref() != Some(version) {
        return Err(ProbeError::ChangedDuringProbe(path.to_path_buf()));
    }

    Ok(MediaInfo {
        container: ffprobe::container_info(&parsed, version.size, edit_lists),
        streams,
    })
}

/// Turn an `ffprobe` failure into the reason a person can read.
fn classify_failure(path: &Path, version: &Version, error: JobError) -> ProbeError {
    if Version::of(path).ok().as_ref() != Some(version) {
        return ProbeError::ChangedDuringProbe(path.to_path_buf());
    }
    match error {
        JobError::Failed { stderr_tail, .. } => ProbeError::Unreadable {
            path: path.to_path_buf(),
            reason: stderr_tail.last().map_or_else(
                || "ffprobe could not read it".to_owned(),
                |line| describe_ffprobe_error(line),
            ),
        },
        other => ProbeError::Engine(other),
    }
}

/// `ffprobe` prefixes its reason with the input URL; the user already knows
/// which file it was.
fn describe_ffprobe_error(line: &str) -> String {
    let reason = line
        .rsplit_once(": ")
        .map_or(line, |(_, reason)| reason)
        .trim();
    match reason {
        "Invalid data found when processing input" => {
            "the file is not a media format FFmpeg recognises, or it is damaged".to_owned()
        }
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_url_is_stripped_from_an_ffprobe_error() {
        assert_eq!(
            describe_ffprobe_error(
                "file:C:\\clips\\a.mp4: Invalid data found when processing input"
            ),
            "the file is not a media format FFmpeg recognises, or it is damaged"
        );
        assert_eq!(
            describe_ffprobe_error("[mov,mp4 @ 0000] moov atom not found"),
            "[mov,mp4 @ 0000] moov atom not found"
        );
    }

    #[test]
    fn a_zero_denominator_is_unknown_not_infinite() {
        assert_eq!(Rational { num: 0, den: 0 }.value(), None);
        assert_eq!(Rational { num: 30, den: 1 }.value(), Some(30.0));
    }
}
