//! The two-stage seek: to the keyframe at or before a frame, then forward to
//! exactly that frame.
//!
//! Seeking to a timestamp that is not a keyframe and showing what the decoder
//! produces first yields the wrong frame. Most players hide that; an editor
//! cannot, because the frame on screen is the frame a cut will be made at.
//! So a seek is planned here, from the keyframe index — never a linear scan —
//! and executed by decoding from the keyframe and discarding, inside FFmpeg,
//! every frame before the target.
//!
//! This is shared, not a player helper. The preview uses it for seeking and
//! scrubbing (#29); the smart-cut port (#41) uses the same plan to find the
//! window a cut depends on — the keyframe it must decode from — rather than
//! working it out a second time.
//!
//! Every position here is a timestamp from the index, in the stream's time
//! base. A variable frame rate makes "frame number times frame duration"
//! wrong, and an edit list makes the container's own timestamps start
//! somewhere other than zero; the index already accounts for both.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::decode::{
    DecodeEnd, DecodeError, DecodeRequest, FrameRing, FrameSize, VideoDecoder, VideoFrame,
};
use crate::keyframes::{IndexError, Keyframe, KeyframeIndex};
use crate::orchestrator::Orchestrator;
use crate::probe::Rational;

/// How long a single-frame decode may take before it is abandoned.
const FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// How to reach one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPlan {
    /// The frame on screen at the requested position: the newest frame whose
    /// timestamp is at or before it, or the stream's first frame for a
    /// position before that.
    pub frame: i64,
    /// The keyframe decoding must start at. Every frame from it up to
    /// `frame` is decoded; the ones before `frame` are discarded. This is the
    /// window a cut at `frame` depends on.
    pub keyframe: Keyframe,
    /// Whether decoding can simply start at the beginning of the stream —
    /// the keyframe is the stream's first — which needs no seek at all.
    pub from_start: bool,
}

impl SeekPlan {
    /// Whether the frame is itself a keyframe: the seek is one decode.
    #[must_use]
    pub fn on_keyframe(&self) -> bool {
        self.keyframe.pts == self.frame
    }

    /// The decode that delivers exactly [`SeekPlan::frame`] first, then up to
    /// `max_frames` frames in all.
    #[must_use]
    pub fn request(
        &self,
        source: &Path,
        stream: u32,
        time_base: Rational,
        size: FrameSize,
        max_frames: Option<u32>,
    ) -> DecodeRequest {
        DecodeRequest {
            source: source.to_path_buf(),
            stream,
            time_base,
            seek_to: (!self.from_start).then_some(self.keyframe.pts),
            first_pts: self.frame,
            size,
            max_frames,
            concat: false,
        }
    }
}

/// Why a seek could not be made.
#[derive(Debug, Error)]
pub enum SeekError {
    #[error("the stream has no frames to seek to")]
    NoFrames,
    #[error("the keyframe index could not answer: {0}")]
    Index(#[from] IndexError),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("the frame did not decode in time")]
    TimedOut,
}

/// Plan the seek to the frame on screen at `pts` of `stream`.
///
/// # Errors
///
/// The stream has no frames, or the index could not read the region.
pub fn plan(index: &KeyframeIndex, stream: u32, pts: i64) -> Result<SeekPlan, SeekError> {
    let frame = match index.frame_at_or_before(stream, pts)? {
        Some(frame) => frame,
        None => index.frame_after(stream, pts)?.ok_or(SeekError::NoFrames)?,
    };
    // For a leading picture of an open GOP — shown before its keyframe,
    // decoded after it — the keyframe at or before its timestamp is the
    // *previous* one, which is exactly the one its references need.
    let keyframe = index
        .at_or_before(stream, frame)?
        .ok_or(SeekError::NoFrames)?;
    let from_start = index
        .at_or_before(stream, keyframe.pts.saturating_sub(1))?
        .is_none();
    Ok(SeekPlan {
        frame,
        keyframe,
        from_start,
    })
}

/// Decode exactly the frame on screen at `pts`, at `size`. One process, one
/// frame: for a caller that wants a single picture — a poster frame, a
/// check of a cut — rather than a playing stream.
///
/// # Errors
///
/// As [`plan`], or the decode failed or took longer than 30 seconds.
pub fn decode_frame(
    orchestrator: &Orchestrator,
    index: &KeyframeIndex,
    source: &Path,
    stream: u32,
    pts: i64,
    size: FrameSize,
) -> Result<VideoFrame, SeekError> {
    let plan = plan(index, stream, pts)?;
    let time_base = index.time_base(stream)?;
    let ring = Arc::new(FrameRing::new(1));
    let decoder = VideoDecoder::start(
        orchestrator,
        &plan.request(source, stream, time_base, size, Some(1)),
        Arc::clone(&ring),
    );
    ring.wait_for_frame(FRAME_TIMEOUT);
    let frame = ring.pop();
    let end = decoder.end();
    decoder.stop();
    match (frame, end) {
        (Some(frame), _) => Ok(frame),
        (None, Some(DecodeEnd::Failed(error))) => Err(error.into()),
        _ => Err(SeekError::TimedOut),
    }
}
