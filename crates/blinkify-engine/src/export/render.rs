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
//! - **A crop** (#128) is the clip's rectangle cut out of the decoded picture
//!   before anything is scaled — scaling first would throw away resolution
//!   the crop keeps — by the filter the preview uses too
//!   ([`crate::picture_filter`], ADR-0021). It is one filter in the one
//!   render pass: no cropped file is ever written.
//! - **Orientation.** A source is decoded as coded, without autorotation, as
//!   the preview decodes it, and turned only where its orientation differs
//!   from the output's: the output stream carries one display rotation
//!   ([`output_rotation`]), the copied packets' — which cannot be turned — so
//!   a rendered picture is encoded in the same coded orientation and shown
//!   upright by the same display matrix. A portrait phone clip rendered
//!   between copies of itself is never transposed at all.
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
use crate::picture_filter::{self, Crop, Decoded};
use crate::probe::{Rational, StreamKind, VideoInfo};
use crate::project::SourceId;
use crate::project::evaluate::Motion;
use crate::tier::ExportTier;

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

/// The display rotation the output's video stream carries, counter-clockwise
/// degrees: that of the source of the first video segment whose packets are
/// copied — a copy or a smart-cut — because a copied packet cannot be turned.
/// Where every picture is rendered there is nothing to keep, and the output
/// is encoded upright. The muxer writes it; every rendered picture is encoded
/// in the orientation it implies.
pub(crate) fn output_rotation(plan: &ExportPlan, inputs: &BTreeMap<SourceId, ExportInput>) -> u32 {
    plan.segments
        .iter()
        .filter(|segment| segment.media == super::plan::Media::Video)
        .filter(|segment| !matches!(segment.tier, ExportTier::FullReEncode { .. }))
        .find_map(|segment| segment.sources.first())
        .and_then(|source| video_of(inputs, source))
        .map_or(0, |video| video.rotation)
}

/// The pictures of the output as they are coded: the sequence's shape,
/// turned back by the output's rotation, with the pixel aspect turned too.
#[derive(Debug, Clone, Copy)]
struct Coded {
    width: u32,
    height: u32,
    pixel_aspect: Rational,
}

impl Coded {
    fn of(plan: &ExportPlan, rotation: u32) -> Self {
        let settings = &plan.sequence;
        if picture_filter::quarter_turns(rotation) % 2 == 1 {
            Self {
                width: settings.height,
                height: settings.width,
                pixel_aspect: Rational {
                    num: settings.pixel_aspect.den,
                    den: settings.pixel_aspect.num,
                },
            }
        } else {
            Self {
                width: settings.width,
                height: settings.height,
                pixel_aspect: settings.pixel_aspect,
            }
        }
    }

    fn size(self) -> String {
        format!("{}x{}", self.width, self.height)
    }
}

/// Scale to fit the output's coded picture, pad the rest black, square the
/// pixels as the sequence has them, and convert to the encoder's pixel
/// format.
fn fit(coded: Coded, pixel_format: &str) -> String {
    format!(
        "scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar={sn}/{sd},format={pixel_format}",
        w = coded.width,
        h = coded.height,
        sn = coded.pixel_aspect.num,
        sd = coded.pixel_aspect.den,
    )
}

/// The source's probed video stream.
fn video_of<'a>(
    inputs: &'a BTreeMap<SourceId, ExportInput>,
    source: &SegmentSource,
) -> Option<&'a VideoInfo> {
    inputs
        .get(&source.source)?
        .info
        .streams
        .iter()
        .find(|stream| stream.index == source.stream)
        .and_then(|stream| match &stream.kind {
            StreamKind::Video(video) => Some(video.as_ref()),
            _ => None,
        })
}

/// What turns `source`'s decoded pictures into the output's: its crop, cut
/// from the picture as coded, then the quarter turns from its orientation to
/// the output's. Empty, or ending in a comma to be followed.
fn picture(
    source: &SegmentSource,
    inputs: &BTreeMap<SourceId, ExportInput>,
    output_rotation: u32,
) -> String {
    let Some(video) = video_of(inputs, source) else {
        return String::new();
    };
    let decoded = Decoded::coded(video);
    let mut chain = String::new();
    if let Some(rect) = source.crop {
        let crop = Crop {
            rect,
            picture: decoded,
        };
        let _ = write!(chain, "{},", crop.filter());
    }
    let turns = (decoded.quarter_turns() + 4 - picture_filter::quarter_turns(output_rotation)) % 4;
    if let Some(turn) = picture_filter::turn(turns) {
        let _ = write!(chain, "{turn},");
    }
    chain
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

/// The one frame a hold at `in_` shows: the newest at or before it, as the
/// evaluator and the preview name it (#134, ADR-0023), not the first after.
///
/// No filter looks ahead, so `fps` does it: every frame at or before `in_`
/// is stamped 0 and every later one a second, and `fps` emits the frame it
/// holds for 0 only when a later one arrives, which is the last at or
/// before. The clone `tpad` adds after the last frame is that later one when
/// the hold is on the file's last frame. `fps` runs at the sequence's
/// `rate`, so the frame leaves on the sequence's time base.
fn held_frame(in_: i64, rate: Rational) -> String {
    format!(
        "tpad=stop=1:stop_mode=clone,setpts=if(lte(PTS\\,{in_})\\,0\\,1/TB),\
         fps=fps={}:start_time=0,select=eq(n\\,0)",
        rate_text(rate)
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
    video_of(inputs, source)
        .and_then(|video| video.frame_rate.average.or(video.frame_rate.real_base))
        .filter(|rate| rate.num > 0 && rate.den > 0)
        .unwrap_or(Rational { num: 30, den: 1 })
}

/// A decoder of `source` from just before `from`, delivering its pictures as
/// coded: turning them is [`picture`]'s, once, with the crop.
fn decoder(path: &Path, source: &SegmentSource, from: i64) -> SidecarCommand {
    let seek = crate::time::seconds(from, source.time_base) - SEEK_MARGIN_SECONDS;
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .flag("-copyts")
        .flag("-noautorotate");
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
    let rotation = output_rotation(plan, inputs);
    let coded = Coded::of(plan, rotation);
    let fit = fit(coded, &choice.pixel_format);
    let Some(source) = segment.sources.first() else {
        // A gap: black at the sequence's shape and rate.
        let command = SidecarCommand::ffmpeg()
            .option("-v", "error")
            .lavfi_input(&format!(
                "color=c=black:s={}:r={}",
                coded.size(),
                rate_text(rate)
            ))
            .option("-vf", format!("{fit},{}", exact(frames, rate)));
        return Ok(RenderJob::Direct(encode(command, choice)));
    };
    let path = path_of(inputs, source)?;
    let picture = picture(source, inputs, rotation);
    let (in_, out) = (source.source_in, source.source_out);
    // Played at `speed`: timestamps divided by it, then onto the grid.
    let slower = format!("setpts=PTS*{}/{}", source.speed.den, source.speed.num);
    match source.motion {
        Some(Motion::Hold) => {
            let mut graph = format!("{},{picture}", held_frame(in_, rate));
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
                            format!("trim=start_pts={start}:end_pts={end},{picture}reverse,{fit}"),
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
            let encoder = SidecarCommand::ffmpeg()
                .option("-v", "error")
                .option("-f", "rawvideo")
                .option("-pix_fmt", choice.pixel_format.clone())
                .option("-s", coded.size())
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
                "trim=start_pts={in_}:end_pts={out},setpts=PTS-STARTPTS,{picture}{slower},fps={},{fit},{}",
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
