//! Edits: the only way an open project's graph changes (#37).
//!
//! The open project is a [`Document`]. It owns its [`Project`] privately and
//! hands out only a shared reference, so the one `&mut Project` of an open
//! project is the one inside [`Document::apply`] — the compiler, not a
//! convention, keeps every mutation on the undo stack:
//!
//! ```compile_fail
//! # use blinkify_engine::project::{Project, SequenceSettings, TrackKind};
//! # use blinkify_engine::project::edit::Document;
//! let document = Document::new(Project::new("t", SequenceSettings::default())).unwrap();
//! document.project().sequence.tracks.clear(); // a shared reference: no.
//! ```
//!
//! An [`Edit`] is what the user did, as data — "move these clips there". It
//! is compiled into primitive [`Change`]s, each of which returns its own
//! inverse when applied. The history keeps the inverses, not snapshots: the
//! payload of an entry is proportional to what changed, however large the
//! project grows. Undoing applies the inverses backwards, which yields the
//! forward changes again for redo — so undo followed by redo is the same
//! arithmetic run twice, and returns the graph byte for byte.
//!
//! An edit that would leave a graph the evaluator (#30) cannot resolve — two
//! clips overlapping, a trim running backwards — is rolled back and refused
//! with the evaluator's reason, so the preview and the export never see it.
//!
//! A gesture — a slider being dragged — is bracketed explicitly with
//! [`Document::begin_gesture`] and [`Document::end_gesture`]: everything in
//! between is one history entry. There is no time-based guess.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::evaluate::{EvaluatedTrack, Placement, Timeline, evaluate};
use super::settings::{CopyEligibility, SettingsError, StreamGeometry, copy_eligibility};
use super::trim::{Edge, StreamExtent, reach, retimed, trim};
use super::{
    Clip, ClipId, Operation, Project, ProjectError, SequenceSettings, SourceId, SourceRef, Track,
    TrackId, TrackKind,
};
use crate::probe::Rational;

/// How many entries the history keeps before it forgets the oldest. An
/// entry holds only what its edit changed, so this is generous; it bounds a
/// session that runs for days, not a normal one.
pub const DEFAULT_DEPTH: usize = 500;

/// What accompanied an edit in the editor: the selection and the playhead.
/// Undo gives back the context from before the edit, redo the one after, so
/// what comes back is selected and can be acted on at once.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EditContext {
    pub selection: Vec<ClipId>,
    /// Sequence frames.
    #[ts(type = "number")]
    pub playhead: i64,
}

/// Where one clip goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipMove {
    pub clip: ClipId,
    pub track: TrackId,
    /// Sequence frames.
    #[ts(type = "number")]
    pub start: i64,
}

/// One thing the user did to the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "edit",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum Edit {
    Rename {
        name: String,
    },
    /// A new, empty track below the others of its kind.
    AddTrack {
        kind: TrackKind,
    },
    /// Place `from..to` of a source stream on `track` at `start`.
    AddClip {
        track: TrackId,
        source: SourceId,
        stream: u32,
        time_base: Rational,
        #[ts(type = "number")]
        start: i64,
        #[ts(type = "number")]
        from: i64,
        #[ts(type = "number")]
        to: i64,
    },
    /// Move clips, together: to another place, another track, or both.
    MoveClips {
        moves: Vec<ClipMove>,
    },
    /// Move one edge of a clip by `frames` sequence frames — positive is
    /// later. Bounded by the source's extent, one frame of length and, unless
    /// it ripples, the neighbouring clips. A ripple trim keeps the clip's
    /// start and moves every later clip on the track by the change.
    TrimEdge {
        clip: ClipId,
        edge: Edge,
        #[ts(type = "number")]
        frames: i64,
        ripple: bool,
    },
    /// Move the cut between two adjacent clips by `frames`: the left one's
    /// end and the right one's start together.
    Roll {
        left: ClipId,
        right: ClipId,
        #[ts(type = "number")]
        frames: i64,
    },
    /// Play the clips at `ratio` times normal speed; `1/1` is normal.
    SetSpeed {
        clips: Vec<ClipId>,
        ratio: Rational,
    },
    RemoveClips {
        clips: Vec<ClipId>,
    },
    /// Remove clips and close the gaps they leave: every later clip on each
    /// track moves earlier by the length removed before it.
    RippleDelete {
        clips: Vec<ClipId>,
    },
    /// Choose the sequence settings (#57): it stops waiting for a first clip.
    SetSettings {
        settings: SequenceSettings,
    },
}

impl Edit {
    /// What the history list calls it.
    #[must_use]
    pub fn label(&self) -> String {
        let clips = |count: usize, one: &str, many: &str| {
            if count == 1 {
                one.to_owned()
            } else {
                format!("{many} {count} clips")
            }
        };
        match self {
            Self::Rename { .. } => "Rename project".to_owned(),
            Self::AddTrack { kind } => match kind {
                TrackKind::Video => "Add video track".to_owned(),
                TrackKind::Audio => "Add audio track".to_owned(),
            },
            Self::AddClip { .. } => "Add clip".to_owned(),
            Self::MoveClips { moves } => clips(moves.len(), "Move clip", "Move"),
            Self::TrimEdge { ripple: false, .. } => "Trim clip".to_owned(),
            Self::TrimEdge { ripple: true, .. } => "Ripple trim".to_owned(),
            Self::Roll { .. } => "Roll edit".to_owned(),
            Self::RippleDelete { clips: ids } => clips(ids.len(), "Ripple delete", "Ripple delete"),
            Self::SetSpeed { clips: ids, .. } => {
                clips(ids.len(), "Change speed", "Change speed of")
            }
            Self::RemoveClips { clips: ids } => clips(ids.len(), "Delete clip", "Delete"),
            Self::SetSettings { .. } => "Change sequence settings".to_owned(),
        }
    }
}

/// Why an edit was refused. The graph is unchanged.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("there is no clip {0}")]
    NoClip(ClipId),
    #[error("there is no track {0}")]
    NoTrack(TrackId),
    #[error("there is no source {0}")]
    NoSource(SourceId),
    #[error("a clip from a {from:?} track cannot go on a {to:?} track")]
    WrongKind { from: TrackKind, to: TrackKind },
    /// The result would not evaluate; the reason is the evaluator's.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Settings(#[from] SettingsError),
}

/// One primitive change to the graph. Applying it returns its inverse.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Change {
    Name(String),
    Settings(SequenceSettings),
    /// Whether the sequence waits for its first clip's settings.
    MatchFirstClip(bool),
    /// Set a source, or remove it with `None`.
    Source(SourceId, Option<SourceRef>),
    /// Put a track at an index of the track list.
    InsertTrack(usize, Track),
    RemoveTrack(TrackId),
    /// Put a clip on a track: at an index, or with `None` at its place in
    /// timeline order.
    InsertClip {
        track: TrackId,
        index: Option<usize>,
        clip: Clip,
    },
    RemoveClip(ClipId),
    /// Replace a clip, by id, where it stands.
    ReplaceClip(Clip),
}

/// The track holding clip `id`, and the clip's index on it.
fn locate(project: &Project, id: ClipId) -> Option<(&Track, usize)> {
    project.sequence.tracks.iter().find_map(|track| {
        track
            .clips
            .iter()
            .position(|c| c.id == id)
            .map(|c| (track, c))
    })
}

/// The track holding clip `id`, and the clip.
fn find(project: &Project, id: ClipId) -> Result<(&Track, &Clip), EditError> {
    project
        .sequence
        .tracks
        .iter()
        .find_map(|track| {
            track
                .clips
                .iter()
                .find(|c| c.id == id)
                .map(|clip| (track, clip))
        })
        .ok_or(EditError::NoClip(id))
}

fn track_mut(project: &mut Project, id: TrackId) -> Result<&mut Track, EditError> {
    project
        .sequence
        .tracks
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or(EditError::NoTrack(id))
}

fn find_track(project: &Project, id: TrackId) -> Result<&Track, EditError> {
    project
        .sequence
        .tracks
        .iter()
        .find(|t| t.id == id)
        .ok_or(EditError::NoTrack(id))
}

impl Change {
    /// Apply the change and return what undoes it.
    fn apply(self, project: &mut Project) -> Result<Self, EditError> {
        Ok(match self {
            Self::Name(name) => Self::Name(std::mem::replace(&mut project.name, name)),
            Self::Settings(settings) => {
                Self::Settings(std::mem::replace(&mut project.sequence.settings, settings))
            }
            Self::MatchFirstClip(waits) => Self::MatchFirstClip(std::mem::replace(
                &mut project.sequence.match_first_clip,
                waits,
            )),
            Self::Source(id, reference) => {
                let previous = match reference {
                    Some(reference) => project.sources.insert(id, reference),
                    None => project.sources.remove(&id),
                };
                Self::Source(id, previous)
            }
            Self::InsertTrack(index, track) => {
                let id = track.id;
                let index = index.min(project.sequence.tracks.len());
                project.sequence.tracks.insert(index, track);
                Self::RemoveTrack(id)
            }
            Self::RemoveTrack(id) => {
                let index = project
                    .sequence
                    .tracks
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or(EditError::NoTrack(id))?;
                Self::InsertTrack(index, project.sequence.tracks.remove(index))
            }
            Self::InsertClip { track, index, clip } => {
                let clips = &mut track_mut(project, track)?.clips;
                let index = index
                    .unwrap_or_else(|| clips.partition_point(|c| c.start <= clip.start))
                    .min(clips.len());
                let id = clip.id;
                clips.insert(index, clip);
                Self::RemoveClip(id)
            }
            Self::RemoveClip(id) => {
                let (track, index) = locate(project, id)
                    .map(|(track, index)| (track.id, index))
                    .ok_or(EditError::NoClip(id))?;
                Self::InsertClip {
                    track,
                    index: Some(index),
                    clip: track_mut(project, track)?.clips.remove(index),
                }
            }
            Self::ReplaceClip(clip) => {
                let slot = project
                    .sequence
                    .tracks
                    .iter_mut()
                    .flat_map(|t| t.clips.iter_mut())
                    .find(|c| c.id == clip.id)
                    .ok_or(EditError::NoClip(clip.id))?;
                Self::ReplaceClip(std::mem::replace(slot, clip))
            }
        })
    }
}

/// Apply `changes` in order. On a failure, what was applied is undone and
/// the error returned; on success, the inverses, in the order applied.
fn apply_all(project: &mut Project, changes: Vec<Change>) -> Result<Vec<Change>, EditError> {
    let mut inverses = Vec::with_capacity(changes.len());
    for change in changes {
        match change.apply(project) {
            Ok(inverse) => inverses.push(inverse),
            Err(error) => {
                revert(project, inverses);
                return Err(error);
            }
        }
    }
    Ok(inverses)
}

/// Apply `inverses` backwards: undo what produced them. Returns the forward
/// changes, in forward order, for redo.
fn revert(project: &mut Project, inverses: Vec<Change>) -> Vec<Change> {
    let mut forward: Vec<Change> = inverses
        .into_iter()
        .rev()
        // An inverse always applies: it was made from the state it undoes.
        .filter_map(|inverse| inverse.apply(project).ok())
        .collect();
    forward.reverse();
    forward
}

/// A clip with every speed change replaced by one of `ratio`, where the first
/// was — or none, at normal speed.
fn respeeded(clip: &Clip, ratio: Rational) -> Clip {
    let normal = ratio.num == ratio.den;
    let mut operations = Vec::with_capacity(clip.operations.len() + 1);
    let mut placed = false;
    for operation in &clip.operations {
        if matches!(operation, Operation::Speed { .. }) {
            if !placed && !normal {
                operations.push(Operation::Speed { ratio });
            }
            placed = true;
        } else {
            operations.push(*operation);
        }
    }
    if !placed && !normal {
        operations.push(Operation::Speed { ratio });
    }
    Clip {
        operations,
        ..clip.clone()
    }
}

/// What an edit compiles to: the primitive changes, the selection after,
/// and whether a bound stopped it short of what was asked.
struct Compiled {
    changes: Vec<Change>,
    selection: Vec<ClipId>,
    clamped: bool,
}

impl From<(Vec<Change>, Vec<ClipId>)> for Compiled {
    fn from((changes, selection): (Vec<Change>, Vec<ClipId>)) -> Self {
        Self {
            changes,
            selection,
            clamped: false,
        }
    }
}

/// What the document knows about its sources that is not in the graph: read
/// from the files, never saved.
#[derive(Debug, Default)]
struct Facts {
    /// What each source's pictures are (#57).
    geometry: BTreeMap<SourceId, StreamGeometry>,
    /// Which ticks of each source stream exist (#34).
    extents: BTreeMap<(SourceId, u32), StreamExtent>,
}

impl Facts {
    /// The ticks of `placement`'s stream that exist, in its time base.
    fn extent_of(&self, placement: &Placement) -> Option<(i64, i64)> {
        self.extents
            .get(&(placement.source, placement.stream))
            .filter(|extent| extent.time_base == placement.time_base)
            .map(|extent| (extent.start, extent.end))
    }
}

/// The primitive changes `edit` makes to `project`, and the selection after.
fn compile(
    project: &Project,
    facts: &Facts,
    edit: &Edit,
    context: &EditContext,
) -> Result<Compiled, EditError> {
    let kept = context.selection.clone();
    match edit {
        Edit::TrimEdge {
            clip,
            edge,
            frames,
            ripple,
        } => return trim_edge(project, facts, *clip, *edge, *frames, *ripple, kept),
        Edit::Roll {
            left,
            right,
            frames,
        } => return roll(project, facts, *left, *right, *frames, kept),
        Edit::RippleDelete { clips } => return ripple_delete(project, clips, kept),
        _ => {}
    }
    Ok(Compiled::from(match edit {
        Edit::Rename { name } => {
            if *name == project.name {
                (Vec::new(), kept)
            } else {
                (vec![Change::Name(name.clone())], kept)
            }
        }
        Edit::AddTrack { kind } => (vec![add_track(project, *kind)], kept),
        Edit::AddClip {
            track,
            source,
            stream,
            time_base,
            start,
            from,
            to,
        } => {
            find_track(project, *track)?;
            if !project.sources.contains_key(source) {
                return Err(EditError::NoSource(*source));
            }
            let id = project.clips().map(|(_, c)| c.id).max().unwrap_or(0) + 1;
            let clip = Clip::new(
                id,
                *source,
                *stream,
                *time_base,
                *start,
                vec![Operation::Trim {
                    from: *from,
                    to: *to,
                }],
            );
            let insert = Change::InsertClip {
                track: *track,
                index: None,
                clip,
            };
            let mut changes = adopt_first_clip(project, &facts.geometry, *track, *source);
            changes.push(insert);
            (changes, vec![id])
        }
        Edit::MoveClips { moves } => (move_clips(project, moves)?, kept),
        Edit::SetSpeed { clips, ratio } => (set_speed(project, clips, *ratio)?, kept),
        Edit::SetSettings { settings } => {
            settings.validate()?;
            let mut changes = Vec::new();
            if *settings != project.sequence.settings {
                changes.push(Change::Settings(*settings));
            }
            if project.sequence.match_first_clip {
                changes.push(Change::MatchFirstClip(false));
            }
            (changes, kept)
        }
        Edit::RemoveClips { clips } => {
            let mut changes = Vec::new();
            for &id in &clips.iter().copied().collect::<BTreeSet<_>>() {
                find(project, id)?;
                changes.push(Change::RemoveClip(id));
            }
            let remaining = kept.into_iter().filter(|c| !clips.contains(c)).collect();
            (changes, remaining)
        }
        Edit::TrimEdge { .. } | Edit::Roll { .. } | Edit::RippleDelete { .. } => {
            unreachable!("compiled above")
        }
    }))
}

/// The evaluated graph, for edits made in sequence frames. An edit to a graph
/// that does not evaluate cannot be placed in frames at all.
fn timeline_of(project: &Project) -> Result<Timeline, EditError> {
    evaluate(project).map_err(|error| EditError::Refused(error.to_string()))
}

/// The placement of `clip`, and the evaluated track it is on.
fn placed(timeline: &Timeline, clip: ClipId) -> Result<(&EvaluatedTrack, &Placement), EditError> {
    timeline
        .tracks
        .iter()
        .find_map(|track| {
            track
                .placements
                .iter()
                .find(|p| p.clip == clip)
                .map(|p| (track, p))
        })
        .ok_or(EditError::NoClip(clip))
}

fn too_long(clip: ClipId) -> EditError {
    EditError::Refused(format!("clip {clip}'s timing does not fit in 64 bits"))
}

/// Replace `id` with its new source range and start, and move every later
/// clip of `track` (at or after `after`) by `shift` frames — how a ripple
/// keeps the rest of the track butted up.
fn retime_and_shift(
    project: &Project,
    track: &EvaluatedTrack,
    id: ClipId,
    (from, to, start): (i64, i64, i64),
    after: i64,
    shift: i64,
) -> Result<Vec<Change>, EditError> {
    let (_, current) = find(project, id)?;
    let mut changes = vec![Change::ReplaceClip(retimed(current, from, to, start))];
    if shift != 0 {
        for later in track
            .placements
            .iter()
            .filter(|p| p.clip != id && p.start >= after)
        {
            let (_, clip) = find(project, later.clip)?;
            changes.push(Change::ReplaceClip(Clip {
                start: later.start + shift,
                ..clip.clone()
            }));
        }
    }
    Ok(changes)
}

fn trim_edge(
    project: &Project,
    facts: &Facts,
    id: ClipId,
    edge: Edge,
    frames: i64,
    ripple: bool,
    kept: Vec<ClipId>,
) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let (track, placement) = placed(&timeline, id)?;
    let before = track
        .placements
        .iter()
        .filter(|p| p.clip != id && p.end() <= placement.start)
        .map(Placement::end)
        .max()
        .unwrap_or(0);
    let after = track
        .placements
        .iter()
        .filter(|p| p.clip != id && p.start >= placement.end())
        .map(|p| p.start)
        .min();
    // A ripple moves the neighbours out of the way; it is bounded only by the
    // source, and by frame 0 for the start.
    let room = if ripple {
        (Some(0), None)
    } else {
        (Some(before), after)
    };
    let trimmed = trim(placement, edge, frames, facts.extent_of(placement), room)
        .ok_or_else(|| too_long(id))?;
    let changes = if ripple {
        let shift = trimmed.length - placement.length;
        // The clip keeps its start; everything after it follows the change.
        retime_and_shift(
            project,
            track,
            id,
            (trimmed.from, trimmed.to, placement.start),
            placement.end(),
            shift,
        )?
    } else {
        retime_and_shift(
            project,
            track,
            id,
            (trimmed.from, trimmed.to, trimmed.start),
            placement.end(),
            0,
        )?
    };
    let unchanged = trimmed.from == placement.source_in
        && trimmed.to == placement.source_out
        && (ripple || trimmed.start == placement.start);
    Ok(Compiled {
        changes: if unchanged { Vec::new() } else { changes },
        selection: kept,
        clamped: trimmed.clamped,
    })
}

fn roll(
    project: &Project,
    facts: &Facts,
    left: ClipId,
    right: ClipId,
    frames: i64,
    kept: Vec<ClipId>,
) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let (left_track, a) = placed(&timeline, left)?;
    let (right_track, b) = placed(&timeline, right)?;
    if left_track.id != right_track.id || a.end() != b.start {
        return Err(EditError::Refused(format!(
            "clips {left} and {right} do not meet, so there is no cut between them to roll"
        )));
    }
    let (_, latest_a) = reach(a, Edge::End, facts.extent_of(a));
    let (earliest_b, _) = reach(b, Edge::Start, facts.extent_of(b));
    let lowest = (1 - a.length).max(earliest_b);
    let highest = latest_a.min(b.length - 1);
    let delta = frames.clamp(lowest.min(0), highest.max(0));
    let a_trim = trim(a, Edge::End, delta, facts.extent_of(a), (None, None))
        .ok_or_else(|| too_long(left))?;
    let b_trim = trim(b, Edge::Start, delta, facts.extent_of(b), (None, None))
        .ok_or_else(|| too_long(right))?;
    if a_trim.start + a_trim.length != b_trim.start {
        return Err(EditError::Refused(format!(
            "the cut between clips {left} and {right} cannot move by exactly {delta} frames"
        )));
    }
    let (_, a_clip) = find(project, left)?;
    let (_, b_clip) = find(project, right)?;
    let changes = if delta == 0 {
        Vec::new()
    } else {
        vec![
            Change::ReplaceClip(retimed(a_clip, a_trim.from, a_trim.to, a_trim.start)),
            Change::ReplaceClip(retimed(b_clip, b_trim.from, b_trim.to, b_trim.start)),
        ]
    };
    Ok(Compiled {
        changes,
        selection: kept,
        clamped: delta != frames || a_trim.clamped || b_trim.clamped,
    })
}

fn ripple_delete(
    project: &Project,
    clips: &[ClipId],
    kept: Vec<ClipId>,
) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let removed: BTreeSet<ClipId> = clips.iter().copied().collect();
    let mut changes = Vec::new();
    for &id in &removed {
        placed(&timeline, id)?;
        changes.push(Change::RemoveClip(id));
    }
    for track in &timeline.tracks {
        let gaps: Vec<&Placement> = track
            .placements
            .iter()
            .filter(|p| removed.contains(&p.clip))
            .collect();
        if gaps.is_empty() {
            continue;
        }
        for later in track
            .placements
            .iter()
            .filter(|p| !removed.contains(&p.clip))
        {
            let shift: i64 = gaps
                .iter()
                .filter(|gap| gap.end() <= later.start)
                .map(|gap| gap.length)
                .sum();
            if shift > 0 {
                let (_, clip) = find(project, later.clip)?;
                changes.push(Change::ReplaceClip(Clip {
                    start: later.start - shift,
                    ..clip.clone()
                }));
            }
        }
    }
    let remaining = kept.into_iter().filter(|c| !removed.contains(c)).collect();
    Ok(Compiled {
        changes,
        selection: remaining,
        clamped: false,
    })
}

/// The `match first clip` default (#57): the first video clip placed on a
/// sequence still waiting for one gives it the clip's own geometry and frame
/// rate, in the same edit, so undoing the placement gives the settings back.
/// A clip whose shape no sequence can have leaves the settings as they are,
/// and the eligibility report says why it cannot be copied.
fn adopt_first_clip(
    project: &Project,
    geometry: &BTreeMap<SourceId, StreamGeometry>,
    track: TrackId,
    source: SourceId,
) -> Vec<Change> {
    let video = find_track(project, track).is_ok_and(|t| t.kind == TrackKind::Video);
    let empty = project.clips().next().is_none();
    if !(project.sequence.match_first_clip && video && empty) {
        return Vec::new();
    }
    geometry
        .get(&source)
        .and_then(|shape| SequenceSettings::matching(shape).ok())
        .map(|settings| vec![Change::Settings(settings), Change::MatchFirstClip(false)])
        .unwrap_or_default()
}

/// A new track below the last of its kind — or, the first of its kind,
/// videos above audio.
fn add_track(project: &Project, kind: TrackKind) -> Change {
    let tracks = &project.sequence.tracks;
    let id = tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1;
    let index = tracks.iter().rposition(|t| t.kind == kind).map_or_else(
        || match kind {
            TrackKind::Video => 0,
            TrackKind::Audio => tracks.len(),
        },
        |last| last + 1,
    );
    Change::InsertTrack(
        index,
        Track {
            id,
            kind,
            clips: Vec::new(),
        },
    )
}

/// Every moving clip leaves first, then each arrives, so clips moving past
/// each other never collide half way.
fn move_clips(project: &Project, moves: &[ClipMove]) -> Result<Vec<Change>, EditError> {
    let mut removes = Vec::new();
    let mut inserts = Vec::new();
    let mut seen = BTreeSet::new();
    for step in moves {
        if !seen.insert(step.clip) {
            continue;
        }
        let (from, current) = find(project, step.clip)?;
        let to = find_track(project, step.track)?;
        if from.kind != to.kind {
            return Err(EditError::WrongKind {
                from: from.kind,
                to: to.kind,
            });
        }
        if from.id == step.track && current.start == step.start {
            continue;
        }
        removes.push(Change::RemoveClip(step.clip));
        inserts.push(Change::InsertClip {
            track: step.track,
            index: None,
            clip: Clip {
                start: step.start,
                ..current.clone()
            },
        });
    }
    removes.append(&mut inserts);
    Ok(removes)
}

fn set_speed(
    project: &Project,
    clips: &[ClipId],
    ratio: Rational,
) -> Result<Vec<Change>, EditError> {
    if ratio.num <= 0 || ratio.den <= 0 {
        return Err(EditError::Refused(format!(
            "a speed of {}/{} is not a speed",
            ratio.num, ratio.den
        )));
    }
    let mut changes = Vec::new();
    for &id in &clips.iter().copied().collect::<BTreeSet<_>>() {
        let (_, current) = find(project, id)?;
        let respeeded = respeeded(current, ratio);
        if respeeded != *current {
            changes.push(Change::ReplaceClip(respeeded));
        }
    }
    Ok(changes)
}

/// One undoable step.
#[derive(Debug, Clone)]
struct Entry {
    label: String,
    /// Applied backwards, these undo the step — or, after an undo, applied
    /// forwards, they redo it.
    changes: Vec<Change>,
    before: EditContext,
    after: EditContext,
}

/// The history as the editor lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryView {
    /// Every entry, oldest first.
    pub entries: Vec<String>,
    /// How many of them are applied: the rest can be redone.
    pub applied: usize,
}

/// What a change of sequence settings would do to copying (#57).
#[derive(Debug, Clone, Default, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsImpact {
    /// Clips copy-eligible now that would have to be re-encoded.
    pub losing_clips: u32,
    pub losing_seconds: f64,
    /// Clips re-encoded now that could be copied.
    pub gaining_clips: u32,
    pub gaining_seconds: f64,
    /// Clips that could not be copied under the new settings, in all.
    pub ineligible_after: u32,
}

/// An open project and its history.
#[derive(Debug)]
pub struct Document {
    project: Project,
    done: VecDeque<Entry>,
    undone: Vec<Entry>,
    /// The entry a gesture is building, if one is under way.
    gesture: Option<Entry>,
    depth: usize,
    /// Whether the graph evaluated before the edit being applied. An edit is
    /// refused for breaking a graph that worked, not for failing to mend one
    /// that did not.
    evaluates: bool,
    /// What each source is, as probed: not part of the graph and not saved.
    /// Copy eligibility and the first-clip default are decided from its
    /// pictures (#57), trim bounds from its streams' extents (#34).
    facts: Facts,
}

/// An edit applied: the context after it, and whether a bound stopped it
/// short of what was asked — a trim that met the end of its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Applied {
    pub context: EditContext,
    pub clamped: bool,
}

impl Document {
    /// Open `project` with an empty history, [`DEFAULT_DEPTH`] deep.
    ///
    /// # Errors
    ///
    /// The project is inconsistent ([`Project::validate`]).
    pub fn new(project: Project) -> Result<Self, ProjectError> {
        Self::with_depth(project, DEFAULT_DEPTH)
    }

    /// Open `project` with a history of at most `depth` entries (at least 1).
    ///
    /// # Errors
    ///
    /// The project is inconsistent ([`Project::validate`]).
    pub fn with_depth(project: Project, depth: usize) -> Result<Self, ProjectError> {
        project.validate()?;
        let evaluates = evaluate(&project).is_ok();
        Ok(Self {
            project,
            done: VecDeque::new(),
            undone: Vec::new(),
            gesture: None,
            depth: depth.max(1),
            evaluates,
            facts: Facts::default(),
        })
    }

    /// The graph, to read.
    #[must_use]
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Apply `edit`, done with `context` in the editor. Returns the context
    /// after it: the selection it leaves.
    ///
    /// # Errors
    ///
    /// The edit names something that does not exist, or its result would not
    /// evaluate. The graph and the history are then unchanged.
    pub fn apply(&mut self, edit: &Edit, context: &EditContext) -> Result<Applied, EditError> {
        let compiled = compile(&self.project, &self.facts, edit, context)?;
        let after = EditContext {
            selection: compiled.selection,
            playhead: context.playhead,
        };
        let context = self.commit(edit.label(), compiled.changes, context, after)?;
        Ok(Applied {
            context,
            clamped: compiled.clamped,
        })
    }

    /// Add sources — an import — as one undoable step.
    ///
    /// Returns the id of each, in order; a source already in the project
    /// keeps its id and is not added twice.
    pub fn add_sources(
        &mut self,
        references: &[SourceRef],
        context: &EditContext,
    ) -> Vec<SourceId> {
        let mut ids = Vec::with_capacity(references.len());
        let mut changes = Vec::new();
        let mut next = self.project.sources.keys().next_back().map_or(1, |l| l + 1);
        let mut added: Vec<(SourceId, &SourceRef)> = Vec::new();
        for reference in references {
            let existing = self
                .project
                .sources
                .iter()
                .map(|(&id, known)| (id, known))
                .chain(added.iter().copied())
                .find(|(_, known)| known.fingerprint().same_content(reference.fingerprint()));
            if let Some((id, _)) = existing {
                ids.push(id);
                continue;
            }
            ids.push(next);
            added.push((next, reference));
            changes.push(Change::Source(next, Some(reference.clone())));
            next += 1;
        }
        let label = if changes.len() == 1 {
            "Import file".to_owned()
        } else {
            format!("Import {} files", changes.len())
        };
        // Adding a source cannot make a graph unevaluable.
        let _ = self.commit(label, changes, context, context.clone());
        ids
    }

    /// Point `source` at `reference`: the same content, moved.
    ///
    /// # Errors
    ///
    /// No such source.
    pub fn relink(
        &mut self,
        source: SourceId,
        reference: SourceRef,
        context: &EditContext,
    ) -> Result<(), EditError> {
        if !self.project.sources.contains_key(&source) {
            return Err(EditError::NoSource(source));
        }
        self.commit(
            "Relink source".to_owned(),
            vec![Change::Source(source, Some(reference))],
            context,
            context.clone(),
        )
        .map(|_| ())
    }

    fn commit(
        &mut self,
        label: String,
        changes: Vec<Change>,
        before: &EditContext,
        after: EditContext,
    ) -> Result<EditContext, EditError> {
        if changes.is_empty() {
            return Ok(after);
        }
        let inverses = apply_all(&mut self.project, changes)?;
        if self.evaluates
            && let Err(error) = evaluate(&self.project)
        {
            revert(&mut self.project, inverses);
            return Err(EditError::Refused(error.to_string()));
        }
        self.evaluates = true;
        self.undone.clear();
        if let Some(gesture) = &mut self.gesture {
            coalesce(&mut gesture.changes, inverses);
            gesture.after = after.clone();
        } else {
            self.push(Entry {
                label,
                changes: inverses,
                before: before.clone(),
                after: after.clone(),
            });
        }
        Ok(after)
    }

    fn push(&mut self, entry: Entry) {
        self.done.push_back(entry);
        while self.done.len() > self.depth {
            self.done.pop_front();
        }
    }

    /// Start a gesture: every edit until [`Document::end_gesture`] is one
    /// history entry, called `label`. A gesture already under way is ended
    /// first.
    pub fn begin_gesture(&mut self, label: &str, context: &EditContext) {
        self.end_gesture();
        self.gesture = Some(Entry {
            label: label.to_owned(),
            changes: Vec::new(),
            before: context.clone(),
            after: context.clone(),
        });
    }

    /// End the gesture under way, if any. A gesture that changed nothing
    /// leaves no entry.
    pub fn end_gesture(&mut self) {
        if let Some(entry) = self.gesture.take()
            && !entry.changes.is_empty()
        {
            self.push(entry);
        }
    }

    /// Undo the last entry. Returns the context from before it, or `None`
    /// when there is nothing to undo.
    pub fn undo(&mut self) -> Option<EditContext> {
        self.end_gesture();
        let mut entry = self.done.pop_back()?;
        entry.changes = revert(&mut self.project, std::mem::take(&mut entry.changes));
        let context = entry.before.clone();
        self.undone.push(entry);
        self.evaluates = evaluate(&self.project).is_ok();
        Some(context)
    }

    /// Redo the last undone entry. Returns the context from after it, or
    /// `None` when there is nothing to redo.
    pub fn redo(&mut self) -> Option<EditContext> {
        self.end_gesture();
        let mut entry = self.undone.pop()?;
        // The forward changes were made from exactly this state, so they
        // apply; if one somehow did not, the entry is dropped rather than
        // half-applied.
        entry.changes = apply_all(&mut self.project, std::mem::take(&mut entry.changes)).ok()?;
        let context = entry.after.clone();
        self.push(entry);
        self.evaluates = evaluate(&self.project).is_ok();
        Some(context)
    }

    /// Record what `source`'s pictures are, or with `None` that it has none
    /// or could not be read.
    pub fn describe_source(&mut self, source: SourceId, geometry: Option<StreamGeometry>) {
        match geometry {
            Some(geometry) => self.facts.geometry.insert(source, geometry),
            None => self.facts.geometry.remove(&source),
        };
    }

    /// Record which ticks of `source`'s streams exist, replacing what was
    /// known. Trims are bounded by them.
    pub fn describe_streams(&mut self, source: SourceId, extents: &[StreamExtent]) {
        self.facts.extents.retain(|(id, _), _| *id != source);
        for extent in extents {
            self.facts
                .extents
                .insert((source, extent.stream), StreamExtent { source, ..*extent });
        }
    }

    /// Every known stream extent — what the timeline bounds a trim preview
    /// by, before the engine bounds the trim itself.
    #[must_use]
    pub fn extents(&self) -> Vec<StreamExtent> {
        self.facts
            .extents
            .values()
            .filter(|extent| self.project.sources.contains_key(&extent.source))
            .copied()
            .collect()
    }

    /// Whether each source with pictures can be stream-copied into the
    /// sequence as it is now: the model's one answer, for the timeline, the
    /// inspector, the export dialog and the planner to read.
    #[must_use]
    pub fn eligibility(&self) -> BTreeMap<SourceId, CopyEligibility> {
        self.eligibility_at(&self.project.sequence.settings)
    }

    fn eligibility_at(&self, settings: &SequenceSettings) -> BTreeMap<SourceId, CopyEligibility> {
        self.facts
            .geometry
            .iter()
            .filter(|(id, _)| self.project.sources.contains_key(id))
            .map(|(&id, shape)| (id, copy_eligibility(settings, shape)))
            .collect()
    }

    /// What changing the sequence to `settings` would do to copying, before
    /// it is done: the clips that would stop being copy-eligible and those
    /// that would start, with their duration.
    ///
    /// # Errors
    ///
    /// The settings are ones no file can carry.
    pub fn settings_impact(
        &self,
        settings: &SequenceSettings,
    ) -> Result<SettingsImpact, SettingsError> {
        settings.validate()?;
        let now = self.eligibility();
        let then = self.eligibility_at(settings);
        let timeline = evaluate(&self.project).ok();
        let rate = self.project.sequence.settings.frame_rate;
        let mut impact = SettingsImpact::default();
        for (track, clip) in self.project.clips() {
            if track.kind != TrackKind::Video {
                continue;
            }
            let (Some(before), Some(after)) = (now.get(&clip.source), then.get(&clip.source))
            else {
                continue;
            };
            let frames = timeline
                .as_ref()
                .and_then(|t| t.placements().find(|p| p.clip == clip.id))
                .map_or(0, |p| p.length);
            #[allow(clippy::cast_precision_loss)]
            let seconds = frames as f64 * rate.den as f64 / rate.num.max(1) as f64;
            match (before.eligible, after.eligible) {
                (true, false) => {
                    impact.losing_clips += 1;
                    impact.losing_seconds += seconds;
                }
                (false, true) => {
                    impact.gaining_clips += 1;
                    impact.gaining_seconds += seconds;
                }
                _ => {}
            }
            if !after.eligible {
                impact.ineligible_after += 1;
            }
        }
        Ok(impact)
    }

    #[must_use]
    pub fn history(&self) -> HistoryView {
        let entries = self
            .done
            .iter()
            .chain(self.gesture.iter())
            .chain(self.undone.iter().rev())
            .map(|entry| entry.label.clone())
            .collect();
        HistoryView {
            entries,
            applied: self.done.len() + usize::from(self.gesture.is_some()),
        }
    }
}

/// Add a gesture's latest inverses to what it has already recorded. A slider
/// drag replaces the same clips over and over: the first inverse of each
/// already restores the state before the gesture, so later ones are dropped
/// and the entry stays the size of one change.
fn coalesce(recorded: &mut Vec<Change>, inverses: Vec<Change>) {
    let only_replaces = |changes: &[Change]| {
        changes
            .iter()
            .all(|change| matches!(change, Change::ReplaceClip(_)))
    };
    if !only_replaces(recorded) || !only_replaces(&inverses) {
        recorded.extend(inverses);
        return;
    }
    for inverse in inverses {
        let known = recorded.iter().any(|change| match (change, &inverse) {
            (Change::ReplaceClip(a), Change::ReplaceClip(b)) => a.id == b.id,
            _ => false,
        });
        if !known {
            recorded.push(inverse);
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::project::Fingerprint;

    const TB: Rational = Rational { num: 1, den: 1000 };

    fn source(n: u8) -> SourceRef {
        SourceRef::for_test(
            &format!("C:\\clips\\{n}.mp4"),
            Fingerprint {
                size: u64::from(n),
                modified: None,
                content_hash: format!("{n:02x}"),
            },
        )
    }

    /// Two video tracks and an audio track; three one-second clips on the
    /// first, at 0, 30 and 90 (30 fps).
    fn document() -> Document {
        let mut project = Project::new("Trip", SequenceSettings::default());
        project.sources.insert(1, source(1));
        let clip = |id, start| {
            Clip::new(
                id,
                1,
                0,
                TB,
                start,
                vec![Operation::Trim { from: 0, to: 1000 }],
            )
        };
        project.sequence.tracks = vec![
            Track {
                id: 1,
                kind: TrackKind::Video,
                clips: vec![clip(1, 0), clip(2, 30), clip(3, 90)],
            },
            Track {
                id: 2,
                kind: TrackKind::Video,
                clips: Vec::new(),
            },
            Track {
                id: 3,
                kind: TrackKind::Audio,
                clips: Vec::new(),
            },
        ];
        Document::new(project).expect("valid")
    }

    fn json(document: &Document) -> String {
        document.project().to_json().expect("json")
    }

    fn at(selection: &[ClipId], playhead: i64) -> EditContext {
        EditContext {
            selection: selection.to_vec(),
            playhead,
        }
    }

    fn starts(document: &Document, track: usize) -> Vec<(ClipId, i64)> {
        document.project().sequence.tracks[track]
            .clips
            .iter()
            .map(|c| (c.id, c.start))
            .collect()
    }

    #[test]
    fn an_edit_is_undone_and_redone_byte_for_byte() {
        let mut document = document();
        let original = json(&document);
        document
            .apply(
                &Edit::MoveClips {
                    moves: vec![ClipMove {
                        clip: 1,
                        track: 2,
                        start: 200,
                    }],
                },
                &at(&[1], 5),
            )
            .expect("move");
        let moved = json(&document);
        assert_ne!(moved, original);
        assert_eq!(starts(&document, 1), vec![(1, 200)]);

        assert_eq!(document.undo(), Some(at(&[1], 5)));
        assert_eq!(json(&document), original);
        assert_eq!(document.redo(), Some(at(&[1], 5)));
        assert_eq!(json(&document), moved);
        assert_eq!(document.redo(), None);
    }

    #[test]
    fn moving_clips_past_each_other_is_one_edit() {
        let mut document = document();
        // Swap the first two: each lands where the other was.
        document
            .apply(
                &Edit::MoveClips {
                    moves: vec![
                        ClipMove {
                            clip: 1,
                            track: 1,
                            start: 30,
                        },
                        ClipMove {
                            clip: 2,
                            track: 1,
                            start: 0,
                        },
                    ],
                },
                &EditContext::default(),
            )
            .expect("swap");
        assert_eq!(starts(&document, 0), vec![(2, 0), (1, 30), (3, 90)]);
        assert_eq!(document.history().entries, vec!["Move 2 clips"]);
    }

    #[test]
    fn an_edit_that_would_not_evaluate_is_refused_and_changes_nothing() {
        let mut document = document();
        let original = json(&document);
        let error = document
            .apply(
                &Edit::MoveClips {
                    moves: vec![ClipMove {
                        clip: 1,
                        track: 1,
                        start: 40,
                    }],
                },
                &EditContext::default(),
            )
            .expect_err("overlaps clip 2");
        assert!(
            matches!(error, EditError::Refused(ref m) if m.contains("overlap")),
            "{error}"
        );
        assert_eq!(json(&document), original);
        assert!(document.history().entries.is_empty());

        assert_eq!(
            document.apply(
                &Edit::MoveClips {
                    moves: vec![ClipMove {
                        clip: 1,
                        track: 3,
                        start: 0
                    }]
                },
                &EditContext::default()
            ),
            Err(EditError::WrongKind {
                from: TrackKind::Video,
                to: TrackKind::Audio
            })
        );
        assert_eq!(
            document.apply(
                &Edit::RemoveClips { clips: vec![9] },
                &EditContext::default()
            ),
            Err(EditError::NoClip(9))
        );
        assert!(matches!(
            document.apply(
                &Edit::Roll {
                    left: 1,
                    right: 3,
                    frames: 2
                },
                &EditContext::default()
            ),
            Err(EditError::Refused(_))
        ));
        assert_eq!(json(&document), original);
    }

    #[test]
    fn a_new_edit_discards_what_could_be_redone() {
        let mut document = document();
        let rename = |name: &str| Edit::Rename {
            name: name.to_owned(),
        };
        document
            .apply(&rename("A"), &EditContext::default())
            .expect("a");
        document
            .apply(&rename("B"), &EditContext::default())
            .expect("b");
        document.undo();
        assert_eq!(
            document.history(),
            HistoryView {
                entries: vec!["Rename project".into(), "Rename project".into()],
                applied: 1
            }
        );
        document
            .apply(&rename("C"), &EditContext::default())
            .expect("c");
        assert_eq!(document.history().applied, 2);
        assert_eq!(document.history().entries.len(), 2);
        assert_eq!(document.redo(), None);
        assert_eq!(document.project().name, "C");
    }

    #[test]
    fn a_gesture_is_one_entry_and_stays_small() {
        let mut document = document();
        let original = json(&document);
        document.begin_gesture("Change speed", &at(&[2], 0));
        for step in 1..=100 {
            document
                .apply(
                    &Edit::SetSpeed {
                        clips: vec![2],
                        ratio: Rational {
                            num: 100 + step,
                            den: 100,
                        },
                    },
                    &at(&[2], 0),
                )
                .expect("speed");
        }
        document.end_gesture();
        assert_eq!(document.history().entries, vec!["Change speed"]);
        assert_eq!(document.done[0].changes.len(), 1, "one clip, one inverse");
        let fast = json(&document);
        document.undo();
        assert_eq!(json(&document), original);
        document.redo();
        assert_eq!(json(&document), fast);

        // A gesture that changed nothing leaves nothing.
        document.begin_gesture("Change speed", &EditContext::default());
        document.end_gesture();
        assert_eq!(document.history().entries.len(), 1);
    }

    #[test]
    fn the_history_forgets_its_oldest_entry_at_its_depth() {
        let mut document = Document::with_depth(document().project, 3).expect("valid");
        for name in ["a", "b", "c", "d"] {
            document
                .apply(
                    &Edit::Rename {
                        name: name.to_owned(),
                    },
                    &EditContext::default(),
                )
                .expect("rename");
        }
        assert_eq!(document.history().entries.len(), 3);
        while document.undo().is_some() {}
        // The first rename is beyond the depth: it stays.
        assert_eq!(document.project().name, "a");
    }

    #[test]
    fn undo_gives_back_the_selection_and_playhead_of_the_edit() {
        let mut document = document();
        let after = document
            .apply(&Edit::RemoveClips { clips: vec![2] }, &at(&[1, 2], 42))
            .expect("delete");
        assert_eq!(after.context, at(&[1], 42));
        assert_eq!(document.undo(), Some(at(&[1, 2], 42)));
        assert_eq!(starts(&document, 0), vec![(1, 0), (2, 30), (3, 90)]);
        assert_eq!(document.redo(), Some(at(&[1], 42)));
    }

    #[test]
    fn an_added_clip_is_selected_and_numbered_after_the_others() {
        let mut document = document();
        let after = document
            .apply(
                &Edit::AddClip {
                    track: 1,
                    source: 1,
                    stream: 0,
                    time_base: TB,
                    start: 60,
                    from: 0,
                    to: 500,
                },
                &at(&[1], 0),
            )
            .expect("add");
        assert_eq!(after.context.selection, vec![4]);
        assert_eq!(
            starts(&document, 0),
            vec![(1, 0), (2, 30), (4, 60), (3, 90)]
        );
        assert_eq!(
            document.apply(
                &Edit::AddClip {
                    track: 1,
                    source: 7,
                    stream: 0,
                    time_base: TB,
                    start: 400,
                    from: 0,
                    to: 1
                },
                &EditContext::default()
            ),
            Err(EditError::NoSource(7))
        );
    }

    #[test]
    fn a_track_is_added_below_its_kind() {
        let mut document = document();
        document
            .apply(
                &Edit::AddTrack {
                    kind: TrackKind::Video,
                },
                &EditContext::default(),
            )
            .expect("add");
        let kinds: Vec<(TrackId, TrackKind)> = document
            .project()
            .sequence
            .tracks
            .iter()
            .map(|t| (t.id, t.kind))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (1, TrackKind::Video),
                (2, TrackKind::Video),
                (4, TrackKind::Video),
                (3, TrackKind::Audio)
            ]
        );
    }

    #[test]
    fn importing_the_same_content_twice_adds_it_once() {
        let mut document = document();
        let ids = document.add_sources(
            &[source(1), source(2), source(2), source(3)],
            &EditContext::default(),
        );
        assert_eq!(ids, vec![1, 2, 2, 3]);
        assert_eq!(document.history().entries, vec!["Import 2 files"]);
        document.undo();
        assert_eq!(document.project().sources.len(), 1);
    }

    #[test]
    fn trim_and_speed_keep_one_operation_each_and_the_rest_in_order() {
        let clip = Clip::new(
            1,
            1,
            0,
            TB,
            0,
            vec![
                Operation::Gain { db: -3.0 },
                Operation::Trim { from: 0, to: 10 },
                Operation::Speed {
                    ratio: Rational { num: 2, den: 1 },
                },
                Operation::Trim { from: 2, to: 8 },
                Operation::Speed {
                    ratio: Rational { num: 3, den: 1 },
                },
            ],
        );
        let trimmed = retimed(&clip, 4, 6, 9);
        assert_eq!(trimmed.start, 9);
        assert_eq!(
            trimmed.operations,
            vec![
                Operation::Gain { db: -3.0 },
                Operation::Trim { from: 4, to: 6 },
                Operation::Speed {
                    ratio: Rational { num: 2, den: 1 },
                },
                Operation::Speed {
                    ratio: Rational { num: 3, den: 1 },
                },
            ]
        );
        let normal = respeeded(&trimmed, Rational { num: 2, den: 2 });
        assert_eq!(
            normal.operations,
            vec![
                Operation::Gain { db: -3.0 },
                Operation::Trim { from: 4, to: 6 },
            ]
        );
        assert_eq!(
            respeeded(&normal, Rational { num: 1, den: 2 })
                .operations
                .last(),
            Some(&Operation::Speed {
                ratio: Rational { num: 1, den: 2 }
            })
        );
    }

    fn uhd() -> StreamGeometry {
        StreamGeometry {
            width: 3840,
            height: 2160,
            frame_rate: Rational { num: 25, den: 1 },
            pixel_aspect: Rational { num: 1, den: 1 },
            variable_frame_rate: false,
            hdr: false,
        }
    }

    /// An empty project waiting for its first clip, with one video and one
    /// audio track and one source whose pictures are 4K at 25 fps.
    fn waiting() -> Document {
        let mut project = Project::matching_first_clip("New");
        project.sources.insert(1, source(1));
        project.sequence.tracks = vec![
            Track {
                id: 1,
                kind: TrackKind::Video,
                clips: Vec::new(),
            },
            Track {
                id: 2,
                kind: TrackKind::Audio,
                clips: Vec::new(),
            },
        ];
        let mut document = Document::new(project).expect("valid");
        document.describe_source(1, Some(uhd()));
        document
    }

    fn add(track: TrackId, start: i64) -> Edit {
        Edit::AddClip {
            track,
            source: 1,
            stream: 0,
            time_base: TB,
            start,
            from: 0,
            to: 1000,
        }
    }

    #[test]
    fn the_first_video_clip_gives_the_sequence_its_settings_in_the_same_edit() {
        let mut document = waiting();
        let before = json(&document);
        document
            .apply(&add(1, 0), &EditContext::default())
            .expect("add");
        let sequence = &document.project().sequence;
        assert!(!sequence.match_first_clip);
        assert_eq!(
            (sequence.settings.width, sequence.settings.height),
            (3840, 2160)
        );
        assert_eq!(sequence.settings.frame_rate, Rational { num: 25, den: 1 });
        assert!(
            document.eligibility()[&1].eligible,
            "the clip plans as copy"
        );
        assert_eq!(document.history().entries, vec!["Add clip"]);

        // One undo takes the clip and the settings back together.
        document.undo();
        assert_eq!(json(&document), before);

        // A second clip does not re-adopt.
        document.redo();
        document.describe_source(
            1,
            Some(StreamGeometry {
                width: 1280,
                height: 720,
                ..uhd()
            }),
        );
        document
            .apply(&add(1, 100), &EditContext::default())
            .expect("add");
        assert_eq!(document.project().sequence.settings.width, 3840);
    }

    #[test]
    fn only_a_video_clip_with_a_usable_shape_is_adopted() {
        let mut document = waiting();
        document
            .apply(&add(2, 0), &EditContext::default())
            .expect("add");
        assert!(
            document.project().sequence.match_first_clip,
            "audio is not a picture"
        );

        let mut odd = waiting();
        odd.describe_source(
            1,
            Some(StreamGeometry {
                width: 1081,
                ..uhd()
            }),
        );
        odd.apply(&add(1, 0), &EditContext::default()).expect("add");
        assert!(odd.project().sequence.match_first_clip);
        assert_eq!(odd.project().sequence.settings, SequenceSettings::default());
        assert!(!odd.eligibility()[&1].eligible);

        let mut unknown = waiting();
        unknown.describe_source(1, None);
        unknown
            .apply(&add(1, 0), &EditContext::default())
            .expect("add");
        assert!(unknown.project().sequence.match_first_clip);
        assert!(unknown.eligibility().is_empty());
    }

    #[test]
    fn choosing_settings_stops_the_wait_and_is_undoable() {
        let mut document = waiting();
        let before = json(&document);
        let chosen = SequenceSettings {
            width: 1280,
            height: 720,
            ..SequenceSettings::default()
        };
        document
            .apply(
                &Edit::SetSettings { settings: chosen },
                &EditContext::default(),
            )
            .expect("set");
        assert!(!document.project().sequence.match_first_clip);
        assert_eq!(document.project().sequence.settings, chosen);
        document.undo();
        assert_eq!(json(&document), before);

        let odd = SequenceSettings {
            width: 1279,
            ..SequenceSettings::default()
        };
        assert!(matches!(
            document.apply(
                &Edit::SetSettings { settings: odd },
                &EditContext::default()
            ),
            Err(EditError::Settings(SettingsError::Size { .. }))
        ));
        assert_eq!(json(&document), before);
    }

    #[test]
    fn a_settings_change_states_what_it_costs_before_it_is_made() {
        let mut document = waiting();
        document
            .apply(&add(1, 0), &EditContext::default())
            .expect("add");
        document
            .apply(&add(1, 50), &EditContext::default())
            .expect("add");
        let hd = SequenceSettings {
            width: 1920,
            height: 1080,
            frame_rate: Rational { num: 25, den: 1 },
            ..SequenceSettings::default()
        };
        let impact = document.settings_impact(&hd).expect("valid");
        assert_eq!(impact.losing_clips, 2);
        assert_eq!(impact.ineligible_after, 2);
        // Two one-second clips at 25 fps.
        assert!((impact.losing_seconds - 2.0).abs() < 1e-9, "{impact:?}");
        assert_eq!(impact.gaining_clips, 0);
        // Nothing was changed by asking.
        assert_eq!(document.project().sequence.settings.width, 3840);
        let same = document.project().sequence.settings;
        assert_eq!(
            document.settings_impact(&same).expect("valid"),
            SettingsImpact::default()
        );
        assert!(
            document
                .settings_impact(&SequenceSettings {
                    frame_rate: Rational { num: 0, den: 1 },
                    ..same
                })
                .is_err()
        );
    }

    /// The standard document with source 1's stream 0 known to run 0..1500
    /// ticks: every clip (0..1000) has 500 ticks — 15 frames — after it and
    /// none before.
    fn bounded() -> Document {
        let mut document = document();
        document.describe_streams(
            1,
            &[StreamExtent {
                source: 1,
                stream: 0,
                time_base: TB,
                start: 0,
                end: 1500,
            }],
        );
        document
    }

    fn edge(clip: ClipId, edge: Edge, frames: i64, ripple: bool) -> Edit {
        Edit::TrimEdge {
            clip,
            edge,
            frames,
            ripple,
        }
    }

    fn spans(document: &Document, track: usize) -> Vec<(ClipId, i64, i64)> {
        let timeline = evaluate(document.project()).expect("evaluates");
        timeline.tracks[track]
            .placements
            .iter()
            .map(|p| (p.clip, p.start, p.end()))
            .collect()
    }

    #[test]
    fn a_trim_stops_at_the_end_of_its_source_and_says_so() {
        let mut document = bounded();
        // Clip 3 (90..120) wants 40 more frames; the source has 15.
        let applied = document
            .apply(&edge(3, Edge::End, 40, false), &EditContext::default())
            .expect("trim");
        assert!(applied.clamped);
        assert_eq!(spans(&document, 0)[2], (3, 90, 135));
        // Nothing before the in-point: the start cannot move earlier at all.
        let applied = document
            .apply(&edge(3, Edge::Start, -5, false), &EditContext::default())
            .expect("trim");
        assert!(applied.clamped);
        assert_eq!(spans(&document, 0)[2], (3, 90, 135));
        // Within the source, nothing is clamped.
        let applied = document
            .apply(&edge(3, Edge::End, -10, false), &EditContext::default())
            .expect("trim");
        assert!(!applied.clamped);
        assert_eq!(spans(&document, 0)[2], (3, 90, 125));
        assert_eq!(document.history().entries.len(), 2, "one entry each");
    }

    #[test]
    fn a_trim_stops_at_its_neighbour_and_a_ripple_trim_moves_it() {
        let mut document = bounded();
        // Clip 1 (0..30) ends where clip 2 starts: it cannot grow.
        let applied = document
            .apply(&edge(1, Edge::End, 5, false), &EditContext::default())
            .expect("trim");
        assert!(applied.clamped);
        assert_eq!(spans(&document, 0)[0], (1, 0, 30));
        // Rippled, it grows and everything after it on the track follows.
        document
            .apply(&edge(1, Edge::End, 5, true), &EditContext::default())
            .expect("ripple");
        assert_eq!(
            spans(&document, 0),
            vec![(1, 0, 35), (2, 35, 65), (3, 95, 125)]
        );
        // A ripple trim of a start keeps the clip where it is and pulls the
        // rest in.
        document
            .apply(&edge(2, Edge::Start, 10, true), &EditContext::default())
            .expect("ripple");
        assert_eq!(
            spans(&document, 0),
            vec![(1, 0, 35), (2, 35, 55), (3, 85, 115)]
        );
        assert_eq!(
            document.history().entries,
            vec!["Ripple trim", "Ripple trim"]
        );
        document.undo();
        document.undo();
        assert_eq!(
            spans(&document, 0),
            vec![(1, 0, 30), (2, 30, 60), (3, 90, 120)]
        );
    }

    #[test]
    fn a_ripple_delete_moves_every_later_clip_by_what_was_removed_before_it() {
        let mut document = bounded();
        let applied = document
            .apply(
                &Edit::RippleDelete { clips: vec![1, 3] },
                &at(&[1, 2, 3], 0),
            )
            .expect("ripple delete");
        assert_eq!(applied.context.selection, vec![2]);
        // Clip 2 closes the 30-frame gap of clip 1; the gap after it stays.
        assert_eq!(spans(&document, 0), vec![(2, 0, 30)]);
        document.undo();
        document
            .apply(
                &Edit::RippleDelete { clips: vec![2] },
                &EditContext::default(),
            )
            .expect("ripple delete");
        assert_eq!(spans(&document, 0), vec![(1, 0, 30), (3, 60, 90)]);
        assert_eq!(document.history().entries.len(), 1);
    }

    #[test]
    fn a_roll_moves_the_cut_and_nothing_else() {
        let mut document = bounded();
        document
            .apply(
                &Edit::Roll {
                    left: 1,
                    right: 2,
                    frames: 6,
                },
                &EditContext::default(),
            )
            .expect("roll");
        let after = spans(&document, 0);
        assert_eq!(after[0], (1, 0, 36));
        assert_eq!(after[1], (2, 36, 60));
        // Earlier than the right clip's source allows: nothing before its
        // in-point, so the cut cannot move back past where it started.
        let applied = document
            .apply(
                &Edit::Roll {
                    left: 1,
                    right: 2,
                    frames: -20,
                },
                &EditContext::default(),
            )
            .expect("roll");
        assert!(applied.clamped);
        let back = spans(&document, 0);
        assert_eq!(back[0], (1, 0, 30));
        assert_eq!(back[1], (2, 30, 60));
    }

    #[test]
    fn numeric_entry_and_a_drag_make_the_same_graph() {
        // The inspector sends the same edit a handle drag commits: a number
        // of frames. There is one path, so the results cannot differ.
        let mut dragged = bounded();
        let mut typed = bounded();
        dragged
            .apply(&edge(2, Edge::Start, 4, false), &EditContext::default())
            .expect("drag");
        typed
            .apply(&edge(2, Edge::Start, 4, false), &EditContext::default())
            .expect("typed");
        assert_eq!(json(&dragged), json(&typed));
    }

    /// xorshift64*, seeded, so a failing sequence reproduces.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }

        fn int(&mut self, bound: u64) -> i64 {
            i64::try_from(self.below(bound)).expect("small")
        }

        fn pick<T: Copy>(&mut self, items: &[T]) -> Option<T> {
            if items.is_empty() {
                return None;
            }
            let index = usize::try_from(self.below(items.len() as u64)).expect("small");
            items.get(index).copied()
        }
    }

    fn random_edit(random: &mut Random, document: &Document) -> Edit {
        let project = document.project();
        let clips: Vec<ClipId> = project.clips().map(|(_, c)| c.id).collect();
        let tracks: Vec<TrackId> = project.sequence.tracks.iter().map(|t| t.id).collect();
        let clip = random.pick(&clips).unwrap_or(1);
        let track = random.pick(&tracks).unwrap_or(1);
        match random.below(7) {
            0 => Edit::Rename {
                name: format!("n{}", random.below(5)),
            },
            1 => Edit::AddTrack {
                kind: if random.below(2) == 0 {
                    TrackKind::Video
                } else {
                    TrackKind::Audio
                },
            },
            2 => {
                let from = random.int(5000);
                Edit::AddClip {
                    track,
                    source: 1,
                    stream: 0,
                    time_base: TB,
                    start: random.int(600),
                    from,
                    to: from + 1 + random.int(3000),
                }
            }
            3 => Edit::MoveClips {
                moves: (0..=random.below(2))
                    .map(|_| ClipMove {
                        clip: random.pick(&clips).unwrap_or(1),
                        track: random.pick(&tracks).unwrap_or(1),
                        start: random.int(600),
                    })
                    .collect(),
            },
            4 => Edit::TrimEdge {
                clip,
                edge: if random.below(2) == 0 {
                    Edge::Start
                } else {
                    Edge::End
                },
                frames: random.int(80) - 40,
                ripple: random.below(2) == 0,
            },
            5 => Edit::SetSpeed {
                clips: vec![clip],
                ratio: Rational {
                    num: 1 + random.int(4),
                    den: 1 + random.int(4),
                },
            },
            _ => Edit::RemoveClips { clips: vec![clip] },
        }
    }

    #[test]
    fn any_sequence_of_edits_fully_undone_restores_the_graph_byte_for_byte() {
        let mut random = Random(0x5eed_0fed_1700);
        for case in 0..300 {
            let mut document = document();
            let original = json(&document);
            let mut states = vec![original.clone()];
            for _ in 0..random.below(40) {
                let edit = random_edit(&mut random, &document);
                if random.below(6) == 0 {
                    document.undo();
                    states.pop();
                    if states.is_empty() {
                        states.push(original.clone());
                    }
                    assert_eq!(states.last(), Some(&json(&document)), "case {case}");
                    continue;
                }
                let before = json(&document);
                match document.apply(&edit, &EditContext::default()) {
                    Ok(_) => {
                        let after = json(&document);
                        if after != before {
                            states.push(after);
                        }
                    }
                    Err(_) => assert_eq!(json(&document), before, "case {case}: {edit:?}"),
                }
                evaluate(document.project())
                    .unwrap_or_else(|error| panic!("case {case}: {edit:?}: {error}"));
            }
            let applied = document.history().applied;
            while document.undo().is_some() {}
            assert_eq!(json(&document), original, "case {case}");
            for _ in 0..applied {
                document.redo();
            }
            assert_eq!(Some(&json(&document)), states.last(), "case {case}");
        }
    }
}
