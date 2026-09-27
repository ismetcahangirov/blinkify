//! Reframe (#132, ADR-0022): a sequence turned to another aspect ratio in one
//! edit, made of what already exists — the sequence settings and a crop per
//! clip — so it undoes exactly (ADR-0007) and costs quality only where a clip
//! is cropped.
//!
//! - **Every video clip on an unlocked track gets a centred crop of the
//!   target shape** ([`centred`], the one place a preset is rounded), unless
//!   its picture already has that shape — then it gets none, and stays a
//!   copy where the sequence matches it. A clip whose crop already has the
//!   shape keeps it where the user put it, so reframing twice changes
//!   nothing and a moved crop is not re-centred.
//! - **The sequence takes a size the sources give.** A source already of the
//!   target shape, whose frame rate the sequence has, lends its exact size:
//!   its clips stay copies, and the cropped clips are re-encoded to that size
//!   whatever it is — scaled up if the crops are smaller, which is said.
//!   With none, the size is the crop of the source with the most time on the
//!   timeline, so that source is never scaled up.
//! - **A locked track is left alone**, and the statement says so. Its clips
//!   are not cropped; the new settings still apply to them, as a settings
//!   change always does.
//! - **Reframing back removes the crops.** A clip whose picture is the shape
//!   asked for has no crop after it, whatever it had before.

use std::collections::BTreeMap;

use serde::Serialize;
use ts_rs::TS;

use super::crop::{Aspect, centred, has_aspect};
use super::edit::{Change, EditError, Facts, crop_of, picture, recropped};
use super::evaluate::evaluate;
use super::settings::{SequenceSettings, StreamGeometry, copy_eligibility, reduced};
use super::{ClipId, Project, SourceId, TrackKind};

/// Why the reframed sequence has the size it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "basis",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum ReframeBasis {
    /// `source` is already of the target shape: the sequence takes its size,
    /// so its clips stay copies.
    Native { source: SourceId },
    /// No source is: the sequence takes the size of `source`'s crop — the
    /// source with the most time on the timeline — so it is not scaled up.
    Cropped { source: SourceId },
}

/// What a reframe decides, before or after it is applied.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Reframe {
    pub aspect: Aspect,
    /// The sequence settings after it.
    pub settings: SequenceSettings,
    pub basis: ReframeBasis,
    /// Clips given a centred crop of the target shape.
    pub cropped: Vec<ClipId>,
    /// Clips whose crop already had the shape: kept where it is.
    pub kept: Vec<ClipId>,
    /// Clips whose picture is already the shape: no crop.
    pub uncropped: Vec<ClipId>,
    /// Clips on locked tracks, left as they are.
    pub locked: Vec<ClipId>,
    /// Reframed clips whose picture, cropped or not, is smaller than the
    /// sequence, so it is scaled up to fill it.
    pub scaled_up: Vec<ClipId>,
}

fn refused(reason: impl Into<String>) -> EditError {
    EditError::Refused(reason.into())
}

/// Whether `shape`'s whole picture is of `aspect`, to within its grid.
fn in_shape(shape: &StreamGeometry, aspect: Aspect) -> bool {
    centred(shape, aspect).is_some_and(|rect| rect.is_whole(shape))
}

/// The sequence at `width` × `height` of `shape`'s pixels, at the frame rate
/// it has now.
fn sized(
    current: &SequenceSettings,
    shape: &StreamGeometry,
    width: u32,
    height: u32,
) -> SequenceSettings {
    SequenceSettings {
        width,
        height,
        frame_rate: current.frame_rate,
        pixel_aspect: reduced(shape.pixel_aspect),
        colour: current.colour,
    }
}

/// The source with the most time among `candidates`, the lowest id on a tie.
fn most_time<T: Copy>(
    candidates: impl Iterator<Item = (SourceId, i64, T)>,
) -> Option<(SourceId, T)> {
    let mut best: Option<(SourceId, i64, T)> = None;
    for candidate in candidates {
        if best.is_none_or(|(_, frames, _)| candidate.1 > frames) {
            best = Some(candidate);
        }
    }
    best.map(|(source, _, value)| (source, value))
}

/// The sequence settings a reframe to `aspect` leaves, and why: the size of
/// a source already of that shape, or else of the crop of the source with
/// the most time. `time` is each source's frames on the timeline.
fn size(
    current: &SequenceSettings,
    time: &BTreeMap<SourceId, (i64, &StreamGeometry)>,
    aspect: Aspect,
) -> Result<(SequenceSettings, ReframeBasis), EditError> {
    let native = most_time(time.iter().filter_map(|(&source, &(frames, shape))| {
        let settings = sized(current, shape, shape.width, shape.height);
        (in_shape(shape, aspect)
            && settings.validate().is_ok()
            && copy_eligibility(&settings, shape).eligible)
            .then_some((source, frames, settings))
    }));
    if let Some((source, settings)) = native {
        return Ok((settings, ReframeBasis::Native { source }));
    }
    let (source, shape) = most_time(
        time.iter()
            .map(|(&source, &(frames, shape))| (source, frames, shape)),
    )
    .ok_or_else(|| refused("the sequence has no video clips to reframe"))?;
    let rect = centred(shape, aspect).ok_or_else(|| {
        refused(format!(
            "source {source}'s picture is too small to hold a {aspect} crop"
        ))
    })?;
    // A sequence's sides are even, whatever the source's grid.
    let even = |side: u32| (side - side % 2).max(2);
    let settings = sized(current, shape, even(rect.width), even(rect.height));
    settings.validate()?;
    Ok((settings, ReframeBasis::Cropped { source }))
}

/// What reframing `project` to `aspect` decides, and the changes that make
/// it: the one compile of [`super::edit::Edit::Reframe`], which the preview
/// of its cost also runs.
pub(super) fn reframe(
    project: &Project,
    facts: &Facts,
    aspect: Aspect,
) -> Result<(Reframe, Vec<Change>), EditError> {
    if aspect == Aspect::Source {
        return Err(refused(
            "a sequence is reframed to a ratio: 16:9, 9:16, 1:1 or 4:5",
        ));
    }
    let timeline = evaluate(project).map_err(|error| refused(error.to_string()))?;
    let lengths: BTreeMap<ClipId, i64> = timeline
        .placements()
        .map(|placement| (placement.clip, placement.length))
        .collect();

    let mut locked = Vec::new();
    let mut open = Vec::new();
    for track in &project.sequence.tracks {
        if track.kind != TrackKind::Video {
            continue;
        }
        for clip in &track.clips {
            if track.locked {
                locked.push(clip.id);
                continue;
            }
            let shape = picture(facts, track, clip).map_err(|_| {
                refused(format!(
                    "clip {}'s source is offline or could not be read, so its crop cannot \
                     be placed: relink it before reframing",
                    clip.id
                ))
            })?;
            open.push((clip, shape));
        }
    }
    if open.is_empty() {
        return Err(refused(if locked.is_empty() {
            "the sequence has no video clips to reframe: choose its size in Sequence settings"
        } else {
            "every video clip is on a locked track: unlock one to reframe"
        }));
    }

    // Time on the timeline of each source among the clips reframed.
    let mut time: BTreeMap<SourceId, (i64, &StreamGeometry)> = BTreeMap::new();
    for (clip, shape) in &open {
        let entry = time.entry(clip.source).or_insert((0, shape));
        entry.0 += lengths.get(&clip.id).copied().unwrap_or(0);
    }
    let current = project.sequence.settings;
    let (settings, basis) = size(&current, &time, aspect)?;

    let mut decided = Reframe {
        aspect,
        settings,
        basis,
        cropped: Vec::new(),
        kept: Vec::new(),
        uncropped: Vec::new(),
        locked,
        scaled_up: Vec::new(),
    };
    let mut changes = Vec::new();
    if settings != current {
        changes.push(Change::Settings(settings));
    }
    if project.sequence.match_first_clip {
        changes.push(Change::MatchFirstClip(false));
    }
    for (clip, shape) in open {
        let now = crop_of(clip);
        let shown = if in_shape(shape, aspect) {
            decided.uncropped.push(clip.id);
            changes.extend(recropped(clip, None, None));
            None
        } else if let Some(rect) = now.filter(|rect| has_aspect(rect, shape, aspect)) {
            decided.kept.push(clip.id);
            Some(rect)
        } else {
            let rect = centred(shape, aspect).ok_or_else(|| {
                refused(format!(
                    "clip {}'s picture is too small to hold a {aspect} crop",
                    clip.id
                ))
            })?;
            decided.cropped.push(clip.id);
            changes.extend(recropped(clip, Some(rect), Some(shape)));
            Some(rect)
        };
        let (width, height) = shown.map_or((shape.width, shape.height), |r| (r.width, r.height));
        if width < settings.width || height < settings.height {
            decided.scaled_up.push(clip.id);
        }
    }
    Ok((decided, changes))
}
