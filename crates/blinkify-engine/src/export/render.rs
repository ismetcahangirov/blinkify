//! The full re-encode executor (#55): a segment the plan cannot copy —
//! a held frame, a reversed clip, a clip in another shape or at a speed no
//! file carries, black in a gap — rendered at the sequence's shape and frame
//! rate and encoded to join the rest of the output.
//!
//! This path stays rare, visible and slow by admission: nothing reaches it
//! that the plan did not route here with a recorded reason, and the export
//! dialog (#50) and report (#52) say so.
//!
//! - **One parameter derivation.** The encoder and its arguments are the
//!   plan's choice for the output's reference source (#44), so a rendered
//!   segment matches the copied ones around it, and the join is checked on
//!   the SPS before a packet of it is written, as a seam's is.
//! - **The sequence's shape.** Pictures are scaled to fit the sequence and
//!   padded with black, and put on the sequence's frame grid, so a segment is
//!   exactly its planned number of frames.
//! - **A hold** is one decoded frame, repeated for the held length.
//! - **A reverse** never holds a clip in memory: the clip is decoded a chunk
//!   of [`REVERSE_CHUNK_FRAMES`] at a time from its last chunk to its first,
//!   each chunk reversed on its own, and the frames fed in order to one
//!   encoder over a pipe. The most held at once is one chunk.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::execute::{ExportError, ExportInput};
use super::plan::{ExportPlan, Segment, SegmentSource};
use super::profile::EncoderChoice;
use crate::capability::EncoderSource;
use crate::orchestrator::SidecarCommand;
use crate::probe::{Rational, StreamKind};
use crate::project::SourceId;
use crate::project::evaluate::Motion;

/// Seconds added to every timestamp an encoder writes, as for readers.
pub const RENDER_OFFSET_SECONDS: i64 = 100;

/// How far before a range a decoder seeks.
const SEEK_MARGIN_SECONDS: f64 = 3.0;

/// Frames of a reversed clip held at once: the decoder's `reverse` filter
/// buffers one chunk, and nothing else holds more than a pipe's worth. At
/// 4K 4:2:0 8-bit that is 32 × 12 MB, about 400 MB, however long the clip.
pub const REVERSE_CHUNK_FRAMES: i64 = 32;

/// What rendering a segment runs.
#[derive(Debug)]
pub enum RenderJob {
    /// One process decodes, filters and encodes.
    Direct(SidecarCommand),
    /// Decoders, one chunk each, last chunk first, write raw frames that are
    /// fed in order to one encoder reading its standard input.
    Reverse {
        decoders: Vec<SidecarCommand>,
        encoder: SidecarCommand,
    },
}

fn rate_text(rate: Rational) -> String {
    format!("{}/{}", rate.num, rate.den)
}

/// Scale to fit the sequence, pad the rest black, square the pixels as the
/// sequence has them, and convert to the encoder's pixel format.
fn fit(plan: &ExportPlan, pixel_format: &str) -> String {
    let settings = &plan.sequence;
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar={sn}/{sd},format={pixel_format}",
        w = settings.width,
        h = settings.height,
        sn = settings.pixel_aspect.num,
        sd = settings.pixel_aspect.den,
    )
}

/// Exactly `frames` frames on the sequence's grid of `rate`: short input is
/// padded by repeating its last frame, long input is cut, and every frame is
/// stamped `n / rate` from the segment's start.
fn exact(frames: i64, rate: Rational) -> String {
    format!(
        "tpad=stop_mode=clone:stop=-1,trim=end_frame={frames},setpts=N*{}/{}/TB",
        rate.den, rate.num
    )
}

/// The encoder's options, the plan's choice for the output's reference.
fn encode(mut command: SidecarCommand, choice: &EncoderChoice) -> SidecarCommand {
    for (name, value) in choice.arguments(choice.codec) {
        command = command.option(name, value);
    }
    if choice.source != EncoderSource::Software {
        command = command.option("-bf", "0");
    }
    command
        .option("-output_ts_offset", RENDER_OFFSET_SECONDS.to_string())
        .option("-f", "nut")
        .output_stdout()
}

fn path_of<'a>(
    inputs: &'a BTreeMap<SourceId, ExportInput>,
    source: &SegmentSource,
) -> Result<&'a Path, ExportError> {
    inputs
        .get(&source.source)
        .map(|input| input.source.path())
        .ok_or(ExportError::MissingSource(source.source))
}

/// The source's nominal frame rate, from its probe.
fn source_rate(inputs: &BTreeMap<SourceId, ExportInput>, source: &SegmentSource) -> Rational {
    inputs
        .get(&source.source)
        .and_then(|input| {
            input
                .info
                .streams
                .iter()
                .find(|stream| stream.index == source.stream)
        })
        .and_then(|stream| match &stream.kind {
            StreamKind::Video(video) => video.frame_rate.average.or(video.frame_rate.real_base),
            _ => None,
        })
        .filter(|rate| rate.num > 0 && rate.den > 0)
        .unwrap_or(Rational { num: 30, den: 1 })
}

/// A decoder of `source` from just before `from`.
fn decoder(path: &Path, source: &SegmentSource, from: i64) -> SidecarCommand {
    let seek = crate::time::seconds(from, source.time_base) - SEEK_MARGIN_SECONDS;
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-copyts");
    if seek > 0.0 {
        command = command.option("-ss", format!("{seek:.6}"));
    }
    command
        .input(path)
        .option("-map", format!("0:{}", source.stream))
}

/// What renders `segment` with `choice`.
///
/// # Errors
///
/// A source is missing.
pub fn render_job(
    segment: &Segment,
    plan: &ExportPlan,
    inputs: &BTreeMap<SourceId, ExportInput>,
    choice: &EncoderChoice,
) -> Result<RenderJob, ExportError> {
    let rate = plan.sequence.frame_rate;
    let frames = segment.length;
    let fit = fit(plan, &choice.pixel_format);
    let Some(source) = segment.sources.first() else {
        // A gap: black at the sequence's shape and rate.
        let settings = &plan.sequence;
        let command = SidecarCommand::ffmpeg()
            .option("-v", "error")
            .lavfi_input(&format!(
                "color=c=black:s={}x{}:r={}",
                settings.width,
                settings.height,
                rate_text(rate)
            ))
            .option("-vf", format!("{fit},{}", exact(frames, rate)));
        return Ok(RenderJob::Direct(encode(command, choice)));
    };
    let path = path_of(inputs, source)?;
    let (in_, out) = (source.source_in, source.source_out);
    // Played at `speed`: timestamps divided by it, then onto the grid.
    let slower = format!("setpts=PTS*{}/{}", source.speed.den, source.speed.num);
    match source.motion {
        Some(Motion::Hold) => {
            let mut graph = format!("trim=start_pts={in_},setpts=PTS-STARTPTS,select=eq(n\\,0),");
            let _ = write!(
                graph,
                "loop=loop={}:size=1:start=0,{fit},{}",
                frames.saturating_sub(1).max(0),
                exact(frames, rate)
            );
            let command = decoder(path, source, in_).option("-vf", graph);
            Ok(RenderJob::Direct(encode(command, choice)))
        }
        Some(Motion::Reverse) => {
            let source_rate = source_rate(inputs, source);
            // A chunk in source ticks: REVERSE_CHUNK_FRAMES at the source's rate.
            let chunk = crate::time::rescale(
                REVERSE_CHUNK_FRAMES,
                Rational {
                    num: source_rate.den,
                    den: source_rate.num,
                },
                source.time_base,
                crate::time::Rounding::Down,
            )
            .filter(|ticks| *ticks > 0)
            .unwrap_or(out - in_);
            let mut decoders = Vec::new();
            let mut end = out;
            while end > in_ {
                let start = (end - chunk).max(in_);
                decoders.push(
                    decoder(path, source, start)
                        .option(
                            "-vf",
                            format!("trim=start_pts={start}:end_pts={end},reverse,{fit}"),
                        )
                        .option("-f", "rawvideo")
                        .output_stdout(),
                );
                end = start;
            }
            // The encoder reads the reversed frames at the source's rate,
            // played at the clip's speed, and puts them on the grid.
            let played = Rational {
                num: source_rate.num.saturating_mul(source.speed.num),
                den: source_rate.den.saturating_mul(source.speed.den),
            };
            let settings = &plan.sequence;
            let encoder = SidecarCommand::ffmpeg()
                .option("-v", "error")
                .option("-f", "rawvideo")
                .option("-pix_fmt", choice.pixel_format.clone())
                .option("-s", format!("{}x{}", settings.width, settings.height))
                .option("-r", rate_text(played))
                .stdin_input()
                .option(
                    "-vf",
                    format!("fps={},{}", rate_text(rate), exact(frames, rate)),
                );
            Ok(RenderJob::Reverse {
                decoders,
                encoder: encode(encoder, choice),
            })
        }
        None => {
            let graph = format!(
                "trim=start_pts={in_}:end_pts={out},setpts=PTS-STARTPTS,{slower},fps={},{fit},{}",
                rate_text(rate),
                exact(frames, rate)
            );
            let command = decoder(path, source, in_).option("-vf", graph);
            Ok(RenderJob::Direct(encode(command, choice)))
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn exactly_the_planned_frames_whatever_the_input_gives() {
        assert_eq!(
            exact(
                90,
                Rational {
                    num: 30_000,
                    den: 1001
                }
            ),
            "tpad=stop_mode=clone:stop=-1,trim=end_frame=90,setpts=N*1001/30000/TB"
        );
    }
}
