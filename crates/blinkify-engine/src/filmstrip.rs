//! Filmstrip thumbnails for the timeline, packed into sprite sheets.
//!
//! The timeline shows thumbnails along every video clip. Thousands of small
//! files are slow to write on Windows and miserable to evict, so thumbnails are
//! tiled into sprite sheets — one JPEG holding a grid of them — by FFmpeg's own
//! `tile` filter, in **one long-running process per filmstrip**. Never a
//! process per thumbnail: that is the classic reason FFmpeg-based editors feel
//! slow.
//!
//! Sheets are written to the content-keyed cache as each one completes, and
//! reported as they are, so the timeline shows the start of a clip while the
//! end is still being decoded.
//!
//! The interval between thumbnails comes from the timeline's zoom
//! ([`interval_for_zoom`]), snapped to a ladder of powers of two so that
//! nearby zoom levels share a filmstrip instead of each generating its own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::cache::{Cache, ContentKey};
use crate::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand,
};
use crate::probe::MediaInfo;

/// Thumbnails per row and rows per sheet.
pub const COLUMNS: u32 = 10;
pub const ROWS: u32 = 10;

/// Bump when the manifest or the sheet layout changes.
const FORMAT_VERSION: u32 = 1;

/// The shortest and longest interval the ladder offers, in seconds.
const MIN_INTERVAL: f64 = 0.25;
const MAX_INTERVAL: f64 = 256.0;

/// The seconds between thumbnails for a timeline zoom: one thumbnail per tile
/// width, rounded *down* to a power of two so thumbnails never leave gaps, and
/// clamped to the ladder.
#[must_use]
pub fn interval_for_zoom(pixels_per_second: f64, tile_width: u32) -> f64 {
    if !(pixels_per_second.is_finite() && pixels_per_second > 0.0) {
        return MAX_INTERVAL;
    }
    let exact = f64::from(tile_width.max(1)) / pixels_per_second;
    let snapped = 2_f64.powf(exact.log2().floor());
    snapped.clamp(MIN_INTERVAL, MAX_INTERVAL)
}

/// Where a filmstrip's sheets are, and how to find one thumbnail in them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Filmstrip {
    version: u32,
    /// Seconds between thumbnails; thumbnail `n` is the frame at `n × interval`.
    pub interval: f64,
    pub tile_width: u32,
    pub tile_height: u32,
    pub columns: u32,
    pub rows: u32,
    /// How many thumbnails there are. The last sheet may be partly empty.
    pub count: u32,
    /// Sheet files, in order, each `columns × rows` tiles.
    #[ts(type = "string[]")]
    pub sheets: Vec<PathBuf>,
}

impl Filmstrip {
    /// The sheet holding thumbnail `n`, and the tile's pixel origin in it.
    #[must_use]
    pub fn locate(&self, n: u32) -> Option<(&Path, u32, u32)> {
        if n >= self.count {
            return None;
        }
        let per_sheet = self.columns * self.rows;
        let sheet = self
            .sheets
            .get(usize::try_from(n.checked_div(per_sheet)?).ok()?)?;
        let within = n.checked_rem(per_sheet)?;
        let column = within.checked_rem(self.columns)?;
        let row = within.checked_div(self.columns)?;
        Some((sheet, column * self.tile_width, row * self.tile_height))
    }
}

/// A sheet written, the payload the timeline redraws on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SheetReady {
    pub index: u32,
    #[ts(type = "string")]
    pub path: PathBuf,
    /// Thumbnails available so far, from the start of the clip.
    pub thumbnails: u32,
}

#[derive(Debug, Error)]
pub enum FilmstripError {
    #[error("the file has no video stream")]
    NoVideo,
    #[error("could not generate the filmstrip: {0}")]
    Engine(#[source] JobError),
    #[error("could not write the filmstrip: {0}")]
    Io(#[source] std::io::Error),
}

/// Splits a stream of concatenated JPEGs (FFmpeg's `image2pipe`) into images.
///
/// A JPEG ends at its End-Of-Image marker `FF D9`. Inside entropy-coded data a
/// literal `FF` is always followed by a stuffed `00`, and restart markers are
/// `FF D0`–`FF D7`, so `FF D9` cannot occur before the real end.
#[derive(Debug, Default)]
struct JpegSplitter {
    pending: Vec<u8>,
}

impl JpegSplitter {
    fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.pending.extend_from_slice(bytes);
        let mut images = Vec::new();
        while let Some(end) = self
            .pending
            .windows(2)
            .position(|pair| pair == [0xFF, 0xD9])
        {
            let image: Vec<u8> = self.pending.drain(..end + 2).collect();
            if image.starts_with(&[0xFF, 0xD8]) {
                images.push(image);
            }
        }
        images
    }
}

/// Receives the sheet stream, writes each sheet as it completes, and reports
/// it.
#[derive(Debug)]
struct SheetWriter {
    cache: Cache,
    key: ContentKey,
    name: String,
    expected: u32,
    written: Vec<PathBuf>,
    failure: Option<std::io::Error>,
    splitter: JpegSplitter,
}

impl SheetWriter {
    fn chunk(&mut self, bytes: &[u8], on_sheet: &mut impl FnMut(SheetReady)) -> Flow {
        for image in self.splitter.push(bytes) {
            let index = u32::try_from(self.written.len()).unwrap_or(u32::MAX);
            let path = self.cache.path(
                "filmstrip",
                &self.key,
                &format!("{}-{index:04}.jpg", self.name),
            );
            if let Err(error) = self.cache.write(&path, &image) {
                self.failure = Some(error);
                return Flow::Fail("a sheet could not be written".to_owned());
            }
            self.written.push(path.clone());
            let thumbnails = (index + 1).saturating_mul(COLUMNS * ROWS);
            on_sheet(SheetReady {
                index,
                path,
                thumbnails: if self.expected > 0 {
                    thumbnails.min(self.expected)
                } else {
                    thumbnails
                },
            });
        }
        Flow::Continue
    }
}

/// Generates filmstrips into the cache.
#[derive(Debug, Clone)]
pub struct Filmstrips {
    orchestrator: Orchestrator,
    cache: Cache,
}

impl Filmstrips {
    #[must_use]
    pub fn new(orchestrator: Orchestrator, cache: Cache) -> Self {
        Self {
            orchestrator,
            cache,
        }
    }

    /// The filmstrip for `path` at `tile_height` pixels and `interval`
    /// seconds: from the cache, or generated now by one FFmpeg process at
    /// background priority, calling `on_sheet` as each sheet is written.
    /// Blocks: run it off the UI thread. Cancelling through `cancel` stops the
    /// process; sheets already written stay, as complete files.
    ///
    /// # Errors
    ///
    /// No video stream, or the process failed or was cancelled.
    pub fn filmstrip(
        &self,
        path: &Path,
        info: &MediaInfo,
        tile_height: u32,
        interval: f64,
        cancel: Option<&CancelToken>,
        on_sheet: impl FnMut(SheetReady) + Send + 'static,
    ) -> Result<Filmstrip, FilmstripError> {
        let (stream, video) = info
            .video()
            .find(|(_, video)| !video.is_attached_picture)
            .ok_or(FilmstripError::NoVideo)?;
        let tile_height = tile_height.max(2) & !1;
        let tile_width = tile_width(video.display_width, video.display_height, tile_height);
        let key = ContentKey::of(path).map_err(FilmstripError::Io)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let interval_ms = (interval * 1000.0).round().max(1.0) as u64;
        let name = format!("-h{tile_height}-i{interval_ms}");
        let manifest_path = self.cache.path("filmstrip", &key, &format!("{name}.json"));

        if let Some(filmstrip) = self
            .cache
            .read(&manifest_path)
            .and_then(|bytes| serde_json::from_slice::<Filmstrip>(&bytes).ok())
            .filter(|f| f.version == FORMAT_VERSION && f.sheets.iter().all(|s| s.exists()))
        {
            return Ok(filmstrip);
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = info
            .container
            .duration_seconds
            .map_or(0, |d| (d / interval).ceil().max(1.0) as u32);

        let writer = Arc::new(Mutex::new(SheetWriter {
            cache: self.cache.clone(),
            key,
            name,
            expected,
            written: Vec::new(),
            failure: None,
            splitter: JpegSplitter::default(),
        }));
        let sink = Arc::clone(&writer);
        let mut on_sheet = on_sheet;
        let job = self.orchestrator.run(
            SidecarCommand::ffmpeg()
                .option("-loglevel", "error")
                .input(path)
                .option("-map", format!("0:{}", stream.index))
                .flags(&["-an", "-sn", "-dn"])
                .option(
                    "-vf",
                    format!(
                        "fps=1/{interval},scale={tile_width}:{tile_height}:flags=bicubic,tile={COLUMNS}x{ROWS}"
                    ),
                )
                .option("-c:v", "mjpeg")
                .option("-q:v", "4")
                .option("-f", "image2pipe")
                .output_stdout(),
            Priority::Background,
            JobOptions::default()
                .cancel_token(cancel.cloned().unwrap_or_default())
                .on_chunk(move |chunk| {
                    sink.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .chunk(chunk, &mut on_sheet)
                }),
        );
        let outcome = job.wait();
        let mut writer = writer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(error) = writer.failure.take() {
            return Err(FilmstripError::Io(error));
        }
        outcome.map_err(FilmstripError::Engine)?;

        let sheets = std::mem::take(&mut writer.written);
        let filled = u32::try_from(sheets.len())
            .unwrap_or(0)
            .saturating_mul(COLUMNS * ROWS);
        let filmstrip = Filmstrip {
            version: FORMAT_VERSION,
            interval,
            tile_width,
            tile_height,
            columns: COLUMNS,
            rows: ROWS,
            count: if expected > 0 {
                expected.min(filled)
            } else {
                filled
            },
            sheets,
        };
        if let Ok(bytes) = serde_json::to_vec(&filmstrip) {
            self.cache
                .write(&manifest_path, &bytes)
                .map_err(FilmstripError::Io)?;
        }
        Ok(filmstrip)
    }
}

/// The tile width for a display size at `height`, rounded to an even number
/// as the scaler and the JPEG encoder want.
#[must_use]
pub fn tile_width(display_width: u32, display_height: u32, height: u32) -> u32 {
    if display_height == 0 {
        return height;
    }
    let width = f64::from(height) * f64::from(display_width) / f64::from(display_height);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let width = (width / 2.0).round() as u32 * 2;
    width.max(2)
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_interval_follows_the_zoom_on_a_power_of_two_ladder() {
        // 128-pixel tiles at 100 px/s want 1.28 s; the ladder says 1 s.
        assert_eq!(interval_for_zoom(100.0, 128), 1.0);
        assert_eq!(interval_for_zoom(10.0, 128), 8.0);
        assert_eq!(interval_for_zoom(10_000.0, 128), MIN_INTERVAL);
        assert_eq!(interval_for_zoom(0.001, 128), MAX_INTERVAL);
        assert_eq!(interval_for_zoom(0.0, 128), MAX_INTERVAL);
    }

    #[test]
    fn a_thumbnail_is_found_on_its_sheet() {
        let strip = Filmstrip {
            version: FORMAT_VERSION,
            interval: 1.0,
            tile_width: 128,
            tile_height: 72,
            columns: 10,
            rows: 10,
            count: 150,
            sheets: vec![PathBuf::from("a.jpg"), PathBuf::from("b.jpg")],
        };
        assert_eq!(strip.locate(0), Some((Path::new("a.jpg"), 0, 0)));
        assert_eq!(strip.locate(13), Some((Path::new("a.jpg"), 3 * 128, 72)));
        assert_eq!(
            strip.locate(149),
            Some((Path::new("b.jpg"), 9 * 128, 4 * 72))
        );
        assert_eq!(strip.locate(150), None);
    }

    #[test]
    fn jpegs_are_split_across_arbitrary_chunk_boundaries() {
        let one = [0xFF, 0xD8, 0x01, 0xFF, 0x00, 0x02, 0xFF, 0xD9];
        let two = [0xFF, 0xD8, 0x03, 0xFF, 0xD9];
        let stream: Vec<u8> = one.iter().chain(two.iter()).copied().collect();
        let mut splitter = JpegSplitter::default();
        let mut images = Vec::new();
        for byte in &stream {
            images.extend(splitter.push(&[*byte]));
        }
        assert_eq!(images, vec![one.to_vec(), two.to_vec()]);
    }

    #[test]
    fn a_portrait_display_makes_a_narrow_tile() {
        assert_eq!(tile_width(1920, 1080, 72), 128);
        assert_eq!(tile_width(720, 1280, 72), 40);
        assert_eq!(tile_width(0, 0, 72), 72);
    }
}
