//! The on-disk peak format.
//!
//! ```text
//! offset  size  field
//!      0     8  magic "BLKPEAKS"
//!      8     2  format version (u16, little-endian)
//!     10     2  channels
//!     12     4  sample rate
//!     16     4  samples per bucket at the base level
//!     20     8  total samples per channel
//!     28     2  level count
//!     30  12×n  per level: samples per bucket (u32), bucket count (u64)
//!      …        per level, in order: bucket × channel × (min, max), i16
//! ```
//!
//! Versioned in the header: a file written by another version is rejected, and
//! the peaks are regenerated rather than misread. Every length read from the
//! file is checked against the bytes actually present before it is used — a
//! cache file is input like any other.

use super::{Level, Peaks};

const MAGIC: &[u8; 8] = b"BLKPEAKS";

/// Bump when the layout changes.
pub const VERSION: u16 = 1;

const HEADER: usize = 30;
const LEVEL_HEADER: usize = 12;

pub(super) fn encode(peaks: &Peaks) -> Vec<u8> {
    let data_len: usize = peaks.levels.iter().map(|level| level.data.len() * 2).sum();
    let mut out = Vec::with_capacity(HEADER + LEVEL_HEADER * peaks.levels.len() + data_len);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&peaks.channels.to_le_bytes());
    out.extend_from_slice(&peaks.sample_rate.to_le_bytes());
    out.extend_from_slice(&peaks.base_samples_per_bucket.to_le_bytes());
    out.extend_from_slice(&peaks.total_samples.to_le_bytes());
    out.extend_from_slice(
        &u16::try_from(peaks.levels.len())
            .unwrap_or(u16::MAX)
            .to_le_bytes(),
    );
    for level in &peaks.levels {
        out.extend_from_slice(&level.samples_per_bucket.to_le_bytes());
        out.extend_from_slice(&level.buckets.to_le_bytes());
    }
    for level in &peaks.levels {
        for value in &level.data {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// `None` for anything that is not a complete peak file of this version.
pub(super) fn decode(bytes: &[u8]) -> Option<Peaks> {
    if bytes.get(0..8)? != MAGIC {
        return None;
    }
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let u64_at = |at: usize| Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));

    if u16_at(8)? != VERSION {
        return None;
    }
    let channels = u16_at(10)?;
    let sample_rate = u32_at(12)?;
    let base_samples_per_bucket = u32_at(16)?;
    let total_samples = u64_at(20)?;
    let level_count = usize::from(u16_at(28)?);
    if channels == 0 {
        return None;
    }

    let mut offset = HEADER.checked_add(LEVEL_HEADER.checked_mul(level_count)?)?;
    let mut levels = Vec::with_capacity(level_count);
    for n in 0..level_count {
        let at = HEADER + LEVEL_HEADER * n;
        let samples_per_bucket = u32_at(at)?;
        let buckets = u64_at(at + 4)?;
        let values = usize::try_from(buckets)
            .ok()?
            .checked_mul(usize::from(channels))?
            .checked_mul(2)?;
        let end = offset.checked_add(values.checked_mul(2)?)?;
        let raw = bytes.get(offset..end)?;
        let data = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        levels.push(Level {
            samples_per_bucket,
            buckets,
            data,
        });
        offset = end;
    }
    // Trailing bytes mean the header lied about the levels.
    (offset == bytes.len()).then_some(Peaks {
        channels,
        sample_rate,
        base_samples_per_bucket,
        total_samples,
        levels,
    })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::expect_used)]
mod tests {
    use super::*;

    fn sample() -> Peaks {
        Peaks {
            channels: 2,
            sample_rate: 48_000,
            base_samples_per_bucket: 256,
            total_samples: 1024,
            levels: vec![
                Level {
                    samples_per_bucket: 256,
                    buckets: 2,
                    data: vec![-1, 1, -2, 2, -3, 3, -4, 4],
                },
                Level {
                    samples_per_bucket: 512,
                    buckets: 1,
                    data: vec![-3, 3, -4, 4],
                },
            ],
        }
    }

    #[test]
    fn peaks_round_trip() {
        let peaks = sample();
        assert_eq!(decode(&encode(&peaks)), Some(peaks));
    }

    #[test]
    fn another_version_is_rejected_not_misread() {
        let mut bytes = encode(&sample());
        bytes[8..10].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(decode(&bytes), None);
    }

    #[test]
    fn a_truncated_or_padded_file_is_rejected() {
        let bytes = encode(&sample());
        assert_eq!(decode(&bytes[..bytes.len() - 1]), None);
        let mut padded = bytes.clone();
        padded.push(0);
        assert_eq!(decode(&padded), None);
        assert_eq!(decode(b"BLKPEAKS"), None);
        assert_eq!(decode(b""), None);
    }

    #[test]
    fn a_bucket_count_larger_than_the_file_is_not_believed() {
        let mut bytes = encode(&sample());
        // Level 0's bucket count, claimed to be astronomical.
        bytes[HEADER + 4..HEADER + 12].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode(&bytes), None);
    }
}
