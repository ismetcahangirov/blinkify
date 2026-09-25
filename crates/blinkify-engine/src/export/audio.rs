//! The audio export path (#43): sound that the plan re-encodes — adjusted,
//! mixed, resampled for a speed, or silence in a gap — encoded by a sidecar
//! process of its own, while the pictures beside it are copied untouched.
//!
//! The encoder writes NUT to a pipe like every other producer (ADR-0010), so
//! an encoded stretch of sound joins the same router as copied sound and the
//! video is never decoded: nothing on this path so much as opens a video
//! decoder.
//!
//! What it holds to:
//!
//! - **The codec is chosen, never defaulted.** Where some of the output's
//!   sound is copied, the encoded stretches must join it in one stream, so
//!   they are encoded to the copied sound's codec, sample rate and channels.
//!   Where none is copied, the caller's [`AudioTarget`] decides: high-bitrate
//!   AAC by default, or a lossless target — FLAC or PCM — for a user who
//!   repaired a recording and does not want a second lossy encode on top.
//! - **The same processing as the preview.** Gain is `volume`, speed is
//!   pitch-preserving `atempo`, exactly as the preview decoder applies them.
//!   Denoise and normalise are Epic #7's; until they exist the export refuses
//!   them rather than inventing a filter the preview does not play.
//! - **Exact length.** The encoded stretch is padded and trimmed to the
//!   segment's duration in samples, so sound and pictures stay aligned.
//! - **Clean joins.** A lossy encoder needs the sound before a stretch to
//!   start it (priming) and after it to end it; the encoder is given real
//!   sound on both sides, whole codec frames of it, and only the packets of
//!   the stretch itself are kept.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use ts_rs::TS;

use super::execute::{Container, ExportError, ExportInput};
use super::plan::{ExportPlan, Media, Segment, SegmentSource};
use crate::audio::decoder::tempo_stages;
use crate::orchestrator::SidecarCommand;
use crate::probe::{Rational, StreamInfo, StreamKind};
use crate::project::SourceId;
use crate::project::evaluate::AudioOperation;
use crate::tier::ExportTier;

/// Seconds added to every timestamp the encoder writes, as for readers.
const ENCODER_OFFSET_SECONDS: i64 = 100;

/// How far before a stretch the source is decoded from, so the demuxer's
/// seek lands before it.
const SEEK_MARGIN_SECONDS: f64 = 3.0;

/// Codec frames of real sound given to a lossy encoder on each side.
const PRIMING_FRAMES: i64 = 2;

/// What the output's sound is encoded to where none of it is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "codec",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum AudioTarget {
    /// AAC-LC at a high bitrate: plays everywhere.
    Aac { kilobits: u32 },
    /// Opus: what a `WebM` file carries.
    Opus { kilobits: u32 },
    /// FLAC at 24 bits: decodes to exactly the processed samples at 24-bit
    /// resolution, which is below any source's own noise floor.
    Flac,
    /// 24-bit PCM: the same samples uncompressed, where the container allows.
    Pcm,
}

impl Default for AudioTarget {
    fn default() -> Self {
        Self::Aac { kilobits: 256 }
    }
}

/// How the output's encoded sound is made: stated in the export report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioEncoding {
    /// The codec, as FFmpeg names it: `aac`, `flac`, `pcm_s16le`.
    pub codec: String,
    /// The FFmpeg encoder that makes it.
    pub encoder: String,
    /// For a lossy codec.
    pub kilobits: Option<u32>,
    pub sample_rate: u32,
    pub channels: u32,
    /// Encoded to match copied sound in the same stream, rather than to the
    /// caller's target.
    pub matches_copied: bool,
    pub lossless: bool,
}

impl AudioEncoding {
    /// Samples per codec frame, for a codec that primes: whole frames of
    /// real sound are given on either side of a stretch.
    fn frame(&self) -> i64 {
        match self.codec.as_str() {
            "aac" => 1024,
            "opus" => 960,
            "mp3" => 1152,
            _ => 0,
        }
    }

    fn layout(&self) -> &'static str {
        match self.channels {
            1 => "mono",
            6 => "5.1",
            8 => "7.1",
            _ => "stereo",
        }
    }
}

fn stream_of<'a>(
    inputs: &'a BTreeMap<SourceId, ExportInput>,
    source: &SegmentSource,
) -> Option<&'a StreamInfo> {
    inputs
        .get(&source.source)?
        .info
        .streams
        .iter()
        .find(|stream| stream.index == source.stream)
}

fn audio_shape(stream: &StreamInfo) -> Option<(u32, u32)> {
    match &stream.kind {
        StreamKind::Audio(audio) => Some((audio.sample_rate?, audio.channels?)),
        _ => None,
    }
}

/// The encoder for `codec` that reproduces it, where one ships.
fn matching_encoder(codec: &str) -> Option<(&'static str, bool)> {
    Some(match codec {
        "aac" => ("aac", false),
        "opus" => ("libopus", false),
        "mp3" => ("libmp3lame", false),
        "vorbis" => ("libvorbis", false),
        "flac" => ("flac", true),
        "pcm_s16le" => ("pcm_s16le", true),
        "pcm_s24le" => ("pcm_s24le", true),
        _ => return None,
    })
}

/// How the plan's encoded sound is to be made, or `None` when all of it is
/// copied.
///
/// # Errors
///
/// No encoder can match the copied sound, or the container cannot hold the
/// codec: both before anything runs.
pub fn encoding_for(
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    target: AudioTarget,
    container: Container,
) -> Result<Option<AudioEncoding>, ExportError> {
    let audio: Vec<&Segment> = plan
        .segments
        .iter()
        .filter(|s| s.media == Media::Audio)
        .collect();
    if audio.iter().all(|s| s.tier.is_lossless()) {
        return Ok(None);
    }
    let copied = audio
        .iter()
        .filter(|s| s.tier.is_lossless())
        .find_map(|s| s.sources.first())
        .and_then(|source| stream_of(inputs, source));
    let encoding = if let Some(stream) = copied {
        let codec = stream.codec.clone().unwrap_or_default();
        let (encoder, lossless) = matching_encoder(&codec).ok_or_else(|| {
            ExportError::Unsupported(format!(
                "no encoder can match the copied {codec} sound around a re-encoded stretch"
            ))
        })?;
        let (sample_rate, channels) = audio_shape(stream).ok_or_else(|| {
            ExportError::Mismatch("the copied sound has no sample rate".to_owned())
        })?;
        let kilobits = (!lossless).then(|| {
            let source = stream.bit_rate.unwrap_or(0.0) / 1000.0;
            // At least the source's own rate, and never a starved one.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let source = source.ceil() as u32;
            source.max(match target {
                AudioTarget::Aac { kilobits } | AudioTarget::Opus { kilobits } => kilobits,
                AudioTarget::Flac | AudioTarget::Pcm => 256,
            })
        });
        AudioEncoding {
            codec,
            encoder: encoder.to_owned(),
            kilobits,
            sample_rate,
            channels,
            matches_copied: true,
            lossless,
        }
    } else {
        let (sample_rate, channels) = audio
            .iter()
            .flat_map(|s| s.sources.iter())
            .find_map(|source| stream_of(inputs, source).and_then(audio_shape))
            .unwrap_or((48_000, 2));
        let (codec, encoder, kilobits, lossless, sample_rate) = match target {
            AudioTarget::Aac { kilobits } => ("aac", "aac", Some(kilobits), false, sample_rate),
            // Opus runs at 48 kHz whatever it is given.
            AudioTarget::Opus { kilobits } => ("opus", "libopus", Some(kilobits), false, 48_000),
            AudioTarget::Flac => ("flac", "flac", None, true, sample_rate),
            AudioTarget::Pcm => ("pcm_s24le", "pcm_s24le", None, true, sample_rate),
        };
        AudioEncoding {
            codec: codec.to_owned(),
            encoder: encoder.to_owned(),
            kilobits,
            sample_rate,
            channels,
            matches_copied: false,
            lossless,
        }
    };
    if !container.accepts(&encoding.codec) {
        return Err(ExportError::CodecNotInContainer {
            codec: encoding.codec.clone(),
            container: format!("{container:?}"),
        });
    }
    Ok(Some(encoding))
}

/// Whether the audio path can make `segment`: every step of its chain one
/// the preview also plays, and no hold or reverse.
///
/// # Errors
///
/// [`ExportError::Unsupported`], naming what cannot be made yet.
pub fn check(segment: &Segment) -> Result<(), ExportError> {
    if matches!(segment.tier, ExportTier::SmartCut { .. }) {
        return Err(ExportError::Unsupported("a smart-cut of sound".to_owned()));
    }
    for source in &segment.sources {
        if source.motion.is_some() {
            return Err(ExportError::Unsupported(
                "the sound of a held or reversed clip".to_owned(),
            ));
        }
    }
    for operation in segment.sources.iter().flat_map(|source| &source.audio) {
        match operation {
            AudioOperation::Gain { .. } => {}
            AudioOperation::Denoise { .. } => {
                return Err(ExportError::Unsupported(
                    "noise reduction, which arrives with Epic #7".to_owned(),
                ));
            }
            AudioOperation::Normalise { .. } => {
                return Err(ExportError::Unsupported(
                    "loudness normalisation, which arrives with Epic #7".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn speed_of(source: &SegmentSource) -> f64 {
    source.speed.value().unwrap_or(1.0)
}

/// The filter chain of input `i`: its range with `preroll` samples of real
/// sound either side (silence where the stream has none), its gain, its
/// speed, and the encoding's rate and layout; and where to seek it from.
fn source_chain(
    i: usize,
    source: &SegmentSource,
    stream: &StreamInfo,
    encoding: &AudioEncoding,
    preroll: i64,
) -> (f64, String) {
    let rate = i64::from(encoding.sample_rate);
    let input_rate = audio_shape(stream).map_or(rate, |(r, _)| i64::from(r));
    let speed = speed_of(source);
    #[allow(clippy::cast_precision_loss)]
    let preroll_in = preroll as f64 / rate as f64 * speed;
    let in_ = crate::time::seconds(source.source_in, source.time_base);
    let out = crate::time::seconds(source.source_out, source.time_base);
    let stream_start = stream.start_seconds.unwrap_or(0.0).max(0.0);
    let from = (in_ - preroll_in).max(stream_start);
    let missing = (from - (in_ - preroll_in)).max(0.0);
    let mut chain = format!(
        "[{i}:{stream}]atrim=start={from:.6}:end={end:.6},asetpts=PTS-STARTPTS",
        stream = source.stream,
        end = out + preroll_in,
    );
    if missing > 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let delay = (missing * input_rate as f64).round() as i64;
        let _ = write!(chain, ",adelay=delays={delay}S:all=1");
    }
    for operation in &source.audio {
        if let AudioOperation::Gain { db } = operation {
            let _ = write!(chain, ",volume={db}dB");
        }
    }
    for factor in tempo_stages(speed) {
        let _ = write!(chain, ",atempo={factor}");
    }
    let _ = write!(
        chain,
        ",aresample={rate},aformat=channel_layouts={layout}[p{i}];",
        layout = encoding.layout()
    );
    (from - SEEK_MARGIN_SECONDS, chain)
}

/// An encoder process for one audio segment, and where in its output the
/// segment's own samples are.
#[derive(Debug)]
pub struct EncoderJob {
    pub command: SidecarCommand,
    /// Samples, at the encoding's rate, before the segment's first.
    pub preroll: i64,
    /// The segment's length in samples.
    pub samples: i64,
    /// Added by the encoder to every timestamp, in samples.
    pub offset: i64,
}

/// The encoder for `segment`: each of its sources decoded from just before
/// its range, trimmed with real sound either side, put through its chain and
/// speed, mixed where there are several, padded and trimmed to exactly the
/// segment's length plus the priming, and encoded.
///
/// # Errors
///
/// A source is missing, or the segment is one the path cannot make.
pub fn encoder_job(
    segment: &Segment,
    inputs: &BTreeMap<SourceId, ExportInput>,
    encoding: &AudioEncoding,
    sequence: Rational,
) -> Result<EncoderJob, ExportError> {
    check(segment)?;
    let rate = i64::from(encoding.sample_rate);
    let per_sample = Rational { num: 1, den: rate };
    let samples = crate::time::rescale(
        segment.length,
        sequence,
        per_sample,
        crate::time::Rounding::Nearest,
    )
    .ok_or_else(|| ExportError::Mismatch("a segment's length is out of range".to_owned()))?;
    let preroll = PRIMING_FRAMES * encoding.frame();
    let total = preroll + samples + preroll;
    let layout = encoding.layout();
    let mut command = SidecarCommand::ffmpeg().option("-v", "error");
    let mut graph = String::new();
    let mut labels = Vec::new();
    if segment.sources.is_empty() {
        command = command.lavfi_input(&format!("anullsrc=r={rate}:cl={layout}"));
        let _ = write!(graph, "[0:a]anull[p0];");
        labels.push("[p0]".to_owned());
    }
    if !segment.sources.is_empty() {
        command = command.flag("-copyts");
    }
    for (i, source) in segment.sources.iter().enumerate() {
        let input = inputs
            .get(&source.source)
            .ok_or(ExportError::MissingSource(source.source))?;
        let stream = stream_of(inputs, source).ok_or_else(|| {
            ExportError::Mismatch(format!(
                "source {} has no stream {}",
                source.source, source.stream
            ))
        })?;
        let (seek, chain) = source_chain(i, source, stream, encoding, preroll);
        if seek > 0.0 {
            command = command.option("-ss", format!("{seek:.6}"));
        }
        command = command.input(input.source.path());
        graph.push_str(&chain);
        labels.push(format!("[p{i}]"));
    }
    let mixed = if labels.len() > 1 {
        let _ = write!(
            graph,
            "{}amix=inputs={}:normalize=0:duration=longest[mix];",
            labels.concat(),
            labels.len()
        );
        "[mix]".to_owned()
    } else {
        labels.concat()
    };
    let _ = write!(
        graph,
        "{mixed}apad,atrim=end_sample={total},asetpts=N/SR/TB[out]"
    );
    command = command
        .option("-filter_complex", graph)
        .option("-map", "[out]")
        .option("-c:a", encoding.encoder.clone())
        .option("-ar", rate.to_string())
        .option("-ac", encoding.channels.to_string());
    if let Some(kilobits) = encoding.kilobits {
        command = command.option("-b:a", format!("{kilobits}k"));
    }
    if encoding.codec == "flac" {
        // Stated, not negotiated: 24 bits in 32-bit samples.
        command = command.option("-sample_fmt", "s32");
    }
    if let [source] = segment.sources.as_slice() {
        command = command.option("-map_metadata:s:0", format!("0:s:{}", source.stream));
    }
    Ok(EncoderJob {
        command: command
            .option("-output_ts_offset", ENCODER_OFFSET_SECONDS.to_string())
            .option("-f", "nut")
            .output_stdout(),
        preroll,
        samples,
        offset: ENCODER_OFFSET_SECONDS * rate,
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_default_target_is_high_bitrate_aac() {
        assert_eq!(AudioTarget::default(), AudioTarget::Aac { kilobits: 256 });
    }

    #[test]
    fn a_lossy_codec_is_primed_with_whole_frames_and_a_lossless_one_is_not() {
        let encoding = |codec: &str| AudioEncoding {
            codec: codec.to_owned(),
            encoder: codec.to_owned(),
            kilobits: None,
            sample_rate: 48_000,
            channels: 2,
            matches_copied: false,
            lossless: false,
        };
        assert_eq!(encoding("aac").frame(), 1024);
        assert_eq!(encoding("flac").frame(), 0);
        assert_eq!(encoding("pcm_s16le").frame(), 0);
    }
}
