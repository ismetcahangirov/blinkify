//! What the planner needs to know about a source, read from its probe and
//! its keyframe index (#23, #24). The planner itself reads no file; this is
//! the one place its facts are gathered, so the export dialog and the
//! executor plan from the same description.

use std::collections::BTreeMap;

use super::plan::{AudioFacts, EncodingSignature, KeyframePoint, SourceFacts, VideoFacts};
use super::profile::{Unmatched, select, source_profile};
use crate::capability::EncoderCapabilities;
use crate::keyframes::{KeyframeIndex, PictureKind};
use crate::probe::{MediaInfo, Rational, StreamInfo, StreamKind};
use crate::project::settings::StreamGeometry;

/// The default of `streams`, or the first.
fn pick<'a, T>(streams: impl Iterator<Item = (&'a StreamInfo, T)>) -> Option<(&'a StreamInfo, T)> {
    let streams: Vec<_> = streams.collect();
    let default = streams.iter().position(|(stream, _)| stream.is_default);
    streams.into_iter().nth(default.unwrap_or(0))
}

/// The first tick past a stream's end, as the trim bounds count it: its
/// start plus its duration, rounded down (see `StreamExtent`).
fn end_of(stream: &StreamInfo, info: &MediaInfo, time_base: Rational) -> Option<i64> {
    let duration = stream
        .duration_seconds
        .or(info.container.duration_seconds)
        .filter(|d| d.is_finite() && *d > 0.0)?;
    let begin = stream
        .start_seconds
        .filter(|s| s.is_finite())
        .unwrap_or(0.0);
    #[allow(clippy::cast_precision_loss)]
    let per_second = time_base.den as f64 / time_base.num as f64;
    let end = ((begin + duration) * per_second).floor();
    // Saturating: `as` from a float clamps, and the value was checked finite.
    #[allow(clippy::cast_possible_truncation)]
    let end = end as i64;
    Some(end)
}

fn valid(time_base: Option<Rational>) -> Option<Rational> {
    time_base.filter(|tb| tb.num > 0 && tb.den > 0)
}

/// The planner's facts about the file `info` describes. `index` is its
/// keyframe index, as far as it has got; without one no cut can be proved
/// to be on a keyframe, and the planner says so rather than guessing.
/// `encoders` is this machine's capability profile (#21), `None` while it is
/// still being probed.
#[must_use]
pub fn source_facts(
    info: &MediaInfo,
    index: Option<&KeyframeIndex>,
    encoders: Option<&EncoderCapabilities>,
) -> SourceFacts {
    let video =
        pick(info.video().filter(|(_, v)| !v.is_attached_picture)).and_then(|(stream, video)| {
            let time_base = valid(stream.time_base)?;
            let geometry = StreamGeometry::of(video)?;
            let keyframes = index
                .and_then(|index| index.keyframes(stream.index).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|keyframe| KeyframePoint {
                    pts: keyframe.pts,
                    open: keyframe.has_leading_pictures,
                    random_access: keyframe.picture != Some(PictureKind::RecoveryPoint),
                })
                .collect();
            Some(VideoFacts {
                stream: stream.index,
                time_base,
                geometry,
                encoding: EncodingSignature {
                    codec: stream.codec.clone(),
                    profile: stream.profile.clone(),
                    level: stream.level,
                    pixel_format: video.pixel_format.clone(),
                    width: video.width,
                    height: video.height,
                    colour: [
                        video.color.primaries.clone(),
                        video.color.transfer.clone(),
                        video.color.matrix.clone(),
                        video.color.range.clone(),
                    ],
                    sample_rate: None,
                    channels: None,
                    configuration: stream.extradata_hash.clone(),
                },
                keyframes,
                keyframes_complete: index.is_some_and(KeyframeIndex::is_complete),
                end: end_of(stream, info, time_base),
                reorders: video.has_b_frames,
                encoder: source_profile(stream).and_then(|profile| {
                    encoders
                        .ok_or(Unmatched::EncodersUnknown)
                        .and_then(|encoders| select(&profile, encoders))
                }),
            })
        });
    let audio: BTreeMap<u32, AudioFacts> = info
        .streams
        .iter()
        .filter_map(|stream| {
            let StreamKind::Audio(audio) = &stream.kind else {
                return None;
            };
            Some((
                stream.index,
                AudioFacts {
                    stream: stream.index,
                    time_base: valid(stream.time_base)?,
                    encoding: EncodingSignature {
                        codec: stream.codec.clone(),
                        profile: stream.profile.clone(),
                        level: None,
                        pixel_format: audio.sample_format.clone(),
                        width: 0,
                        height: 0,
                        colour: [None, None, None, None],
                        sample_rate: audio.sample_rate,
                        channels: audio.channels,
                        configuration: stream.extradata_hash.clone(),
                    },
                },
            ))
        })
        .collect();
    let default_audio = pick(info.audio())
        .map(|(stream, _)| stream.index)
        .filter(|index| audio.contains_key(index));
    SourceFacts {
        video,
        audio,
        default_audio,
    }
}
