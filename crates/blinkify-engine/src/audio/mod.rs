//! Preview audio: decoding sources to the output rate, the buffer the
//! playback clock is read from, and the sinks that play it (#28).
//!
//! Everything here is interleaved stereo `f32`. A source with more channels is
//! downmixed by FFmpeg as it is decoded; a device with more channels gets the
//! stereo pair on its first two. This path is for monitoring: nothing in it
//! reaches an export — except [`chain`], the one audio filter chain, which
//! the export runs too so that what is heard is what is written (Epic #7).

mod buffer;
pub mod chain;
pub mod decoder;
pub mod denoise;
pub mod gain;
pub mod loudness;
mod meter;
pub mod sink;

pub use buffer::OutputBuffer;
pub use decoder::{AudioDecoder, AudioEnd, AudioRequest, SampleRing};
pub use meter::MonitorLevels;
pub use sink::{AudioOutputState, BufferSlot, Sink};

/// Interleaved channels in every buffer on this path.
pub const CHANNELS: usize = 2;
