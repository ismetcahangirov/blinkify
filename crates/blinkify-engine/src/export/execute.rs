//! The export executor: the plan, carried out in one pass to the output file.
//!
//! ADR-0010. Every segment's packets come from a sidecar process that writes
//! NUT to a pipe — for a copied segment, a reader that stream-copies the
//! source. The engine selects the packets the segment needs, rebases their
//! timestamps onto the output timeline, interleaves the streams, and writes
//! them as NUT into the one muxer process that writes the file. Nothing is
//! written to disk but the output, and the output is written beside the
//! target under a temporary name and renamed only when it is complete.
//!
//! What the executor holds to:
//!
//! - **It does what the plan says, and fails loudly where it cannot.** A
//!   segment the plan copies is copied or the export fails; it is never
//!   quietly re-encoded. A file that disagrees with the plan — no keyframe
//!   where the index said one was — is an error.
//! - **The source is read, never written.** It is opened by the readers only.
//! - **The target is never overwritten** unless the caller says the user
//!   confirmed it (`CLAUDE.md` section 19), and a failed or cancelled export
//!   leaves nothing at the target and no partial file beside it.
//! - **Cancellation is real**: one token stops every process of the export.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use thiserror::Error;

use super::audio::{self, AudioEncoding, AudioTarget};
use super::nut::{self, Header, NutError, Packet, StreamHeader};
use super::plan::{ExportPlan, Media, Segment, SegmentSource};
use super::profile::{EncoderChoice, JoinMismatch, validate_join};
use super::render::{self, RenderJob};
use super::seam::{self, Piece, Stitch};
use crate::audio::chain::ChainError;
use crate::audio::denoise::Models;
use crate::capability::VideoCodec;
use crate::orchestrator::{
    CancelToken, Flow, JobError, JobOptions, JobProgress, Orchestrator, Priority, SidecarCommand,
};
use crate::probe::{MediaInfo, Rational, StreamKind};
use crate::project::SourceId;
use crate::proxy::ExportSource;
use crate::tier::ExportTier;
use crate::time::{Rounding, rescale};

/// Seconds added to every timestamp a reader writes. NUT cannot carry a
/// negative timestamp, and a source's first decode timestamps are negative
/// wherever it has B-frames or an edit list's pre-roll; FFmpeg would shift
/// them by an amount the engine cannot see. A fixed, known offset keeps
/// every timestamp positive and exact.
const READER_OFFSET_SECONDS: i64 = 100;

/// How far before a segment's in-point a reader seeks. The demuxer seeks to
/// a keyframe at or before the time asked for; the margin keeps a rounding
/// of that time from landing just past the in-point's own keyframe.
const SEEK_MARGIN_SECONDS: f64 = 3.0;

/// Packets buffered between a stream's router and the interleaver.
const PACKET_QUEUE: usize = 64;

/// Chunks of NUT buffered on their way into the muxer.
const MUXER_QUEUE: usize = 32;

/// The size of those chunks.
const MUXER_CHUNK: usize = 256 * 1024;

/// A source as the export reads it: the original file and what it contains.
#[derive(Debug, Clone)]
pub struct ExportInput {
    pub source: ExportSource,
    pub info: Arc<MediaInfo>,
}

/// The container the output is written in, from the target's extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Mov,
    Matroska,
    WebM,
}

impl Container {
    /// The container `path`'s extension names.
    ///
    /// # Errors
    ///
    /// [`ExportError::Container`] for any other extension.
    pub fn of(path: &Path) -> Result<Self, ExportError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "mp4" | "m4v" => Ok(Self::Mp4),
            "mov" => Ok(Self::Mov),
            "mkv" => Ok(Self::Matroska),
            "webm" => Ok(Self::WebM),
            _ => Err(ExportError::Container(extension)),
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Matroska => "matroska",
            Self::WebM => "webm",
        }
    }

    /// Whether the container can hold packets of `codec` unchanged.
    #[must_use]
    pub fn accepts(self, codec: &str) -> bool {
        match self {
            Self::Mp4 | Self::Mov => {
                matches!(
                    codec,
                    "h264"
                        | "hevc"
                        | "av1"
                        | "vp9"
                        | "mpeg4"
                        | "aac"
                        | "mp3"
                        | "opus"
                        | "flac"
                        | "alac"
                        | "ac3"
                        | "eac3"
                ) || (self == Self::Mov && codec.starts_with("pcm_"))
            }
            Self::Matroska => true,
            Self::WebM => matches!(codec, "vp8" | "vp9" | "av1" | "opus" | "vorbis"),
        }
    }
}

/// Why an export did not produce its file.
#[derive(Debug, Error)]
pub enum ExportError {
    #[error("\".{0}\" is not a container Blinkify writes: choose .mp4, .mov, .mkv or .webm")]
    Container(String),
    #[error("{codec} cannot be written into a {container} file unchanged")]
    CodecNotInContainer { codec: String, container: String },
    #[error("{} already exists, and replacing it was not confirmed", .0.display())]
    TargetExists(PathBuf),
    #[error("{} is one of the export's sources, and a source is never written to", .0.display())]
    TargetIsSource(PathBuf),
    #[error("the plan declines part of this export: {0}")]
    Declined(String),
    #[error("this export needs a step Blinkify cannot run yet: {0}")]
    Unsupported(String),
    #[error("source {0} is not available to the export")]
    MissingSource(SourceId),
    #[error("the file does not match the plan: {0}")]
    Mismatch(String),
    #[error("the re-encoded pictures cannot join the copied ones: {0}")]
    Seam(#[from] JoinMismatch),
    #[error(transparent)]
    Engine(#[from] JobError),
    #[error(transparent)]
    Nut(#[from] NutError),
    #[error("the output could not be written: {0}")]
    Io(#[from] io::Error),
    #[error("the export was cancelled")]
    Cancelled,
    /// A clip's audio chain cannot be built: a model it needs is missing.
    #[error("the sound cannot be processed: {0}")]
    AudioChain(#[from] ChainError),
}

/// What an export wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    pub path: PathBuf,
    /// Packets written, per output stream: video first where there is one.
    pub packets: Vec<u64>,
    /// How the re-encoded sound was made, where any was: for the report.
    pub audio: Option<AudioEncoding>,
    /// Every sidecar command the export ran, as a person reads it: what
    /// the report can show, and what a test inspects to prove the pictures
    /// were never decoded.
    pub commands: Vec<String>,
}

/// Everything an export needs.
pub struct ExportRequest<'a> {
    pub plan: &'a ExportPlan,
    pub inputs: &'a BTreeMap<SourceId, ExportInput>,
    pub target: &'a Path,
    /// The user confirmed replacing an existing target.
    pub overwrite: bool,
    /// What re-encoded sound becomes where none of the output's sound is
    /// copied (#43).
    pub audio: AudioTarget,
    /// The bundled models noise reduction runs with (#47), if they were
    /// found. An export that needs one without it is refused before it
    /// starts.
    pub models: Option<&'a Models>,
    pub cancel: CancelToken,
    pub on_progress: Option<Box<dyn FnMut(JobProgress) + Send>>,
}

impl std::fmt::Debug for ExportRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportRequest")
            .field("target", &self.target)
            .field("overwrite", &self.overwrite)
            .finish_non_exhaustive()
    }
}

/// The file the output is written to until it is complete: beside the
/// target, so the rename is on one volume and cannot half-happen.
#[must_use]
pub fn partial_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!("{name}.blinkify-partial"))
}

/// A NUT stream arriving from a process, as a [`Read`].
struct ChunkReader {
    chunks: Receiver<Vec<u8>>,
    current: Vec<u8>,
    at: usize,
}

impl Read for ChunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.at >= self.current.len() {
            match self.chunks.recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.at = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let available = self.current.get(self.at..).unwrap_or_default();
        let count = available.len().min(buf.len());
        if let (Some(to), Some(from)) = (buf.get_mut(..count), available.get(..count)) {
            to.copy_from_slice(from);
        }
        self.at += count;
        Ok(count)
    }
}

/// The muxer's standard input, as a [`Write`]: buffered into large chunks.
struct ChunkWriter {
    chunks: SyncSender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl Write for ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        if self.buffer.len() >= MUXER_CHUNK {
            self.flush()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.buffer);
        self.chunks
            .send(chunk)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the muxer has exited"))
    }
}

/// A process writing NUT, being read.
struct Opened {
    reader: nut::Reader<ChunkReader>,
    job: crate::orchestrator::Job,
    stop: Arc<AtomicBool>,
}

impl Opened {
    /// Start `command`. It gets a cancel switch of its own: a reader stopped
    /// because its segment is complete is cancelled, and that must not
    /// cancel the export. The export's switch stops it through the router,
    /// which checks it between packets and drops the reader.
    fn start(orchestrator: &Orchestrator, command: SidecarCommand) -> Result<Self, ExportError> {
        let (sender, chunks) = mpsc::sync_channel::<Vec<u8>>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let job = orchestrator.run(
            command,
            Priority::Export,
            JobOptions::default().on_chunk(move |chunk| {
                if stopped.load(Ordering::SeqCst) || sender.send(chunk.to_vec()).is_err() {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            }),
        );
        let reader = nut::Reader::open(ChunkReader {
            chunks,
            current: Vec::new(),
            at: 0,
        });
        match reader {
            Ok(reader) => Ok(Self { reader, job, stop }),
            Err(error) => {
                // The process may have failed before writing a header: its
                // error says why, and is the one to report.
                stop.store(true, Ordering::SeqCst);
                job.wait()?;
                Err(error.into())
            }
        }
    }

    /// Start `command` with its standard input fed from `input`.
    fn start_fed(
        orchestrator: &Orchestrator,
        command: SidecarCommand,
        input: Receiver<Vec<u8>>,
    ) -> Result<Self, ExportError> {
        let (sender, chunks) = mpsc::sync_channel::<Vec<u8>>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let job = orchestrator.run(
            command,
            Priority::Export,
            JobOptions::default().stdin(input).on_chunk(move |chunk| {
                if stopped.load(Ordering::SeqCst) || sender.send(chunk.to_vec()).is_err() {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            }),
        );
        match nut::Reader::open(ChunkReader {
            chunks,
            current: Vec::new(),
            at: 0,
        }) {
            Ok(reader) => Ok(Self { reader, job, stop }),
            Err(error) => {
                stop.store(true, Ordering::SeqCst);
                job.wait()?;
                Err(error.into())
            }
        }
    }

    /// Stop the process, having read what was needed, and wait for it.
    fn finish(self) -> Result<(), ExportError> {
        self.stop.store(true, Ordering::SeqCst);
        drop(self.reader);
        match self.job.wait() {
            Ok(_) => Ok(()),
            Err(JobError::Cancelled) => Err(ExportError::Cancelled),
            Err(error) => Err(error.into()),
        }
    }
}

/// Where a segment's packets go: its source's in-point, the time base the
/// producer writes in, and the output's.
#[derive(Debug, Clone)]
struct Place {
    source: SourceId,
    /// The source stream's time base: the unit of the plan's ticks.
    source_base: Rational,
    /// The producer's NUT time base.
    nut_base: Rational,
    /// The reader offset, in `nut_base` ticks.
    offset: i64,
    /// The segment's in-point, in source ticks.
    origin: i64,
    speed: Rational,
    /// The segment's start on the output timeline, in `out_base` ticks.
    start: i64,
    out_base: Rational,
}

impl Place {
    fn of(
        plan: &ExportPlan,
        segment: &Segment,
        source: &SegmentSource,
        stream: &StreamHeader,
        out_base: Rational,
    ) -> Result<Self, ExportError> {
        let nut_base = stream.time_base;
        Ok(Self {
            source: source.source,
            source_base: source.time_base,
            nut_base,
            offset: convert(READER_OFFSET_SECONDS, Rational { num: 1, den: 1 }, nut_base)?,
            origin: source.source_in,
            speed: source.speed,
            start: convert(segment.start, plan.time_base, out_base)?,
            out_base,
        })
    }

    /// Source ticks as the producer writes them.
    fn nut(&self, ticks: i64) -> Result<i64, ExportError> {
        let offset = convert(
            READER_OFFSET_SECONDS,
            Rational { num: 1, den: 1 },
            self.nut_base,
        )?;
        Ok(convert(ticks, self.source_base, self.nut_base)? + offset)
    }

    /// A producer's timestamp back in source ticks.
    fn source_ticks(&self, pts: i64) -> Result<i64, ExportError> {
        convert(pts - self.offset, self.nut_base, self.source_base)
    }

    /// Where a producer's packet at `pts` (in `base`) goes on the output.
    fn output(&self, pts: i64, base: Rational) -> Result<i64, ExportError> {
        let origin = self.nut(self.origin)?;
        Ok(self.start + retime(pts - origin, base, self.out_base, self.speed)?)
    }
}

/// Which of a producer's packets are its segment's.
enum Window<'s> {
    /// A copy: the source's packets in the source's range.
    Copy(&'s SegmentSource),
    /// A smart-cut: copied pieces from the reader, recoded pieces from seam
    /// encoders.
    SmartCut(&'s SegmentSource),
    /// An encode: after `preroll` samples of priming, `samples` of them;
    /// every timestamp offset by `offset` samples.
    Encoded {
        preroll: i64,
        samples: i64,
        offset: i64,
    },
}

/// Metadata as NUT carries it: key and value.
type Metadata = Vec<(String, String)>;

/// A packet on its way to the muxer, already on the output timeline.
struct Routed {
    pts: i64,
    key: bool,
    data: Vec<u8>,
}

/// One output stream: its segments in order, routed onto one timeline.
struct Route<'a> {
    media: Media,
    segments: Vec<&'a Segment>,
}

struct Context<'a> {
    orchestrator: &'a Orchestrator,
    inputs: &'a BTreeMap<SourceId, ExportInput>,
    plan: &'a ExportPlan,
    cancel: &'a CancelToken,
    encoding: Option<&'a AudioEncoding>,
    models: Option<&'a Models>,
    commands: &'a Mutex<Vec<String>>,
}

fn seconds(ticks: i64, time_base: Rational) -> f64 {
    crate::time::seconds(ticks, time_base)
}

/// The command that stream-copies `source`'s stream from just before `in`.
fn reader_command(path: &Path, source: &SegmentSource) -> SidecarCommand {
    let from = seconds(source.source_in, source.time_base) - SEEK_MARGIN_SECONDS;
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-copyts");
    if from > 0.0 {
        command = command.option("-ss", format!("{from:.6}"));
    }
    command
        .input(path)
        .option("-map", format!("0:{}", source.stream))
        .option("-c", "copy")
        .option("-output_ts_offset", READER_OFFSET_SECONDS.to_string())
        .option("-f", "nut")
        .output_stdout()
}

/// A copied packet's distance from its segment's in-point, played at
/// `speed`, in ticks of `to` (#42). Every packet is rescaled on its own
/// timestamp, so a variable-frame-rate source keeps its own rhythm, only
/// faster or slower; the packet itself is not touched.
fn retime(ticks: i64, from: Rational, to: Rational, speed: Rational) -> Result<i64, ExportError> {
    let played = Rational {
        num: from.num.saturating_mul(speed.den),
        den: from.den.saturating_mul(speed.num),
    };
    convert(ticks, played, to)
}

/// `ticks` of `from` in `to`, to the nearest tick.
fn convert(ticks: i64, from: Rational, to: Rational) -> Result<i64, ExportError> {
    rescale(ticks, from, to, Rounding::Nearest)
        .ok_or_else(|| ExportError::Mismatch(format!("a timestamp ({ticks}) is out of range")))
}

impl Context<'_> {
    fn path_of(&self, source: SourceId) -> Result<&Path, ExportError> {
        self.inputs
            .get(&source)
            .map(|input| input.source.path())
            .ok_or(ExportError::MissingSource(source))
    }

    /// Route one output stream: open each segment's reader in turn, select
    /// the packets the segment covers, and send them, rebased, to `packets`.
    /// The first reader's stream header goes to `header` before any packet.
    fn route(
        &self,
        route: &Route<'_>,
        header: &mpsc::Sender<Result<(StreamHeader, Metadata), String>>,
        packets: &SyncSender<Routed>,
    ) -> Result<u64, ExportError> {
        let mut output_time_base = None;
        let mut sent = 0;
        let mut stitch: Option<Stitch> = None;
        let mut reference: Option<Vec<u8>> = None;
        if let Some((stream, codec)) = self.reference_first(route, header)? {
            output_time_base = Some(stream.time_base);
            stitch = codec.map(|codec| Stitch::new(codec, &stream.extradata));
            reference = Some(stream.extradata);
        }
        for segment in &route.segments {
            runnable(segment, self.encoding.is_some(), self.models)?;
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            if route.media == Media::Video
                && let ExportTier::FullReEncode { .. } = segment.tier
            {
                sent += self.render_segment(
                    segment,
                    header,
                    &mut output_time_base,
                    &mut stitch,
                    &mut reference,
                    packets,
                )?;
                continue;
            }
            let (command, window) = self.producer(segment)?;
            self.commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(command.to_string());
            let mut opened = Opened::start(self.orchestrator, command)?;
            let stream = opened
                .reader
                .header()
                .streams
                .first()
                .cloned()
                .ok_or_else(|| ExportError::Mismatch("a producer wrote no stream".to_owned()))?;
            let out_base = if let Some(time_base) = output_time_base {
                time_base
            } else {
                let global = self.global_metadata(&opened);
                // The source's nominal rate is not the output's once speeds
                // and joins retime it; the muxer derives the rate from the
                // timestamps instead, down to the last frame's duration.
                let mut output = stream.clone();
                output.metadata.retain(|(key, _)| key != "r_frame_rate");
                let _ = header.send(Ok((output, global)));
                output_time_base = Some(stream.time_base);
                stream.time_base
            };
            if route.media == Media::Video && stitch.is_none() {
                stitch = video_codec(self.inputs, segment)
                    .map(|codec| Stitch::new(codec, &stream.extradata));
                reference = Some(stream.extradata.clone());
            }
            sent += match window {
                Window::Copy(source) => {
                    let place = Place::of(self.plan, segment, source, &stream, out_base)?;
                    self.copy_packets(
                        route.media,
                        &place,
                        (source.source_in, source.source_out, false),
                        &mut opened,
                        packets,
                        stitch.as_mut(),
                    )?
                    .0
                }
                Window::SmartCut(source) => {
                    let place = Place::of(self.plan, segment, source, &stream, out_base)?;
                    self.smart_cut(
                        segment,
                        source,
                        &place,
                        &stream.extradata,
                        &mut opened,
                        packets,
                        stitch.as_mut(),
                    )?
                }
                Window::Encoded {
                    preroll,
                    samples,
                    offset,
                } => self.encoded_segment(
                    segment,
                    &mut opened,
                    &stream,
                    out_base,
                    (preroll, samples, offset),
                    packets,
                )?,
            };
            opened.finish()?;
        }
        Ok(sent)
    }

    /// The process that makes `segment`'s packets, and which of them are
    /// the segment's.
    fn producer<'s>(
        &self,
        segment: &'s Segment,
    ) -> Result<(SidecarCommand, Window<'s>), ExportError> {
        if let (ExportTier::StreamCopy, [source]) = (segment.tier, segment.sources.as_slice()) {
            let path = self.path_of(source.source)?;
            return Ok((reader_command(path, source), Window::Copy(source)));
        }
        if let (ExportTier::SmartCut { .. }, [source]) = (segment.tier, segment.sources.as_slice())
        {
            // The reader opens at the first copied keyframe: its stream
            // header is the output's, whatever the seams are made with.
            let path = self.path_of(source.source)?;
            let first_copy = seam::pieces(segment, source.source_in, source.source_out)
                .into_iter()
                .find_map(|piece| match piece {
                    Piece::Remux { from, .. } => Some(from),
                    Piece::Recode { .. } => None,
                })
                .unwrap_or(source.source_in);
            let from = SegmentSource {
                source_in: first_copy,
                ..source.clone()
            };
            return Ok((reader_command(path, &from), Window::SmartCut(source)));
        }
        let encoding = self
            .encoding
            .ok_or_else(|| ExportError::Unsupported("a re-encoded segment".to_owned()))?;
        let job = audio::encoder_job(
            segment,
            self.inputs,
            encoding,
            self.plan.time_base,
            self.models,
        )?;
        Ok((
            job.command,
            Window::Encoded {
                preroll: job.preroll,
                samples: job.samples,
                offset: job.offset,
            },
        ))
    }

    /// Send the packets of an encoded `segment`: those whose samples are the
    /// segment's own, not the priming on either side.
    fn encoded_segment(
        &self,
        segment: &Segment,
        opened: &mut Opened,
        stream: &StreamHeader,
        out_base: Rational,
        (preroll, samples, offset): (i64, i64, i64),
        packets: &SyncSender<Routed>,
    ) -> Result<u64, ExportError> {
        let rate = self.encoding.map_or(48_000, |e| i64::from(e.sample_rate));
        let per_sample = Rational { num: 1, den: rate };
        let nut_base = stream.time_base;
        let first = convert(offset + preroll, per_sample, nut_base)?;
        let length = convert(samples, per_sample, nut_base)?;
        let start = convert(segment.start, self.plan.time_base, out_base)?;
        let mut sent = 0;
        while let Some(packet) = opened.reader.next_packet()? {
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let at = packet.pts - first;
            if at < 0 {
                continue;
            }
            if at >= length {
                break;
            }
            packets
                .send(Routed {
                    pts: start + convert(at, nut_base, out_base)?,
                    key: packet.key,
                    data: packet.data,
                })
                .map_err(|_| ExportError::Cancelled)?;
            sent += 1;
        }
        Ok(sent)
    }

    /// Where a video stream starts with a rendered segment but copies packets
    /// later, the output's configuration is the copied packets': open the
    /// first copied segment's reader for its header, send it, and return it.
    fn reference_first(
        &self,
        route: &Route<'_>,
        header: &mpsc::Sender<Result<(StreamHeader, Metadata), String>>,
    ) -> Result<Option<(StreamHeader, Option<VideoCodec>)>, ExportError> {
        let rendered = |s: &&Segment| matches!(s.tier, ExportTier::FullReEncode { .. });
        let (Media::Video, Some(first)) = (route.media, route.segments.first()) else {
            return Ok(None);
        };
        let Some(copied) = route.segments.iter().find(|s| !rendered(s)) else {
            return Ok(None);
        };
        if !rendered(first) {
            return Ok(None);
        }
        let (command, _) = self.producer(copied)?;
        let opened = Opened::start(self.orchestrator, command)?;
        let stream = opened
            .reader
            .header()
            .streams
            .first()
            .cloned()
            .ok_or_else(|| ExportError::Mismatch("a reader wrote no stream".to_owned()))?;
        let global = self.global_metadata(&opened);
        opened.finish()?;
        let mut output = stream.clone();
        output.metadata.retain(|(key, _)| key != "r_frame_rate");
        let _ = header.send(Ok((output, global)));
        Ok(Some((stream, video_codec(self.inputs, copied))))
    }

    /// The file metadata a producer carries, less what describes the pipe.
    fn global_metadata(&self, opened: &Opened) -> Metadata {
        let _ = self;
        opened
            .reader
            .header()
            .metadata
            .iter()
            // What wrote the pipe, not what recorded the file.
            .filter(|(key, _)| key != "encoder")
            .cloned()
            .collect()
    }

    /// Render a segment the plan re-encodes whole (#55), check it can join
    /// the copied stream, and send its packets.
    fn render_segment(
        &self,
        segment: &Segment,
        header: &mpsc::Sender<Result<(StreamHeader, Metadata), String>>,
        output_time_base: &mut Option<Rational>,
        stitch: &mut Option<Stitch>,
        reference: &mut Option<Vec<u8>>,
        packets: &SyncSender<Routed>,
    ) -> Result<u64, ExportError> {
        let choice = segment.encoder.as_ref().ok_or_else(|| {
            ExportError::Unsupported("a re-encode with no encoder chosen".to_owned())
        })?;
        let job = render::render_job(segment, self.plan, self.inputs, choice)?;
        let (command, feeders) = match job {
            RenderJob::Direct(command) => (command, Vec::new()),
            RenderJob::Reverse { decoders, encoder } => (encoder, decoders),
        };
        let mut log = self.commands.lock().unwrap_or_else(PoisonError::into_inner);
        log.push(command.to_string());
        log.extend(feeders.iter().map(ToString::to_string));
        drop(log);
        let (mut opened, feeding) = if feeders.is_empty() {
            (Opened::start(self.orchestrator, command)?, None)
        } else {
            // The decoders are started first: the encoder writes nothing,
            // not even its header, until frames reach it.
            let (sender, frames) = mpsc::sync_channel::<Vec<u8>>(8);
            let orchestrator = self.orchestrator.clone();
            let cancel = self.cancel.clone();
            let feeding =
                thread::spawn(move || feed_reversed(&orchestrator, feeders, &sender, &cancel));
            (
                Opened::start_fed(self.orchestrator, command, frames)?,
                Some(feeding),
            )
        };
        let encoded = opened
            .reader
            .header()
            .streams
            .first()
            .cloned()
            .ok_or_else(|| ExportError::Mismatch("a renderer wrote no stream".to_owned()))?;
        let out_base = if let Some(base) = *output_time_base {
            base
        } else {
            // Nothing in this stream is copied: the output is the encoder's.
            let mut output = encoded.clone();
            output.metadata.clear();
            let _ = header.send(Ok((output, self.global_metadata(&opened))));
            *output_time_base = Some(encoded.time_base);
            *stitch = Some(Stitch::new(choice.codec, &encoded.extradata));
            *reference = Some(encoded.extradata.clone());
            encoded.time_base
        };
        if let Some(reference) = reference.as_deref() {
            validate_join(choice.codec, reference, &encoded.extradata)?;
        }
        if let Some(stitch) = stitch.as_mut() {
            stitch.begin_seam(&encoded.extradata);
        }
        let offset = convert(
            render::RENDER_OFFSET_SECONDS,
            Rational { num: 1, den: 1 },
            encoded.time_base,
        )?;
        let start = convert(segment.start, self.plan.time_base, out_base)?;
        let mut sent = 0;
        while let Some(packet) = opened.reader.next_packet()? {
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let data = match stitch.as_mut() {
                Some(stitch) => stitch.seam(&packet.data),
                None => packet.data,
            };
            packets
                .send(Routed {
                    pts: start + convert(packet.pts - offset, encoded.time_base, out_base)?,
                    key: packet.key,
                    data,
                })
                .map_err(|_| ExportError::Cancelled)?;
            sent += 1;
        }
        opened.finish()?;
        if let Some(feeding) = feeding {
            feeding
                .join()
                .unwrap_or_else(|_| Err(ExportError::Mismatch("a decoder panicked".to_owned())))?;
        }
        if sent == 0 {
            return Err(ExportError::Mismatch(format!(
                "the re-encode at frame {} produced no pictures",
                segment.start
            )));
        }
        Ok(sent)
    }

    /// Send the copied packets of `from..to` (source ticks) that `opened`
    /// produces: from the keyframe at `from`, those shown inside the range,
    /// rebased against the segment's in-point. Where `leading` is asked for
    /// and the copy stops on a keyframe, the packets after it that are shown
    /// before it — its leading pictures — are read and not sent, and where
    /// the first of them is shown is returned, in source ticks.
    fn copy_packets(
        &self,
        media: Media,
        place: &Place,
        (from, to, leading): (i64, i64, bool),
        opened: &mut Opened,
        packets: &SyncSender<Routed>,
        mut stitch: Option<&mut Stitch>,
    ) -> Result<(u64, Option<i64>), ExportError> {
        let in_ = place.nut(from)?;
        let out = place.nut(to)?;
        let mut started = media == Media::Audio;
        let mut sent = 0;
        let mut stopped_at = None;
        while let Some(packet) = opened.reader.next_packet()? {
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            if !started {
                if packet.key && packet.pts == in_ {
                    started = true;
                } else if packet.key && packet.pts > in_ {
                    return Err(ExportError::Mismatch(format!(
                        "source {} has no keyframe at {from}",
                        place.source
                    )));
                } else {
                    continue;
                }
            }
            if packet.pts >= out {
                // Past the out-point: a keyframe there means everything
                // before it has been decoded; audio packets are all such.
                if packet.key {
                    stopped_at = Some(packet.pts);
                    break;
                }
                continue;
            }
            if packet.pts < in_ {
                // Leading pictures of an open GOP: shown before the cut.
                continue;
            }
            let data = match stitch.as_deref_mut() {
                Some(stitch) if media == Media::Video => stitch.copied(packet.data, packet.key),
                _ => packet.data,
            };
            packets
                .send(Routed {
                    pts: place.output(packet.pts, place.nut_base)?,
                    key: packet.key,
                    data,
                })
                .map_err(|_| ExportError::Cancelled)?;
            sent += 1;
        }
        if !started {
            return Err(ExportError::Mismatch(format!(
                "source {} ended before {from}",
                place.source
            )));
        }
        let mut first_leading = None;
        if let (true, Some(keyframe)) = (leading, stopped_at) {
            while let Some(packet) = opened.reader.next_packet()? {
                if packet.pts >= keyframe {
                    break;
                }
                first_leading = Some(first_leading.map_or(packet.pts, |p: i64| p.min(packet.pts)));
            }
        }
        let first_leading = first_leading
            .map(|pts| place.source_ticks(pts))
            .transpose()?;
        Ok((sent, first_leading))
    }

    /// Carry out a smart-cut: the plan's pieces in order, the copied ones
    /// from `opened`, the recoded ones each from a seam encoder.
    #[allow(clippy::too_many_arguments)]
    fn smart_cut(
        &self,
        segment: &Segment,
        source: &SegmentSource,
        place: &Place,
        extradata: &[u8],
        opened: &mut Opened,
        packets: &SyncSender<Routed>,
        mut stitch: Option<&mut Stitch>,
    ) -> Result<u64, ExportError> {
        let choice = segment.encoder.as_ref().ok_or_else(|| {
            ExportError::Unsupported("a smart-cut with no encoder chosen".to_owned())
        })?;
        let mut sent = 0;
        let mut recode_from = None;
        for piece in seam::pieces(segment, source.source_in, source.source_out) {
            match piece {
                Piece::Recode { from, to } => {
                    let from = recode_from
                        .take()
                        .map_or(from, |leading: i64| leading.min(from));
                    sent += self.recode_piece(
                        source,
                        place,
                        (from, to),
                        choice,
                        extradata,
                        packets,
                        stitch.as_deref_mut(),
                    )?;
                }
                Piece::Remux { from, to, leading } => {
                    let (copied, first_leading) = self.copy_packets(
                        Media::Video,
                        place,
                        (from, to, leading),
                        opened,
                        packets,
                        stitch.as_deref_mut(),
                    )?;
                    sent += copied;
                    recode_from = first_leading;
                }
            }
        }
        Ok(sent)
    }

    /// Encode the pictures of `source` shown in `from..to` with `choice`,
    /// check that they can join the copied stream before a packet of them is
    /// sent, and send them.
    #[allow(clippy::too_many_arguments)]
    fn recode_piece(
        &self,
        source: &SegmentSource,
        place: &Place,
        (from, to): (i64, i64),
        choice: &EncoderChoice,
        extradata: &[u8],
        packets: &SyncSender<Routed>,
        stitch: Option<&mut Stitch>,
    ) -> Result<u64, ExportError> {
        let path = self.path_of(source.source)?;
        let command = seam_command(path, source, (from, to), choice);
        self.commands
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(command.to_string());
        let mut opened = Opened::start(self.orchestrator, command)?;
        let encoded = opened
            .reader
            .header()
            .streams
            .first()
            .cloned()
            .ok_or_else(|| ExportError::Mismatch("a seam encoder wrote no stream".to_owned()))?;
        validate_join(choice.codec, extradata, &encoded.extradata)?;
        let mut stitch = stitch;
        if let Some(stitch) = stitch.as_deref_mut() {
            stitch.begin_seam(&encoded.extradata);
        }
        let seam_place = Place {
            nut_base: encoded.time_base,
            ..place.clone()
        };
        let low = seam_place.nut(from)?;
        let high = seam_place.nut(to)?;
        let mut sent = 0;
        while let Some(packet) = opened.reader.next_packet()? {
            if self.cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            if packet.pts < low || packet.pts >= high {
                continue;
            }
            let data = match stitch.as_deref_mut() {
                Some(stitch) => stitch.seam(&packet.data),
                None => packet.data,
            };
            packets
                .send(Routed {
                    pts: seam_place.output(packet.pts, encoded.time_base)?,
                    key: packet.key,
                    data,
                })
                .map_err(|_| ExportError::Cancelled)?;
            sent += 1;
        }
        opened.finish()?;
        if sent == 0 {
            return Err(ExportError::Mismatch(format!(
                "the seam of source {} from {from} to {to} produced no pictures",
                source.source
            )));
        }
        Ok(sent)
    }
}

/// The codec of `source`'s stream, as the probe names it.
fn codec_of(inputs: &BTreeMap<SourceId, ExportInput>, source: &SegmentSource) -> Option<String> {
    inputs
        .get(&source.source)?
        .info
        .streams
        .iter()
        .find(|stream| stream.index == source.stream)?
        .codec
        .clone()
}

/// The display rotation of the first video source, which the copied stream
/// must keep: NUT does not carry it, so the muxer is told.
fn rotation(plan: &ExportPlan, inputs: &BTreeMap<SourceId, ExportInput>) -> u32 {
    plan.segments
        .iter()
        .filter(|segment| segment.media == Media::Video)
        .find_map(|segment| segment.sources.first())
        .and_then(|source| {
            inputs
                .get(&source.source)?
                .info
                .streams
                .iter()
                .find(|stream| stream.index == source.stream)
                .and_then(|stream| match &stream.kind {
                    StreamKind::Video(video) => Some(video.rotation),
                    _ => None,
                })
        })
        .unwrap_or(0)
}

/// The sources' chapters, moved onto the output timeline: a chapter starting
/// inside a copied segment starts at the same point of that segment's
/// output, and ends with the chapter or the segment, whichever is first.
fn chapters(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    primary: Media,
) -> Vec<nut::Chapter> {
    const MILLISECONDS: Rational = Rational { num: 1, den: 1000 };
    let mut found: Vec<nut::Chapter> = Vec::new();
    for segment in plan.segments.iter().filter(|s| s.media == primary) {
        let [source] = segment.sources.as_slice() else {
            continue;
        };
        let Some(input) = inputs.get(&source.source) else {
            continue;
        };
        let in_ = seconds(source.source_in, source.time_base);
        let out = seconds(source.source_out, source.time_base);
        let start = seconds(segment.start, plan.time_base);
        for chapter in &input.info.container.chapters {
            let from = chapter.start_seconds.max(in_);
            let to = chapter.end_seconds.min(out);
            if to <= from || !(in_..out).contains(&from) {
                continue;
            }
            let at = crate::time::from_seconds(start + from - in_, MILLISECONDS, Rounding::Nearest);
            let length = crate::time::from_seconds(to - from, MILLISECONDS, Rounding::Nearest);
            let (Ok(at), Ok(length)) = (u64::try_from(at), u64::try_from(length)) else {
                continue;
            };
            if found.iter().any(|c| c.start == at) || length == 0 {
                continue;
            }
            found.push(nut::Chapter {
                time_base: MILLISECONDS,
                start: at,
                length,
                metadata: chapter
                    .title
                    .iter()
                    .map(|title| ("title".to_owned(), title.clone()))
                    .collect(),
            });
        }
    }
    found.sort_by_key(|chapter| chapter.start);
    found
}

/// Whether this executor can carry out `segment`: a copy of one source, at
/// any speed the plan kept a copy (#42), or — for sound — an encode the audio
/// path can make.
/// Anything else is refused before any output is written, never copied or
/// encoded differently from what the plan chose.
fn runnable(
    segment: &Segment,
    encodes_audio: bool,
    models: Option<&Models>,
) -> Result<(), ExportError> {
    match (segment.tier, segment.sources.as_slice()) {
        (ExportTier::StreamCopy, [_]) => Ok(()),
        (ExportTier::SmartCut { .. }, [_]) | (ExportTier::FullReEncode { .. }, _)
            if segment.media == Media::Video && segment.encoder.is_some() =>
        {
            Ok(())
        }
        (ExportTier::FullReEncode { .. }, _) if segment.media == Media::Audio && encodes_audio => {
            audio::check(segment, models)
        }
        (tier, _) => Err(ExportError::Unsupported(format!(
            "the {:?} segment at frame {} ({tier:?})",
            segment.media, segment.start
        ))),
    }
}

/// Everything [`export`] checks before a process starts, answered before
/// the export is asked for (#50): what its sound will be encoded to, or why
/// it would be refused.
///
/// # Errors
///
/// What [`export`] would refuse with before running anything.
pub fn check(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    target: &Path,
    overwrite: bool,
    audio: AudioTarget,
    models: Option<&Models>,
) -> Result<Option<AudioEncoding>, ExportError> {
    preflight(&ExportRequest {
        plan,
        inputs,
        target,
        overwrite,
        audio,
        models,
        cancel: CancelToken::default(),
        on_progress: None,
    })
    .map(|(_, encoding)| encoding)
}

/// Check everything that can be checked before a process starts.
fn preflight(
    request: &ExportRequest<'_>,
) -> Result<(Container, Option<AudioEncoding>), ExportError> {
    let container = Container::of(request.target)?;
    let encoding = audio::encoding_for(request.plan, request.inputs, request.audio, container)?;
    if let Some(segment) = request.plan.segments.iter().find(|s| s.decline.is_some()) {
        return Err(ExportError::Declined(format!(
            "the {:?} segment at frame {}",
            segment.media, segment.start
        )));
    }
    for segment in &request.plan.segments {
        runnable(segment, encoding.is_some(), request.models)?;
        for source in &segment.sources {
            if !request.inputs.contains_key(&source.source) {
                return Err(ExportError::MissingSource(source.source));
            }
            if segment.tier.is_lossless()
                && let Some(codec) = codec_of(request.inputs, source)
                && !container.accepts(&codec)
            {
                return Err(ExportError::CodecNotInContainer {
                    codec,
                    container: container.muxer().to_owned(),
                });
            }
        }
    }
    // Before the overwrite check, so that no confirmation can reach a source:
    // `CLAUDE.md` section 19 — no operation overwrites a source file.
    if let Some(input) = request
        .inputs
        .values()
        .find(|input| same_file(input.source.path(), request.target))
    {
        return Err(ExportError::TargetIsSource(
            input.source.path().to_path_buf(),
        ));
    }
    if request.target.exists() && !request.overwrite {
        return Err(ExportError::TargetExists(request.target.to_path_buf()));
    }
    Ok((container, encoding))
}

/// Whether `a` and `b` name one file. Resolved where both exist, so a
/// relative path, a different case or a `..` cannot hide a source; compared
/// as written where either does not, since a file that does not exist yet is
/// not a source.
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Each output stream's segments, video first.
fn routes_of(plan: &ExportPlan) -> Vec<Route<'_>> {
    [Media::Video, Media::Audio]
        .into_iter()
        .map(|media| Route {
            media,
            segments: plan.segments.iter().filter(|s| s.media == media).collect(),
        })
        .filter(|route| !route.segments.is_empty())
        .collect()
}

/// Carry out `request.plan`, writing the output file.
///
/// # Errors
///
/// See [`ExportError`]. On any error, and on cancellation, nothing is left
/// at the target and the partial file is removed.
pub fn export(
    orchestrator: &Orchestrator,
    request: ExportRequest<'_>,
) -> Result<ExportOutcome, ExportError> {
    let (container, encoding) = preflight(&request)?;
    let partial = partial_path(request.target);
    if partial.exists() {
        // Only an earlier export of this same target makes this name.
        std::fs::remove_file(&partial)?;
    }
    let result = run(
        orchestrator,
        request,
        container,
        encoding.as_ref(),
        &partial,
    );
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn run(
    orchestrator: &Orchestrator,
    request: ExportRequest<'_>,
    container: Container,
    encoding: Option<&AudioEncoding>,
    partial: &Path,
) -> Result<ExportOutcome, ExportError> {
    let ExportRequest {
        plan,
        inputs,
        target,
        overwrite,
        audio: _,
        models,
        cancel,
        on_progress,
    } = request;
    let commands = Mutex::new(Vec::new());
    let routes = routes_of(plan);
    if routes.is_empty() {
        return Err(ExportError::Unsupported(
            "an export with no streams".to_owned(),
        ));
    }
    let context = Context {
        orchestrator,
        inputs,
        plan,
        cancel: &cancel,
        encoding,
        models,
        commands: &commands,
    };
    let primary = routes.first().map_or(Media::Video, |route| route.media);

    thread::scope(|scope| -> Result<ExportOutcome, ExportError> {
        let mut headers = Vec::new();
        let mut queues = Vec::new();
        let mut workers = Vec::new();
        for route in &routes {
            let (header_sender, header) = mpsc::channel();
            let (packet_sender, packets) = mpsc::sync_channel(PACKET_QUEUE);
            let context = &context;
            workers.push(scope.spawn(move || {
                let result = context.route(route, &header_sender, &packet_sender);
                if let Err(error) = &result {
                    let _ = header_sender.send(Err(error.to_string()));
                }
                result
            }));
            headers.push(header);
            queues.push(packets);
        }
        let Some((streams, metadata)) = collect_headers(&headers) else {
            // Unblock and stop every router before collecting its error.
            cancel.cancel();
            drop(queues);
            return Err(first_error(workers, "a stream produced no header"));
        };
        let header = Header {
            metadata,
            chapters: chapters(plan, inputs, primary),
            streams: streams.clone(),
        };

        let (chunk_sender, chunks) = mpsc::sync_channel(MUXER_QUEUE);
        let command = muxer_command(plan, inputs, &streams, container, partial);
        let mux_job = start_muxer(
            orchestrator,
            command,
            &cancel,
            chunks,
            partial,
            on_progress,
            &commands,
        );

        let written = interleave(
            &header,
            &streams,
            queues,
            ChunkWriter {
                chunks: chunk_sender,
                buffer: Vec::with_capacity(MUXER_CHUNK),
            },
            &cancel,
        );
        let router_results = join_all(workers);
        let muxed = mux_job.wait();
        conclude(
            &cancel,
            router_results,
            written,
            muxed,
            partial,
            target,
            overwrite,
        )
        .map(|(path, packets)| ExportOutcome {
            path,
            packets,
            audio: encoding.cloned(),
            commands: commands
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
        })
    })
}

/// Settle an export once every process has finished: any failure is the
/// export's, and only a complete output is moved to the target.
fn conclude(
    cancel: &CancelToken,
    routed: Vec<Result<u64, ExportError>>,
    written: Result<Vec<u64>, ExportError>,
    muxed: Result<crate::orchestrator::JobOutput, JobError>,
    partial: &Path,
    target: &Path,
    overwrite: bool,
) -> Result<(PathBuf, Vec<u64>), ExportError> {
    if cancel.is_cancelled() {
        return Err(ExportError::Cancelled);
    }
    for result in routed {
        result?;
    }
    let packets = written?;
    match muxed {
        Ok(_) => {}
        Err(JobError::Cancelled) => return Err(ExportError::Cancelled),
        Err(error) => return Err(error.into()),
    }
    if overwrite && target.exists() {
        std::fs::remove_file(target)?;
    }
    std::fs::rename(partial, target)?;
    Ok((target.to_path_buf(), packets))
}

/// Wait for every router, a panic counting as a failure.
fn join_all(
    workers: Vec<thread::ScopedJoinHandle<'_, Result<u64, ExportError>>>,
) -> Vec<Result<u64, ExportError>> {
    workers
        .into_iter()
        .map(|worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err(ExportError::Mismatch("a router panicked".to_owned())))
        })
        .collect()
}

/// Start the muxer, fed from `chunks`, and record its command.
fn start_muxer(
    orchestrator: &Orchestrator,
    command: SidecarCommand,
    cancel: &CancelToken,
    chunks: Receiver<Vec<u8>>,
    partial: &Path,
    on_progress: Option<Box<dyn FnMut(JobProgress) + Send>>,
    commands: &Mutex<Vec<String>>,
) -> crate::orchestrator::Job {
    commands
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(command.to_string());
    let mut options = JobOptions::default()
        .cancel_token(cancel.clone())
        .stdin(chunks)
        .remove_on_failure(partial.to_path_buf());
    if let Some(callback) = on_progress {
        options = options.on_progress(callback);
    }
    orchestrator.run(command, Priority::Export, options)
}

/// Every route's stream header, in route order, and the file metadata of
/// the first that has any; `None` if a route failed before its header.
fn collect_headers(
    headers: &[Receiver<Result<(StreamHeader, Metadata), String>>],
) -> Option<(Vec<StreamHeader>, Metadata)> {
    let mut streams = Vec::new();
    let mut metadata = Vec::new();
    for header in headers {
        let (stream, global) = header.recv().ok()?.ok()?;
        if metadata.is_empty() {
            metadata = global;
        }
        streams.push(stream);
    }
    Some((streams, metadata))
}

/// The first error among `workers`, or `fallback`.
fn first_error(
    workers: Vec<thread::ScopedJoinHandle<'_, Result<u64, ExportError>>>,
    fallback: &str,
) -> ExportError {
    workers
        .into_iter()
        .filter_map(|worker| worker.join().ok())
        .find_map(Result::err)
        .unwrap_or_else(|| ExportError::Mismatch(fallback.to_owned()))
}

/// The muxer: NUT on its standard input, every stream copied into the
/// container, the metadata and chapters the router wrote carried over, and
/// the rotation NUT cannot carry set again.
fn muxer_command(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    streams: &[StreamHeader],
    container: Container,
    partial: &Path,
) -> SidecarCommand {
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .option("-f", "nut");
    let angle = rotation(plan, inputs);
    if angle != 0 && streams.iter().any(|s| s.class == nut::Class::Video) {
        command = command.option("-display_rotation:v:0", angle.to_string());
    }
    let total = Duration::from_secs_f64(seconds(plan.length, plan.time_base).max(0.0));
    let mut command = command
        .stdin_input()
        .option("-map", "0")
        .option("-c", "copy");
    if let Some(tag) = in_band_tag(plan, inputs, container) {
        command = command.option("-tag:v", tag);
    }
    command
        .option("-map_metadata", "0")
        .option("-map_chapters", "0")
        .option("-f", container.muxer())
        .output_file(partial)
        .report_progress(total)
}

/// Run each chunk's decoder in turn — the last chunk of the clip first — and
/// pass its raw, reversed frames to the encoder's input. One decoder runs at
/// a time, so the frames held are one chunk's.
fn feed_reversed(
    orchestrator: &Orchestrator,
    decoders: Vec<SidecarCommand>,
    frames: &SyncSender<Vec<u8>>,
    cancel: &CancelToken,
) -> Result<(), ExportError> {
    for decoder in decoders {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let sender = frames.clone();
        let job = orchestrator.run(
            decoder,
            Priority::Export,
            JobOptions::default().on_chunk(move |chunk| {
                if sender.send(chunk.to_vec()).is_err() {
                    Flow::Fail("the encoder stopped reading".to_owned())
                } else {
                    Flow::Continue
                }
            }),
        );
        job.wait()?;
    }
    Ok(())
}

/// Where seams put parameter sets in-band, the MP4 sample entry that says
/// they may change there: `avc3` for H.264, `hev1` for HEVC. Matroska needs
/// nothing.
fn in_band_tag(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    container: Container,
) -> Option<&'static str> {
    if !matches!(container, Container::Mp4 | Container::Mov) {
        return None;
    }
    let seam = plan
        .segments
        .iter()
        .find(|s| s.media == Media::Video && !s.tier.is_lossless())?;
    let seam = plan
        .segments
        .iter()
        .find(|s| s.media == Media::Video && !s.sources.is_empty())
        .filter(|_| !seam.tier.is_lossless())?;
    match video_codec(inputs, seam)? {
        VideoCodec::H264 => Some("avc3"),
        VideoCodec::Hevc => Some("hev1"),
        VideoCodec::Vp9 | VideoCodec::Av1 => None,
    }
}

/// The codec of a video segment's source.
fn video_codec(inputs: &BTreeMap<SourceId, ExportInput>, segment: &Segment) -> Option<VideoCodec> {
    let source = segment.sources.first()?;
    Some(match codec_of(inputs, source)?.as_str() {
        "h264" => VideoCodec::H264,
        "hevc" => VideoCodec::Hevc,
        "vp9" => VideoCodec::Vp9,
        "av1" => VideoCodec::Av1,
        _ => return None,
    })
}

/// The seam encoder for `from..to` of `source`: decoded from the keyframe
/// before it, trimmed to exactly those pictures on their own timestamps,
/// and encoded as `choice` says, without B-frames so its decode order is
/// its presentation order.
fn seam_command(
    path: &Path,
    source: &SegmentSource,
    (from, to): (i64, i64),
    choice: &EncoderChoice,
) -> SidecarCommand {
    let seek = seconds(from, source.time_base) - SEEK_MARGIN_SECONDS;
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-copyts");
    if seek > 0.0 {
        command = command.option("-ss", format!("{seek:.6}"));
    }
    command = command
        .input(path)
        .option("-map", format!("0:{}", source.stream))
        .option("-vf", format!("trim=start_pts={from}:end_pts={to}"));
    for (name, value) in choice.arguments(choice.codec) {
        command = command.option(name, value);
    }
    if choice.source != crate::capability::EncoderSource::Software {
        command = command.option("-bf", "0");
    }
    command
        .option("-fps_mode", "passthrough")
        .option("-enc_time_base", "demux")
        .option("-output_ts_offset", READER_OFFSET_SECONDS.to_string())
        .option("-f", "nut")
        .output_stdout()
}

/// Write every routed packet into the muxer, the streams interleaved by
/// presentation time, and close its input. Returns the packets written per
/// stream.
// `queues` is taken by value on purpose: dropping it when writing fails is
// what unblocks the routers waiting to send.
#[allow(clippy::needless_pass_by_value)]
fn interleave(
    header: &Header,
    streams: &[StreamHeader],
    queues: Vec<Receiver<Routed>>,
    output: ChunkWriter,
    cancel: &CancelToken,
) -> Result<Vec<u64>, ExportError> {
    let mut writer = nut::Writer::new(output, header)?;
    let mut heads: Vec<Option<Routed>> = queues.iter().map(|queue| queue.recv().ok()).collect();
    let mut written = vec![0_u64; queues.len()];
    loop {
        if cancel.is_cancelled() {
            return Err(ExportError::Cancelled);
        }
        let next = heads
            .iter()
            .enumerate()
            .filter_map(|(index, head)| {
                let head = head.as_ref()?;
                let time_base = streams.get(index)?.time_base;
                Some((index, head.pts, time_base))
            })
            .min_by(|a, b| {
                let left = i128::from(a.1) * i128::from(a.2.num) * i128::from(b.2.den);
                let right = i128::from(b.1) * i128::from(b.2.num) * i128::from(a.2.den);
                left.cmp(&right)
            })
            .map(|(index, ..)| index);
        let Some(index) = next else {
            break;
        };
        let Some(packet) = heads.get_mut(index).and_then(Option::take) else {
            break;
        };
        writer.write(&Packet {
            stream: index,
            pts: packet.pts,
            key: packet.key,
            data: packet.data,
        })?;
        if let Some(count) = written.get_mut(index) {
            *count += 1;
        }
        if let (Some(slot), Some(queue)) = (heads.get_mut(index), queues.get(index)) {
            *slot = queue.recv().ok();
        }
    }
    let mut output = writer.finish()?;
    output.flush()?;
    Ok(written)
}
