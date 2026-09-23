//! Finding MP4 and QuickTime edit lists.
//!
//! An edit list (`moov/trak/edts/elst`) maps the track's media timeline onto
//! the presentation timeline. The common one hides encoder pre-roll: a stream
//! copied from the middle of a GOP starts at the previous keyframe, and the
//! edit list says "skip the first 0.3 s". Ignoring it produces an A/V offset
//! that shows up in some players and not others, which is the worst kind.
//!
//! `ffprobe` applies edit lists but does not report them, so the boxes are read
//! here. Every media file is hostile input (`CLAUDE.md` section 11): each box
//! size is checked against what is actually left in its parent before it is
//! trusted, arithmetic is checked, nothing is indexed blindly, and the walk is
//! bounded in depth and in the number of boxes it will look at.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Boxes examined before the parser gives up: a real file has a few dozen at
/// the levels walked here, a hostile one can claim millions.
const MAX_BOXES: usize = 10_000;
/// The largest `moov` read into memory. Real ones are kilobytes to a few
/// megabytes; one claiming gigabytes is not a real one.
const MAX_MOOV: u64 = 64 * 1024 * 1024;

/// The edit list of one track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditList {
    /// The track's ID from its `tkhd`.
    pub track_id: u32,
    pub entries: Vec<EditListEntry>,
}

impl EditList {
    /// Whether the edit list changes anything: more than one edit, an empty
    /// edit (a delay), or a start other than the beginning of the media.
    #[must_use]
    pub fn shifts_presentation(&self) -> bool {
        match self.entries.as_slice() {
            [only] => only.media_time != 0,
            _ => true,
        }
    }
}

/// One `elst` entry. Times are in the units the file uses: `segment_duration`
/// in the movie timescale, `media_time` in the track's media timescale, and
/// `-1` for an empty edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditListEntry {
    #[ts(type = "number")]
    pub segment_duration: u64,
    #[ts(type = "number")]
    pub media_time: i64,
}

/// Every edit list in an ISO-BMFF file, by track. Empty for a file with none,
/// and for any file that is not ISO-BMFF at all.
///
/// # Errors
///
/// Only an I/O error reading the file. Malformed boxes end the walk quietly:
/// the absence of a readable edit list is itself the answer.
pub fn read_edit_lists(path: &Path) -> std::io::Result<Vec<EditList>> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let Some(moov) = find_top_level_moov(&mut file, length)? else {
        return Ok(Vec::new());
    };
    let mut budget = MAX_BOXES;
    let mut lists = Vec::new();
    let traks = children(&moov, &mut budget);
    for trak in traks.iter().filter(|b| b.kind == *b"trak") {
        let mut track_id = 0;
        let mut entries = None;
        for child in children(trak.body, &mut budget) {
            match &child.kind {
                b"tkhd" => track_id = parse_tkhd_track_id(child.body).unwrap_or(0),
                b"edts" => {
                    entries = children(child.body, &mut budget)
                        .into_iter()
                        .find(|b| b.kind == *b"elst")
                        .and_then(|elst| parse_elst(elst.body));
                }
                _ => {}
            }
        }
        if let Some(entries) = entries {
            lists.push(EditList { track_id, entries });
        }
    }
    Ok(lists)
}

/// Walk the top level for `moov` without reading `mdat`, which is the bulk of
/// the file and may come first.
fn find_top_level_moov(file: &mut File, length: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut offset = 0_u64;
    for _ in 0..MAX_BOXES {
        let Some(header_room) = length.checked_sub(offset) else {
            break;
        };
        if header_room < 8 {
            break;
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; 16];
        let read = file.read(&mut header)?;
        let Some(parsed) = parse_header(header.get(..read).unwrap_or_default(), header_room) else {
            break;
        };
        if parsed.kind == *b"moov" {
            let body_len = parsed.size.saturating_sub(parsed.own_len);
            if body_len > MAX_MOOV {
                return Ok(None);
            }
            let mut body = vec![0_u8; usize::try_from(body_len).unwrap_or(0)];
            file.seek(SeekFrom::Start(offset + parsed.own_len))?;
            file.read_exact(&mut body)?;
            return Ok(Some(body));
        }
        let Some(next) = offset.checked_add(parsed.size) else {
            break;
        };
        offset = next;
    }
    Ok(None)
}

struct Header {
    kind: [u8; 4],
    size: u64,
    own_len: u64,
}

/// A box header. `room` is the bytes left in the parent; a size that claims
/// more than that, or less than the header itself, is rejected.
fn parse_header(bytes: &[u8], room: u64) -> Option<Header> {
    let size32 = u32::from_be_bytes(bytes.get(0..4)?.try_into().ok()?);
    let kind: [u8; 4] = bytes.get(4..8)?.try_into().ok()?;
    let (size, own_len) = match size32 {
        // Extends to the end of the parent.
        0 => (room, 8),
        // A 64-bit size follows the type.
        1 => (u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?), 16),
        n => (u64::from(n), 8),
    };
    (size >= own_len && size <= room).then_some(Header {
        kind,
        size,
        own_len,
    })
}

struct Child<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// The boxes directly inside `body`, stopping at the first malformed one or
/// when the shared budget runs out.
fn children<'a>(body: &'a [u8], budget: &mut usize) -> Vec<Child<'a>> {
    let mut rest = body;
    let mut allowed = *budget;
    let mut out = Vec::new();
    while allowed > 0 {
        let room = u64::try_from(rest.len()).unwrap_or(0);
        let Some(header) = parse_header(rest, room) else {
            break;
        };
        let (Ok(size), Ok(own_len)) = (
            usize::try_from(header.size),
            usize::try_from(header.own_len),
        ) else {
            break;
        };
        let (Some(inner), Some(after)) = (rest.get(own_len..size), rest.get(size..)) else {
            break;
        };
        out.push(Child {
            kind: header.kind,
            body: inner,
        });
        rest = after;
        allowed -= 1;
    }
    *budget = allowed;
    out
}

fn parse_tkhd_track_id(body: &[u8]) -> Option<u32> {
    // version(1) flags(3); v0: creation(4) modification(4) track_id(4)
    //                      v1: creation(8) modification(8) track_id(4)
    let offset = if *body.first()? == 1 { 20 } else { 12 };
    Some(u32::from_be_bytes(
        body.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn parse_elst(body: &[u8]) -> Option<Vec<EditListEntry>> {
    let version = *body.first()?;
    let count = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?);
    let entry_len: usize = if version == 1 { 20 } else { 12 };
    let entries = body.get(8..)?;
    // The count comes from the file. It is trusted only as far as the bytes
    // that are actually there.
    let available = entries.len().checked_div(entry_len)?;
    let count = usize::try_from(count).ok()?.min(available);
    entries
        .chunks_exact(entry_len)
        .take(count)
        .map(|entry| {
            if version == 1 {
                Some(EditListEntry {
                    segment_duration: u64::from_be_bytes(entry.get(0..8)?.try_into().ok()?),
                    media_time: i64::from_be_bytes(entry.get(8..16)?.try_into().ok()?),
                })
            } else {
                Some(EditListEntry {
                    segment_duration: u64::from(u32::from_be_bytes(
                        entry.get(0..4)?.try_into().ok()?,
                    )),
                    media_time: i64::from(i32::from_be_bytes(entry.get(4..8)?.try_into().ok()?)),
                })
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn boxed(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(&kind);
        out.extend_from_slice(body);
        out
    }

    fn elst(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut body = vec![0, 0, 0, 0];
        body.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (duration, media_time) in entries {
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&media_time.to_be_bytes());
            body.extend_from_slice(&[0, 1, 0, 0]);
        }
        boxed(*b"elst", &body)
    }

    fn tkhd(track_id: u32) -> Vec<u8> {
        let mut body = vec![0_u8; 12];
        body.extend_from_slice(&track_id.to_be_bytes());
        body.extend_from_slice(&[0_u8; 64]);
        boxed(*b"tkhd", &body)
    }

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("blinkify-elst-{name}"));
        std::fs::write(&path, bytes).expect("write");
        path
    }

    fn file_with(trak_children: &[Vec<u8>]) -> Vec<u8> {
        let trak = boxed(*b"trak", &trak_children.concat());
        let mut file = boxed(*b"ftyp", b"isom\0\0\0\0");
        file.extend(boxed(*b"mdat", &[0_u8; 32]));
        file.extend(boxed(*b"moov", &trak));
        file
    }

    #[test]
    fn an_edit_list_after_mdat_is_found() {
        let path = write(
            "found.mp4",
            &file_with(&[tkhd(1), boxed(*b"edts", &elst(&[(3000, 1024)]))]),
        );
        let lists = read_edit_lists(&path).expect("reads");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].track_id, 1);
        assert_eq!(lists[0].entries[0].media_time, 1024);
        assert!(lists[0].shifts_presentation());
    }

    #[test]
    fn a_single_edit_from_zero_changes_nothing() {
        let list = EditList {
            track_id: 1,
            entries: vec![EditListEntry {
                segment_duration: 9000,
                media_time: 0,
            }],
        };
        assert!(!list.shifts_presentation());
    }

    #[test]
    fn an_entry_count_larger_than_the_box_is_not_believed() {
        let mut lie = elst(&[(1, 1)]);
        // Claim four billion entries in a box that holds one.
        lie[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        let path = write("lie.mp4", &file_with(&[tkhd(2), boxed(*b"edts", &lie)]));
        let lists = read_edit_lists(&path).expect("reads");
        assert_eq!(lists[0].entries.len(), 1);
    }

    #[test]
    fn a_box_claiming_more_than_its_parent_ends_the_walk_quietly() {
        let mut file = boxed(*b"ftyp", b"isom");
        file.extend_from_slice(&u32::MAX.to_be_bytes());
        file.extend_from_slice(b"moov");
        let path = write("truncated.mp4", &file);
        assert!(read_edit_lists(&path).expect("reads").is_empty());
    }

    #[test]
    fn a_file_that_is_not_iso_bmff_has_no_edit_lists() {
        let path = write("text.mp4", b"this is not a video file at all");
        assert!(read_edit_lists(&path).expect("reads").is_empty());
        let empty = write("empty.mp4", b"");
        assert!(read_edit_lists(&empty).expect("reads").is_empty());
    }
}
