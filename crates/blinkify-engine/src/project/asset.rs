//! What the media library shows about a source (#53): read from the probe
//! once, at import or open, and kept beside the graph — never in the project
//! file, which holds only the reference (#32).
//!
//! The values that matter in a bug report — resolution, codec, profile, bit
//! depth, frame rate, whether it is variable — are the probe's, verbatim.

use serde::Serialize;
use ts_rs::TS;

use super::settings::{SequenceSettings, StreamGeometry};
use crate::probe::{FrameRateMode, MediaInfo, Rational};

/// A source as the library card and the inspector (#56) show it.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssetInfo {
    /// The container's duration, in seconds, where it states one.
    pub duration_seconds: Option<f64>,
    /// The first video stream that is not a cover image.
    pub video: Option<VideoSummary>,
    /// The first audio stream.
    pub audio: Option<AudioSummary>,
    /// The sequence settings that would copy this source exactly — what
    /// adopting its settings means (#57). `None` without pictures, or with a
    /// shape no file can carry.
    pub matching: Option<SequenceSettings>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VideoSummary {
    pub stream: u32,
    pub codec: Option<String>,
    pub profile: Option<String>,
    /// Coded size.
    pub width: u32,
    pub height: u32,
    /// As displayed, rotation applied.
    pub display_width: u32,
    pub display_height: u32,
    pub rotation: u32,
    pub bit_depth: Option<u8>,
    pub frame_rate: Option<Rational>,
    pub variable_frame_rate: bool,
    pub pixel_format: Option<String>,
    pub colour_primaries: Option<String>,
    pub colour_transfer: Option<String>,
    pub colour_matrix: Option<String>,
    pub colour_range: Option<String>,
    pub hdr: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AudioSummary {
    pub stream: u32,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

impl AssetInfo {
    /// Summarise a probe. `None` when the file has neither pictures nor
    /// sound Blinkify can use — what an import refuses, with that reason.
    #[must_use]
    pub fn of(info: &MediaInfo) -> Option<Self> {
        let video = info
            .video()
            .find(|(_, video)| !video.is_attached_picture)
            .map(|(stream, video)| VideoSummary {
                stream: stream.index,
                codec: stream.codec.clone(),
                profile: stream.profile.clone(),
                width: video.width,
                height: video.height,
                display_width: video.display_width,
                display_height: video.display_height,
                rotation: video.rotation,
                bit_depth: video.bit_depth,
                frame_rate: video.frame_rate.average.or(video.frame_rate.real_base),
                variable_frame_rate: video.frame_rate.mode == FrameRateMode::Variable,
                pixel_format: video.pixel_format.clone(),
                colour_primaries: video.color.primaries.clone(),
                colour_transfer: video.color.transfer.clone(),
                colour_matrix: video.color.matrix.clone(),
                colour_range: video.color.range.clone(),
                hdr: video.hdr.is_some(),
            });
        let audio = info.audio().next().map(|(stream, audio)| AudioSummary {
            stream: stream.index,
            codec: stream.codec.clone(),
            sample_rate: audio.sample_rate,
            channels: audio.channels,
        });
        if video.is_none() && audio.is_none() {
            return None;
        }
        let matching = info
            .video()
            .find(|(_, video)| !video.is_attached_picture)
            .and_then(|(_, video)| StreamGeometry::of(video))
            .and_then(|geometry| SequenceSettings::matching(&geometry).ok());
        Some(Self {
            duration_seconds: info.container.duration_seconds,
            video,
            audio,
            matching,
        })
    }
}
