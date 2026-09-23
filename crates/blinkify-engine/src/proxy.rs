//! Preview proxies, and the type that keeps them out of every export.
//!
//! A 4K or high-bit-rate source cannot be decoded fast enough to scrub on a
//! typical laptop. A proxy — the same picture at 540 lines in an all-intra
//! codec, where every frame is a keyframe and a seek is one decode — fixes
//! that. It is **offered, never forced** ([`proxy_advice`]), and it is used
//! **for preview only**.
//!
//! That last rule is the one that matters. A proxy that reached an export would
//! silently replace the user's footage with a 540-line MJPEG copy of it, which
//! is exactly the loss this product exists to prevent (`CLAUDE.md` section 1).
//! So the rule is a type, not a flag someone can forget:
//!
//! - [`ExportSource`] has a private field and one constructor,
//!   [`MediaAsset::export_source`], which returns the **original** file. There
//!   is no conversion from a [`Proxy`] or a [`PreviewSource`] into it.
//! - Export code (Epic #6) takes an `ExportSource`. It cannot be handed a
//!   proxy, because no value of that type can hold one.
//!
//! ```compile_fail
//! # use blinkify_engine::proxy::{ExportSource, Proxy};
//! fn export(_: ExportSource) {}
//! fn with(proxy: Proxy) {
//!     export(ExportSource::from(proxy)); // there is no such conversion
//! }
//! ```
//!
//! ```compile_fail
//! # use blinkify_engine::proxy::ExportSource;
//! // Nor can one be built around an arbitrary path: the field is private.
//! let _ = ExportSource { path: std::path::PathBuf::from("proxy.mkv") };
//! ```
//!
//! Proxies are written in ten-second segments. Cancelling deletes the segment
//! being written and keeps the complete ones, so generation resumes where it
//! stopped instead of starting again.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::cache::{Cache, ContentKey};
use crate::orchestrator::{
    CancelToken, JobError, JobOptions, JobProgress, Orchestrator, Priority, SidecarCommand,
};
use crate::probe::MediaInfo;

/// Proxy height in lines. Enough to judge framing and motion in a preview
/// panel; small enough that an all-intra decode is trivially fast.
pub const PROXY_HEIGHT: u32 = 540;

/// Seconds per proxy segment: the most work a cancellation throws away.
const SEGMENT_SECONDS: u32 = 10;

const FORMAT_VERSION: u32 = 1;

/// A source whose pixel count is above this is offered a proxy.
const HEAVY_PIXELS: u64 = 2560 * 1440;
/// Or whose bit rate is above this.
const HEAVY_BIT_RATE: f64 = 60_000_000.0;

/// The one file an export may read: always the original.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSource {
    path: PathBuf,
}

impl ExportSource {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// What the preview decodes: the original, or a proxy of it.
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewSource {
    Original(PathBuf),
    Proxy(Proxy),
}

impl PreviewSource {
    /// Whether the preview is showing a proxy — which the interface must say,
    /// persistently, whenever it is (#26).
    #[must_use]
    pub fn is_proxy(&self) -> bool {
        matches!(self, Self::Proxy(_))
    }
}

/// A media file in a project, and its proxy if one has been made.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaAsset {
    original: PathBuf,
    proxy: Option<Proxy>,
}

impl MediaAsset {
    #[must_use]
    pub fn new(original: PathBuf) -> Self {
        Self {
            original,
            proxy: None,
        }
    }

    /// Use `proxy` for preview. Export is unaffected; see the module docs.
    pub fn attach_proxy(&mut self, proxy: Proxy) {
        self.proxy = Some(proxy);
    }

    pub fn detach_proxy(&mut self) {
        self.proxy = None;
    }

    #[must_use]
    pub fn original(&self) -> &Path {
        &self.original
    }

    /// The source for export. The original file, whatever proxy is attached.
    #[must_use]
    pub fn export_source(&self) -> ExportSource {
        ExportSource {
            path: self.original.clone(),
        }
    }

    /// The source for preview: the proxy when there is one.
    #[must_use]
    pub fn preview_source(&self) -> PreviewSource {
        self.proxy.clone().map_or_else(
            || PreviewSource::Original(self.original.clone()),
            PreviewSource::Proxy,
        )
    }
}

/// A finished proxy: its segments, in order, covering the whole source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Proxy {
    version: u32,
    pub height: u32,
    pub segments: Vec<ProxySegment>,
}

impl Proxy {
    /// The proxy's segments as one input: an FFmpeg concat list beside them,
    /// written the first time it is asked for. Relative names, so a user
    /// directory with a quote in it needs no escaping. The list is text, not
    /// media, and lives with the proxy it describes.
    ///
    /// # Errors
    ///
    /// The proxy has no segments, or the list cannot be written.
    pub fn concat_list(&self) -> std::io::Result<PathBuf> {
        let dir = self
            .segments
            .first()
            .and_then(|segment| segment.file.parent())
            .ok_or_else(|| std::io::Error::other("the proxy has no segments"))?;
        let list = dir.join("segments.ffconcat");
        if !list.is_file() {
            let mut text = String::from("ffconcat version 1.0\n");
            for segment in &self.segments {
                let name = segment
                    .file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| std::io::Error::other("a proxy segment has no name"))?;
                text.push_str("file ");
                text.push_str(name);
                text.push('\n');
            }
            fs::write(&list, text)?;
        }
        Ok(list)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProxySegment {
    #[ts(type = "string")]
    pub file: PathBuf,
    /// Source time this segment starts and ends at, in seconds.
    pub start: f64,
    pub end: f64,
}

/// Why a proxy is worth offering.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "reason", rename_all = "kebab-case")]
#[ts(export)]
pub enum ProxyReason {
    Resolution { width: u32, height: u32 },
    BitRate { megabits_per_second: f64 },
}

/// Whether to offer a proxy for this source, and why. `None`: it scrubs as it
/// is. The user decides; this never starts one.
#[must_use]
pub fn proxy_advice(info: &MediaInfo) -> Vec<ProxyReason> {
    let mut reasons = Vec::new();
    if let Some((stream, video)) = info.video().find(|(_, video)| !video.is_attached_picture) {
        if u64::from(video.display_width) * u64::from(video.display_height) > HEAVY_PIXELS {
            reasons.push(ProxyReason::Resolution {
                width: video.display_width,
                height: video.display_height,
            });
        }
        if let Some(rate) = stream.bit_rate.or(info.container.bit_rate)
            && rate > HEAVY_BIT_RATE
        {
            reasons.push(ProxyReason::BitRate {
                megabits_per_second: rate / 1_000_000.0,
            });
        }
    }
    reasons
}

#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("the file has no video stream")]
    NoVideo,
    #[error("could not generate the proxy: {0}")]
    Engine(#[source] JobError),
    #[error("could not write the proxy: {0}")]
    Io(#[source] std::io::Error),
}

/// Makes proxies into a cache of their own.
///
/// Give it a different [`Cache`] from the one holding peaks and filmstrips: a
/// proxy of an hour of 4K is gigabytes, and sharing a budget would let it evict
/// every waveform — or be evicted by them, mid-edit.
#[derive(Debug, Clone)]
pub struct Proxies {
    orchestrator: Orchestrator,
    cache: Cache,
}

impl Proxies {
    #[must_use]
    pub fn new(orchestrator: Orchestrator, cache: Cache) -> Self {
        Self {
            orchestrator,
            cache,
        }
    }

    /// The proxy for `source`: finished already, or generated now — resuming
    /// from the segments a cancelled run left — at background priority.
    /// Blocks: run it off the UI thread.
    ///
    /// Cancelling through `cancel` kills the process and deletes the segment
    /// it was writing; complete segments stay for the next run.
    ///
    /// # Errors
    ///
    /// No video stream, or the process failed or was cancelled.
    pub fn generate(
        &self,
        source: &Path,
        info: &MediaInfo,
        cancel: &CancelToken,
        on_progress: impl FnMut(JobProgress) + Send + 'static,
    ) -> Result<Proxy, ProxyError> {
        let (stream, _) = info
            .video()
            .find(|(_, video)| !video.is_attached_picture)
            .ok_or(ProxyError::NoVideo)?;
        let dir = self.directory(source)?;
        fs::create_dir_all(&dir).map_err(ProxyError::Io)?;
        let manifest = dir.join("proxy.json");
        if let Some(proxy) = read_manifest(&manifest) {
            return Ok(proxy);
        }

        // Resume after the last complete segment.
        let done = completed_segments(&dir);
        remove_incomplete(&dir, &done);
        let resume_at = done.last().map_or(0.0, |segment| segment.end);
        let start_number = u32::try_from(done.len()).unwrap_or(0);
        let duration = info.container.duration_seconds.unwrap_or(0.0);
        let remaining = std::time::Duration::from_secs_f64((duration - resume_at).max(0.0));

        let mut command = SidecarCommand::ffmpeg().option("-loglevel", "error");
        if resume_at > 0.0 {
            // Input seek: decodes from the keyframe before, and outputs from
            // exactly here. The offset keeps segment timestamps continuous.
            command = command.option("-ss", format!("{resume_at:.6}"));
        }
        command = command
            .input(source)
            .option("-map", format!("0:{}", stream.index))
            .flags(&["-an", "-sn", "-dn"])
            .option("-vf", format!("scale=-2:{PROXY_HEIGHT}:flags=bicubic"))
            // All-intra: every frame a keyframe, so a seek is one decode.
            // Larger on disk than a long-GOP proxy, which is the point.
            .option("-c:v", "mjpeg")
            .option("-q:v", "5")
            .option("-pix_fmt", "yuvj420p")
            .option("-output_ts_offset", format!("{resume_at:.6}"))
            .option("-f", "segment")
            .option("-segment_time", SEGMENT_SECONDS.to_string())
            .option("-segment_format", "matroska")
            .option("-reset_timestamps", "0")
            .option("-segment_start_number", start_number.to_string())
            .option("-segment_list_type", "csv")
            .option("-segment_list", dir.join(format!("list-{start_number:05}.csv")))
            .output_file(&dir.join("seg_%05d.mkv"))
            .report_progress(remaining);

        let outcome = self
            .orchestrator
            .run(
                command,
                Priority::Background,
                JobOptions::default()
                    .cancel_token(cancel.clone())
                    .on_progress(on_progress),
            )
            .wait();

        // Whatever happened, only complete segments may remain.
        let done = completed_segments(&dir);
        remove_incomplete(&dir, &done);
        outcome.map_err(ProxyError::Engine)?;

        let proxy = Proxy {
            version: FORMAT_VERSION,
            height: PROXY_HEIGHT,
            segments: done,
        };
        let bytes = serde_json::to_vec(&proxy).map_err(|e| ProxyError::Io(e.into()))?;
        self.cache
            .write(&manifest, &bytes)
            .map_err(ProxyError::Io)?;
        Ok(proxy)
    }

    /// The finished proxy of `source`, if one was made. Never generates.
    ///
    /// # Errors
    ///
    /// The source cannot be read to find its proxy.
    pub fn find(&self, source: &Path) -> Result<Option<Proxy>, ProxyError> {
        Ok(read_manifest(&self.directory(source)?.join("proxy.json")))
    }

    /// Delete a proxy, finished or partial.
    ///
    /// # Errors
    ///
    /// The source cannot be read to find its proxy, or the proxy cannot be
    /// deleted.
    pub fn discard(&self, source: &Path) -> Result<(), ProxyError> {
        let dir = self.directory(source)?;
        match fs::remove_dir_all(&dir) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(ProxyError::Io(error))
            }
            _ => Ok(()),
        }
    }

    /// Where the proxy of `source` lives: keyed by content, like every cache.
    ///
    /// # Errors
    ///
    /// The source cannot be read.
    pub fn directory(&self, source: &Path) -> Result<PathBuf, ProxyError> {
        let key = ContentKey::of(source).map_err(ProxyError::Io)?;
        Ok(self
            .cache
            .path("proxies", &key, &format!("-{PROXY_HEIGHT}p")))
    }
}

fn read_manifest(path: &Path) -> Option<Proxy> {
    let proxy: Proxy = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (proxy.version == FORMAT_VERSION && proxy.segments.iter().all(|s| s.file.is_file()))
        .then_some(proxy)
}

/// Segments that FFmpeg finished — the ones named in a segment list, which it
/// writes only when a segment is closed — in order.
fn completed_segments(dir: &Path) -> Vec<ProxySegment> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut lists: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("list-"))
        })
        .collect();
    lists.sort();
    let mut segments: Vec<ProxySegment> = lists
        .iter()
        .filter_map(|list| fs::read_to_string(list).ok())
        .flat_map(|text| {
            text.lines()
                .filter_map(|line| {
                    let mut fields = line.split(',');
                    let file = fields.next()?.trim();
                    let start = fields.next()?.trim().parse().ok()?;
                    let end = fields.next()?.trim().parse().ok()?;
                    Some(ProxySegment {
                        file: dir.join(file),
                        start,
                        end,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|segment| segment.file.is_file())
        .collect();
    // In file order, which is time order: the segment muxer numbers them.
    // Its listed *start* is not trusted for the first segment of a resumed
    // run — it counts from zero there, not from where the input was sought
    // to — but the segments are contiguous by construction, so each one
    // starts where the one before it ended.
    segments.sort_by(|a, b| a.file.cmp(&b.file));
    let mut previous_end = 0.0;
    for segment in &mut segments {
        segment.start = previous_end;
        previous_end = segment.end;
    }
    segments
}

/// Delete every segment file that is not complete — the one a cancelled or
/// failed run was writing.
fn remove_incomplete(dir: &Path, complete: &[ProxySegment]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        let is_segment = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("seg_"));
        if is_segment && !complete.iter().any(|segment| segment.file == path) {
            let _ = fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_export_source_is_the_original_whatever_proxy_is_attached() {
        let mut asset = MediaAsset::new(PathBuf::from("C:/footage/4k.mp4"));
        asset.attach_proxy(Proxy {
            version: FORMAT_VERSION,
            height: PROXY_HEIGHT,
            segments: vec![ProxySegment {
                file: PathBuf::from("C:/cache/proxies/x/seg_00000.mkv"),
                start: 0.0,
                end: 10.0,
            }],
        });
        assert!(asset.preview_source().is_proxy());
        assert_eq!(asset.export_source().path(), Path::new("C:/footage/4k.mp4"));

        asset.detach_proxy();
        assert!(!asset.preview_source().is_proxy());
    }
}
