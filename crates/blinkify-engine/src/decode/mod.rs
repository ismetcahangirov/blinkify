//! Decoding raw frames for the preview.
//!
//! An HTML `<video>` element pointed at the source cannot seek to an exact
//! frame and cannot show the edit graph, so the preview is built from decoded
//! frames (#27). How those frames are produced decides whether it is usable:
//!
//! - **One FFmpeg process per source**, decoding forward into a pipe as raw
//!   RGBA — the pixel format conversion and the scale to preview size happen
//!   in FFmpeg, never in JavaScript. A process lives as long as playback runs
//!   forward; it is replaced only when the position jumps somewhere it cannot
//!   decode forward to. (The FFmpeg command line cannot seek a running
//!   process, so a jump is a new process; there is never more than one per
//!   source.)
//! - **Every frame carries its real timestamp**, in the stream's time base,
//!   read from `showinfo` — never inferred from a frame count, which a
//!   variable frame rate makes wrong. See [`showinfo`].
//! - **Backpressure is mandatory.** Frames go into a bounded [`FrameRing`];
//!   when it is full the reader blocks, the pipe fills, and FFmpeg stops.
//! - **Rotation is not applied here.** FFmpeg's autorotation is switched off
//!   and the frame is delivered as coded; the renderer rotates it at draw
//!   time from the probe's display matrix, which costs nothing on the GPU and
//!   keeps a portrait phone video upright without a transpose per frame.
//! - **Frame-exact start.** Decoding starts at a keyframe (the seek point) and
//!   frames before the requested first frame are discarded inside FFmpeg, by
//!   timestamp — before they are scaled, converted or piped.

mod ring;
mod showinfo;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

pub use ring::{FrameRing, RingStats};

use crate::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, Orchestrator, Priority, SidecarCommand,
};
use crate::probe::{Rational, VideoInfo};
use crate::time::{Rounding, rescale};

/// Bytes per pixel of the RGBA frames the decoder delivers.
pub const BYTES_PER_PIXEL: usize = 4;

/// How long a complete frame on standard output may wait for its timestamp on
/// standard error before the decode is declared broken.
const TIMESTAMP_WAIT: Duration = Duration::from_secs(5);

/// One decoded frame, as coded — unrotated — in RGBA.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// Presentation timestamp, in the stream's time base.
    pub pts: i64,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, rows top to bottom, no padding.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFrame")
            .field("pts", &self.pts)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

/// The size, in pixels, of the frames a decode delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    #[must_use]
    pub fn bytes(self) -> usize {
        usize::try_from(self.width)
            .unwrap_or(0)
            .saturating_mul(usize::try_from(self.height).unwrap_or(0))
            .saturating_mul(BYTES_PER_PIXEL)
    }

    /// The largest frame size for `video` that fits `max_width` by
    /// `max_height` once it is rotated for display, never larger than the
    /// source. A zero bound means "no bound".
    ///
    /// The size is in the coded, unrotated orientation, and in square pixels:
    /// an anamorphic source is widened by its sample aspect ratio here, so
    /// the renderer only ever has to rotate.
    #[must_use]
    pub fn fit(video: &VideoInfo, max_width: u32, max_height: u32) -> Self {
        let sar = video
            .sample_aspect_ratio
            .and_then(Rational::value)
            .filter(|sar| sar.is_finite() && *sar > 0.0)
            .unwrap_or(1.0);
        let square_width = f64::from(video.width.max(1)) * sar;
        let square_height = f64::from(video.height.max(1));
        // The bound is on the displayed picture; the frame is unrotated.
        let (bound_width, bound_height) = if video.rotation % 180 == 90 {
            (max_height, max_width)
        } else {
            (max_width, max_height)
        };
        let limit = |bound: u32, length: f64| {
            if bound == 0 {
                f64::INFINITY
            } else {
                f64::from(bound) / length
            }
        };
        let scale = limit(bound_width, square_width)
            .min(limit(bound_height, square_height))
            .min(1.0);
        let pixels = |length: f64| {
            let rounded = (length * scale).round().max(2.0);
            // Bounded above by the source dimension, which is a `u32`.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pixels = rounded as u32;
            pixels
        };
        Self {
            width: pixels(square_width),
            height: pixels(square_height),
        }
    }
}

/// Why a decode could not run.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("{} has no video to preview", .0.display())]
    NoVideo(PathBuf),
    #[error("{} is no longer available", .0.display())]
    SourceUnavailable(PathBuf),
    #[error("the preview decoder failed: {0}")]
    Failed(String),
}

/// What to decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeRequest {
    pub source: PathBuf,
    /// The stream's absolute index in the file.
    pub stream: u32,
    /// The stream's time base, which every timestamp here is in.
    pub time_base: Rational,
    /// The keyframe to start decoding at, or `None` for the start of the
    /// stream.
    pub seek_to: Option<i64>,
    /// The first frame to deliver. Frames before it are decoded — they are
    /// needed as references — and discarded inside FFmpeg.
    pub first_pts: i64,
    pub size: FrameSize,
    /// Stop after this many frames, or run to the end of the stream.
    pub max_frames: Option<u32>,
}

impl DecodeRequest {
    /// The FFmpeg invocation.
    ///
    /// `-copyts` keeps the stream's own timestamps, so the ones `showinfo`
    /// reports are the ones the keyframe index and the edit graph use.
    /// `-seek_timestamp 1` makes `-ss` an absolute timestamp rather than an
    /// offset from the file's start time, and `-noaccurate_seek` leaves the
    /// exact selection to the `select` filter, which compares integer ticks
    /// rather than a rounded number of seconds.
    #[must_use]
    pub fn command(&self) -> SidecarCommand {
        let mut command = SidecarCommand::ffmpeg()
            .option("-v", "info")
            .flag("-nostats")
            .flag("-copyts")
            .flag("-noautorotate");
        if let Some(seek) = self.seek_to {
            command = command
                .option("-seek_timestamp", "1")
                .flag("-noaccurate_seek")
                .option("-ss", seconds_at_or_after(seek, self.time_base));
        }
        let graph = format!(
            "select=gte(pts\\,{first}),scale={width}:{height}:flags=bilinear,format=rgba,showinfo=checksum=0",
            first = self.first_pts,
            width = self.size.width,
            height = self.size.height,
        );
        command = command
            .input(&self.source)
            .option("-map", format!("0:{}", self.stream))
            .flags(&["-an", "-sn", "-dn"])
            .option("-vf", graph)
            .option("-fps_mode", "passthrough");
        if let Some(frames) = self.max_frames {
            command = command.option("-frames:v", frames.to_string());
        }
        command
            .option("-pix_fmt", "rgba")
            .option("-f", "rawvideo")
            .output_stdout()
    }
}

/// `ticks` in `time_base` as decimal seconds, rounded **up** to the
/// microsecond FFmpeg parses `-ss` into. Rounding down could land a hair
/// before the keyframe, and the demuxer would then seek to the one before it.
fn seconds_at_or_after(ticks: i64, time_base: Rational) -> String {
    let numerator = i128::from(ticks) * i128::from(time_base.num) * 1_000_000;
    let denominator = i128::from(time_base.den).max(1);
    let floor = numerator.div_euclid(denominator);
    let micros = if numerator.rem_euclid(denominator) == 0 {
        floor
    } else {
        floor + 1
    };
    let sign = if micros < 0 { "-" } else { "" };
    let micros = micros.abs();
    format!(
        "{sign}{}.{:06}",
        micros.div_euclid(1_000_000),
        micros.rem_euclid(1_000_000)
    )
}

/// Timestamps read from `showinfo`, waiting for the frames they belong to.
#[derive(Debug, Default)]
struct Timestamps {
    queue: Mutex<TimestampQueue>,
    arrived: Condvar,
}

#[derive(Debug, Default)]
struct TimestampQueue {
    pts: VecDeque<Option<i64>>,
    link_time_base: Option<Rational>,
    ended: bool,
}

impl Timestamps {
    fn line(&self, line: &str, stream_time_base: Rational) {
        let Some(parsed) = showinfo::parse(line) else {
            return;
        };
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        match parsed {
            showinfo::Line::TimeBase(time_base) => queue.link_time_base = Some(time_base),
            showinfo::Line::Frame { pts, .. } => {
                let from = queue.link_time_base.unwrap_or(stream_time_base);
                let pts =
                    pts.and_then(|pts| rescale(pts, from, stream_time_base, Rounding::Nearest));
                queue.pts.push_back(pts);
            }
        }
        drop(queue);
        self.arrived.notify_all();
    }

    fn end(&self) {
        self.queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .ended = true;
        self.arrived.notify_all();
    }

    /// The next frame's timestamp. `Err` with the reason if it cannot come.
    fn next(&self, cancel: &CancelToken) -> Result<i64, String> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        let mut waited = Duration::ZERO;
        let step = Duration::from_millis(50);
        loop {
            if let Some(pts) = queue.pts.pop_front() {
                return pts.ok_or_else(|| "a decoded frame has no timestamp".to_owned());
            }
            if queue.ended || cancel.is_cancelled() {
                return Err("the decoder stopped reporting timestamps".to_owned());
            }
            if waited >= TIMESTAMP_WAIT {
                return Err("a decoded frame arrived without its timestamp".to_owned());
            }
            queue = self
                .arrived
                .wait_timeout(queue, step)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
            waited += step;
        }
    }
}

/// Splits standard output into frames and hands each to the ring.
struct FrameAssembler {
    size: FrameSize,
    frame_bytes: usize,
    partial: Vec<u8>,
    timestamps: Arc<Timestamps>,
    ring: Arc<FrameRing>,
    head: Arc<AtomicI64>,
    delivered: u32,
    max_frames: Option<u32>,
    cancel: CancelToken,
}

impl FrameAssembler {
    fn chunk(&mut self, mut rest: &[u8]) -> Flow {
        while !rest.is_empty() {
            let need = self.frame_bytes - self.partial.len();
            let Some((head, tail)) = rest.split_at_checked(need.min(rest.len())) else {
                return Flow::Fail("frame assembly lost its place".to_owned());
            };
            self.partial.extend_from_slice(head);
            rest = tail;
            if self.partial.len() < self.frame_bytes {
                continue;
            }
            let pixels = std::mem::replace(&mut self.partial, Vec::with_capacity(self.frame_bytes));
            let pts = match self.timestamps.next(&self.cancel) {
                Ok(pts) => pts,
                Err(reason) => return Flow::Fail(reason),
            };
            let frame = VideoFrame {
                pts,
                width: self.size.width,
                height: self.size.height,
                pixels,
            };
            // Blocks while the ring is full: this is the backpressure.
            if !self.ring.push(frame, &self.cancel) {
                return Flow::Stop;
            }
            self.head.store(pts, Ordering::Release);
            self.delivered = self.delivered.saturating_add(1);
            if self.max_frames.is_some_and(|max| self.delivered >= max) {
                return Flow::Stop;
            }
        }
        Flow::Continue
    }
}

/// How a decode ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeEnd {
    /// Every requested frame was delivered.
    Finished,
    /// It was stopped before it finished.
    Stopped,
    Failed(DecodeError),
}

/// One running decode: an FFmpeg process filling a [`FrameRing`].
#[derive(Debug)]
pub struct VideoDecoder {
    cancel: CancelToken,
    ring: Arc<FrameRing>,
    pid: Arc<OnceLock<u32>>,
    head: Arc<AtomicI64>,
    frames: Arc<AtomicU64>,
    end: Arc<Mutex<Option<DecodeEnd>>>,
    watcher: Option<JoinHandle<()>>,
}

impl VideoDecoder {
    /// Start decoding `request` into `ring`. Returns at once.
    #[must_use]
    pub fn start(
        orchestrator: &Orchestrator,
        request: &DecodeRequest,
        ring: Arc<FrameRing>,
    ) -> Self {
        let cancel = CancelToken::default();
        let timestamps = Arc::new(Timestamps::default());
        let head = Arc::new(AtomicI64::new(i64::MIN));
        let frames = Arc::new(AtomicU64::new(0));
        let mut assembler = FrameAssembler {
            size: request.size,
            frame_bytes: request.size.bytes().max(1),
            partial: Vec::with_capacity(request.size.bytes()),
            timestamps: Arc::clone(&timestamps),
            ring: Arc::clone(&ring),
            head: Arc::clone(&head),
            delivered: 0,
            max_frames: request.max_frames,
            cancel: cancel.clone(),
        };
        let counted = Arc::clone(&frames);
        let line_timestamps = Arc::clone(&timestamps);
        let time_base = request.time_base;
        let options = JobOptions::default()
            .cancel_token(cancel.clone())
            .on_stderr_line(move |line| line_timestamps.line(line, time_base))
            .on_chunk(move |chunk| {
                let before = assembler.delivered;
                let flow = assembler.chunk(chunk);
                counted.fetch_add(u64::from(assembler.delivered - before), Ordering::Relaxed);
                flow
            });
        let waker = Arc::clone(&ring);
        let job = orchestrator.run(request.command(), Priority::Playback, options);
        let pid = job.pid_handle();
        let end = Arc::new(Mutex::new(None));
        let watcher = {
            let end = Arc::clone(&end);
            let source = request.source.clone();
            thread::Builder::new()
                .name("preview-decode".to_owned())
                .spawn(move || {
                    let outcome = job.wait();
                    timestamps.end();
                    ring.finish();
                    let finished = match outcome {
                        Ok(_) => DecodeEnd::Finished,
                        Err(JobError::Cancelled | JobError::ShuttingDown) => DecodeEnd::Stopped,
                        Err(_) if !source.is_file() => {
                            DecodeEnd::Failed(DecodeError::SourceUnavailable(source))
                        }
                        Err(error) => DecodeEnd::Failed(DecodeError::Failed(error.to_string())),
                    };
                    *end.lock().unwrap_or_else(PoisonError::into_inner) = Some(finished);
                })
                .ok()
        };
        Self {
            cancel,
            ring: waker,
            pid,
            head,
            frames,
            end,
            watcher,
        }
    }

    /// The decoder process's id, once it has started.
    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        self.pid.get().copied()
    }

    /// The timestamp of the newest frame delivered, if any.
    #[must_use]
    pub fn head(&self) -> Option<i64> {
        let head = self.head.load(Ordering::Acquire);
        (head != i64::MIN).then_some(head)
    }

    /// Frames delivered so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// How the decode ended, or `None` while it runs.
    #[must_use]
    pub fn end(&self) -> Option<DecodeEnd> {
        self.end
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Stop the decode and wait until its process has exited.
    pub fn stop(mut self) {
        self.stop_in_place();
    }

    fn stop_in_place(&mut self) {
        self.cancel.cancel();
        // A producer blocked on a full ring must see the cancel now, not at
        // its next re-check.
        self.ring.wake_producers();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.stop_in_place();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Color, FrameRate, FrameRateMode};

    fn video(width: u32, height: u32, rotation: u32, sar: Option<Rational>) -> VideoInfo {
        let (display_width, display_height) = if rotation % 180 == 90 {
            (height, width)
        } else {
            (width, height)
        };
        VideoInfo {
            width,
            height,
            display_width,
            display_height,
            rotation,
            pixel_format: None,
            bit_depth: None,
            chroma_subsampling: None,
            sample_aspect_ratio: sar,
            frame_rate: FrameRate {
                average: None,
                real_base: None,
                mode: FrameRateMode::Unknown,
            },
            field_order: None,
            has_b_frames: false,
            color: Color {
                range: None,
                primaries: None,
                transfer: None,
                matrix: None,
            },
            hdr: None,
            is_attached_picture: false,
        }
    }

    #[test]
    fn a_frame_fits_the_bound_and_never_grows() {
        let hd = video(1920, 1080, 0, None);
        assert_eq!(
            FrameSize::fit(&hd, 960, 960),
            FrameSize {
                width: 960,
                height: 540
            }
        );
        assert_eq!(
            FrameSize::fit(&hd, 4000, 4000),
            FrameSize {
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            FrameSize::fit(&hd, 0, 0),
            FrameSize {
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn a_portrait_frame_is_fitted_as_it_will_be_displayed() {
        // Coded landscape, displayed portrait: a 540-wide, 960-tall slot
        // fits the whole picture at half size.
        let phone = video(1920, 1080, 90, None);
        assert_eq!(
            FrameSize::fit(&phone, 540, 960),
            FrameSize {
                width: 960,
                height: 540
            }
        );
    }

    #[test]
    fn an_anamorphic_frame_is_delivered_in_square_pixels() {
        let dv = video(720, 576, 0, Some(Rational { num: 16, den: 15 }));
        assert_eq!(
            FrameSize::fit(&dv, 0, 0),
            FrameSize {
                width: 768,
                height: 576
            }
        );
    }

    #[test]
    fn seek_seconds_round_up_to_the_microsecond() {
        let ntsc = Rational { num: 1, den: 30000 };
        // 1001 ticks is 33366.66… µs; rounding down would land before it.
        assert_eq!(seconds_at_or_after(1001, ntsc), "0.033367");
        assert_eq!(
            seconds_at_or_after(10752, Rational { num: 1, den: 15360 }),
            "0.700000"
        );
        assert_eq!(
            seconds_at_or_after(-4608, Rational { num: 1, den: 15360 }),
            "-0.300000"
        );
    }

    #[test]
    fn the_command_starts_at_the_keyframe_and_selects_by_tick() {
        let request = DecodeRequest {
            source: PathBuf::from("clip.mp4"),
            stream: 0,
            time_base: Rational { num: 1, den: 15360 },
            seek_to: Some(10752),
            first_pts: 11264,
            size: FrameSize {
                width: 64,
                height: 36,
            },
            max_frames: Some(1),
        };
        let command = request.command().to_string();
        assert!(command.contains("-ss 0.700000"), "{command}");
        assert!(command.contains("select=gte(pts\\,11264)"), "{command}");
        assert!(command.contains("-noautorotate"), "{command}");
        assert!(command.contains("-frames:v 1"), "{command}");
        assert!(command.ends_with("pipe:1"), "{command}");
    }
}
