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

use super::evaluate::{EvaluatedTrack, Motion, Placement, Timeline, evaluate};
use super::settings::{CopyEligibility, SettingsError, StreamGeometry, copy_eligibility};
use super::split::halves;
use super::trim::{Edge, StreamExtent, reach, retimed, trim};
use super::{
    Clip, ClipId, Operation, Project, ProjectError, SequenceSettings, SourceId, SourceRef, Track,
    TrackId, TrackKind,
};
use crate::probe::Rational;
use crate::time::{Rounding, rescale};

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
    /// Cut clips in two at sequence frame `at` (#35): the listed clips, or
    /// with none listed every clip under `at`. The left half keeps the id.
    Split {
        clips: Vec<ClipId>,
        #[ts(type = "number")]
        at: i64,
    },
    /// Put two halves of one cut back together: `right` must continue
    /// `left` in the source and on the timeline.
    Join {
        left: ClipId,
        right: ClipId,
    },
    /// Copy clips, each right after itself; later clips on the track move to
    /// make room.
    Duplicate {
        clips: Vec<ClipId>,
    },
    /// Hold the frame at `at` for `frames` sequence frames, inserted there;
    /// the rest of the track moves later by as much (#35).
    FreezeFrame {
        clip: ClipId,
        #[ts(type = "number")]
        at: i64,
        #[ts(type = "number")]
        frames: i64,
    },
    /// Play clips backwards, or forwards again (#35).
    SetReverse {
        clips: Vec<ClipId>,
        reverse: bool,
    },
    /// Remove a track and every clip on it (#36).
    RemoveTrack {
        track: TrackId,
    },
    /// Move a track to `index` in the track list: its compositing place,
    /// for a video track. Video tracks stay above audio ones.
    MoveTrack {
        track: TrackId,
        index: u32,
    },
    RenameTrack {
        track: TrackId,
        name: String,
    },
    /// Set a track's switches; those left `None` are unchanged.
    SetTrack {
        track: TrackId,
        muted: Option<bool>,
        solo: Option<bool>,
        locked: Option<bool>,
        collapsed: Option<bool>,
    },
    /// Give video clips' sound a clip of its own on an audio track, linked
    /// to the pictures. Nothing is copied or encoded: the new clip plays the
    /// same file's sound.
    DetachAudio {
        clips: Vec<ClipId>,
    },
    /// Stop clips moving and deleting with the clips linked to them.
    Unlink {
        clips: Vec<ClipId>,
    },
    /// Remove a source from the project, and every clip of it (#53). The
    /// file is not touched: only the reference goes.
    RemoveSource {
        source: SourceId,
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
            Self::Split { .. } => "Split".to_owned(),
            Self::Join { .. } => "Join clips".to_owned(),
            Self::Duplicate { clips: ids } => clips(ids.len(), "Duplicate clip", "Duplicate"),
            Self::FreezeFrame { .. } => "Freeze frame".to_owned(),
            Self::SetReverse { reverse: true, .. } => "Reverse".to_owned(),
            Self::SetReverse { reverse: false, .. } => "Play forwards".to_owned(),
            Self::RemoveTrack { .. } => "Remove track".to_owned(),
            Self::MoveTrack { .. } => "Move track".to_owned(),
            Self::RenameTrack { .. } => "Rename track".to_owned(),
            Self::SetTrack { muted: Some(_), .. } => "Mute track".to_owned(),
            Self::SetTrack { solo: Some(_), .. } => "Solo track".to_owned(),
            Self::SetTrack {
                locked: Some(_), ..
            } => "Lock track".to_owned(),
            Self::SetTrack { .. } => "Change track".to_owned(),
            Self::DetachAudio { clips: ids } => clips(ids.len(), "Detach audio", "Detach audio of"),
            Self::Unlink { clips: ids } => clips(ids.len(), "Unlink clip", "Unlink"),
            Self::RemoveSource { .. } => "Remove from project".to_owned(),
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
    /// A locked track refuses every edit to its clips (#36).
    #[error("track {0} is locked: unlock it to change its clips")]
    Locked(TrackId),
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
    /// Set a track's name and switches.
    Header(TrackId, Header),
}

/// A track's name and switches: everything of it but its clips.
#[derive(Debug, Clone, PartialEq, Eq)]
// The track's four switches, one for one.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct Header {
    name: String,
    muted: bool,
    solo: bool,
    locked: bool,
    collapsed: bool,
}

impl Header {
    fn of(track: &Track) -> Self {
        Self {
            name: track.name.clone(),
            muted: track.muted,
            solo: track.solo,
            locked: track.locked,
            collapsed: track.collapsed,
        }
    }
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
            Self::Header(id, header) => {
                let track = track_mut(project, id)?;
                let previous = Header::of(track);
                track.name = header.name;
                track.muted = header.muted;
                track.solo = header.solo;
                track.locked = header.locked;
                track.collapsed = header.collapsed;
                Self::Header(id, previous)
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
    /// What the library shows of each source (#53).
    assets: BTreeMap<SourceId, super::asset::AssetInfo>,
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
    // Linked clips move and delete together (#36): the edit is widened to
    // them before it is compiled.
    let widened = widen(project, edit)?;
    let edit = widened.as_ref().unwrap_or(edit);
    match edit {
        Edit::DetachAudio { clips } => return detach_audio(project, facts, clips, kept),
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
        Edit::Split { clips, at } => return split(project, clips, *at),
        Edit::Join { left, right } => return join(project, *left, *right, kept),
        Edit::Duplicate { clips } => return duplicate(project, clips),
        Edit::FreezeFrame { clip, at, frames } => {
            return freeze_frame(project, *clip, *at, *frames);
        }
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
        Edit::AddClip { .. } => add_clip(project, facts, edit)?,
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
        Edit::SetReverse { clips, reverse } => (set_reverse(project, clips, *reverse)?, kept),
        Edit::RemoveTrack { .. }
        | Edit::MoveTrack { .. }
        | Edit::RenameTrack { .. }
        | Edit::SetTrack { .. }
        | Edit::Unlink { .. } => return track_edit(project, edit, kept),
        Edit::RemoveSource { source } => {
            if !project.sources.contains_key(source) {
                return Err(EditError::NoSource(*source));
            }
            let clips = project.clips_of(*source);
            let mut changes: Vec<Change> = clips.iter().map(|&id| Change::RemoveClip(id)).collect();
            changes.push(Change::Source(*source, None));
            let remaining = kept.into_iter().filter(|c| !clips.contains(c)).collect();
            return Ok(Compiled {
                changes,
                selection: remaining,
                clamped: false,
            });
        }
        Edit::TrimEdge { .. }
        | Edit::Roll { .. }
        | Edit::RippleDelete { .. }
        | Edit::Split { .. }
        | Edit::Join { .. }
        | Edit::Duplicate { .. }
        | Edit::FreezeFrame { .. }
        | Edit::DetachAudio { .. } => unreachable!("compiled above"),
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

/// `clip` with a held frame lasting `frames` instead: its `Freeze`
/// operation is the length.
fn held(clip: &Clip, frames: i64) -> Clip {
    let operations = clip
        .operations
        .iter()
        .map(|operation| match operation {
            Operation::Freeze { .. } => Operation::Freeze { frames },
            other => *other,
        })
        .collect();
    Clip {
        operations,
        ..clip.clone()
    }
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
    changes.extend(shift_after(project, track, id, after, shift)?);
    Ok(changes)
}

/// Every clip of `track` but `id` starting at or after `after`, moved by
/// `shift` frames.
fn shift_after(
    project: &Project,
    track: &EvaluatedTrack,
    id: ClipId,
    after: i64,
    shift: i64,
) -> Result<Vec<Change>, EditError> {
    let mut changes = Vec::new();
    if shift == 0 {
        return Ok(changes);
    }
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
    Ok(changes)
}

/// The next free clip id.
fn next_id(project: &Project) -> ClipId {
    project.clips().map(|(_, c)| c.id).max().unwrap_or(0) + 1
}

/// `clip` as one half of a split: its trim, its start, and — if it holds a
/// frame — its length.
fn half_of(clip: &Clip, placement: &Placement, half: super::split::Half) -> Clip {
    let retimed = retimed(clip, half.from, half.to, half.start);
    if placement.motion == Some(Motion::Hold) {
        held(&retimed, half.length)
    } else {
        retimed
    }
}

fn split(project: &Project, clips: &[ClipId], at: i64) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let wanted: BTreeSet<ClipId> = clips.iter().copied().collect();
    for &id in &wanted {
        placed(&timeline, id)?;
    }
    let mut changes = Vec::new();
    let mut selection = Vec::new();
    let mut id = next_id(project);
    for track in &timeline.tracks {
        for placement in &track.placements {
            if !wanted.is_empty() && !wanted.contains(&placement.clip) {
                continue;
            }
            let Some((left, right)) = halves(placement, at) else {
                continue;
            };
            let (_, clip) = find(project, placement.clip)?;
            changes.push(Change::ReplaceClip(half_of(clip, placement, left)));
            changes.push(Change::InsertClip {
                track: track.id,
                index: None,
                clip: Clip {
                    id,
                    ..half_of(clip, placement, right)
                },
            });
            selection.push(id);
            id += 1;
        }
    }
    if changes.is_empty() {
        return Err(EditError::Refused(format!(
            "nothing to split at frame {at}: no clip runs across it"
        )));
    }
    Ok(Compiled {
        changes,
        selection,
        clamped: false,
    })
}

fn join(
    project: &Project,
    left: ClipId,
    right: ClipId,
    kept: Vec<ClipId>,
) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let (left_track, a) = placed(&timeline, left)?;
    let (right_track, b) = placed(&timeline, right)?;
    let (_, a_clip) = find(project, left)?;
    let (_, b_clip) = find(project, right)?;
    let refused = || {
        Err(EditError::Refused(format!(
            "clips {left} and {right} are not two halves of one cut"
        )))
    };
    let same = left_track.id == right_track.id
        && a.end() == b.start
        && a.source == b.source
        && a.stream == b.stream
        && a.time_base == b.time_base
        && a.speed == b.speed
        && a.motion == b.motion
        && a.audio == b.audio;
    if !same {
        return refused();
    }
    let joined = match a.motion {
        Some(Motion::Hold) if a.source_in == b.source_in => held(a_clip, a.length + b.length),
        // Backwards, the right half plays the source before the left.
        Some(Motion::Reverse) if b.source_out == a.source_in => {
            retimed(a_clip, b.source_in, a.source_out, a.start)
        }
        None if a.source_out == b.source_in => retimed(a_clip, a.source_in, b.source_out, a.start),
        _ => return refused(),
    };
    let selection = kept.into_iter().filter(|&c| c != right).collect();
    let _ = b_clip;
    Ok(Compiled {
        changes: vec![Change::RemoveClip(right), Change::ReplaceClip(joined)],
        selection,
        clamped: false,
    })
}

fn duplicate(project: &Project, clips: &[ClipId]) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let wanted: BTreeSet<ClipId> = clips.iter().copied().collect();
    for &id in &wanted {
        placed(&timeline, id)?;
    }
    let mut changes = Vec::new();
    let mut copies = Vec::new();
    let mut id = next_id(project);
    for track in &timeline.tracks {
        // Walk the track in order: each copy goes right after its original,
        // and everything after moves by the copies placed before it.
        let mut shift = 0;
        let mut inserts = Vec::new();
        for placement in &track.placements {
            let (_, clip) = find(project, placement.clip)?;
            if shift != 0 {
                changes.push(Change::ReplaceClip(Clip {
                    start: placement.start + shift,
                    ..clip.clone()
                }));
            }
            if wanted.contains(&placement.clip) {
                inserts.push(Change::InsertClip {
                    track: track.id,
                    index: None,
                    clip: Clip {
                        id,
                        start: placement.end() + shift,
                        ..clip.clone()
                    },
                });
                copies.push(id);
                id += 1;
                shift += placement.length;
            }
        }
        changes.extend(inserts);
    }
    Ok(Compiled {
        changes,
        selection: copies,
        clamped: false,
    })
}

fn freeze_frame(
    project: &Project,
    id: ClipId,
    at: i64,
    frames: i64,
) -> Result<Compiled, EditError> {
    if frames < 1 {
        return Err(EditError::Refused(
            "a freeze frame lasts at least one frame".to_owned(),
        ));
    }
    let timeline = timeline_of(project)?;
    let (track, placement) = placed(&timeline, id)?;
    if track.kind != TrackKind::Video {
        return Err(EditError::Refused(
            "only pictures can be frozen: this clip is on an audio track".to_owned(),
        ));
    }
    let tick = placement
        .source_at(at)
        .ok_or_else(|| EditError::Refused(format!("clip {id} is not on screen at frame {at}")))?;
    let (_, clip) = find(project, id)?;
    let mut next = next_id(project);
    let mut changes = shift_after(project, track, id, placement.end(), frames)?;
    // The rest of the clip, after the hold.
    if let Some((left, right)) = halves(placement, at) {
        changes.push(Change::ReplaceClip(half_of(clip, placement, left)));
        changes.push(Change::InsertClip {
            track: track.id,
            index: None,
            clip: Clip {
                id: next,
                start: right.start + frames,
                ..half_of(clip, placement, right)
            },
        });
        next += 1;
    } else {
        // At the clip's first frame: the whole clip moves after the hold.
        changes.push(Change::ReplaceClip(Clip {
            start: placement.start + frames,
            ..clip.clone()
        }));
    }
    let hold = Clip::new(
        next,
        clip.source,
        clip.stream,
        clip.time_base,
        at,
        vec![
            Operation::Trim {
                from: tick,
                to: tick + 1,
            },
            Operation::Freeze { frames },
        ],
    );
    changes.push(Change::InsertClip {
        track: track.id,
        index: None,
        clip: hold,
    });
    Ok(Compiled {
        changes,
        selection: vec![next],
        clamped: false,
    })
}

fn set_reverse(
    project: &Project,
    clips: &[ClipId],
    reverse: bool,
) -> Result<Vec<Change>, EditError> {
    let mut changes = Vec::new();
    for &id in &clips.iter().copied().collect::<BTreeSet<_>>() {
        let (track, clip) = find(project, id)?;
        if track.kind != TrackKind::Video {
            return Err(EditError::Refused(format!(
                "clip {id} is sound: reversing it is not supported"
            )));
        }
        let has = clip.operations.contains(&Operation::Reverse);
        if has == reverse {
            continue;
        }
        let mut operations: Vec<Operation> = clip
            .operations
            .iter()
            .filter(|operation| **operation != Operation::Reverse)
            .copied()
            .collect();
        if reverse {
            operations.push(Operation::Reverse);
        }
        changes.push(Change::ReplaceClip(Clip {
            operations,
            ..clip.clone()
        }));
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
    let trimmed = trim_moving(placement, facts, edge, frames, room).ok_or_else(|| too_long(id))?;
    if placement.motion == Some(Motion::Hold) {
        let (_, current) = find(project, id)?;
        let start = if ripple {
            placement.start
        } else {
            trimmed.start
        };
        let mut changes = vec![Change::ReplaceClip(Clip {
            start,
            ..held(current, trimmed.length)
        })];
        if ripple {
            changes.extend(shift_after(
                project,
                track,
                id,
                placement.end(),
                trimmed.length - placement.length,
            )?);
        }
        return Ok(Compiled {
            changes: if trimmed.length == placement.length && start == placement.start {
                Vec::new()
            } else {
                changes
            },
            selection: kept,
            clamped: trimmed.clamped,
        });
    }
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

/// A trim of `placement` however it moves through its source: a hold changes
/// only its length, a reverse trims the other edge of the source.
fn trim_moving(
    placement: &Placement,
    facts: &Facts,
    edge: Edge,
    frames: i64,
    room: (Option<i64>, Option<i64>),
) -> Option<super::trim::Trimmed> {
    match placement.motion {
        Some(Motion::Hold) => Some(trim_hold(placement, edge, frames, room)),
        Some(Motion::Reverse) => {
            // Backwards, the timeline's start is the source's end: trim the
            // other edge of the source, with the room mirrored to match.
            let end = placement.end();
            let (mirrored, room) = match edge {
                Edge::Start => (
                    Edge::End,
                    (None, room.0.map(|b| end + (placement.start - b))),
                ),
                Edge::End => (
                    Edge::Start,
                    (room.1.map(|a| placement.start - (a - end)), None),
                ),
            };
            trim(
                placement,
                mirrored,
                -frames,
                facts.extent_of(placement),
                room,
            )
            .map(|t| super::trim::Trimmed {
                start: match edge {
                    Edge::Start => end - t.length,
                    Edge::End => placement.start,
                },
                ..t
            })
        }
        None => trim(placement, edge, frames, facts.extent_of(placement), room),
    }
}

/// A trim of a held frame: only its length changes, never its source.
fn trim_hold(
    placement: &Placement,
    edge: Edge,
    frames: i64,
    room: (Option<i64>, Option<i64>),
) -> super::trim::Trimmed {
    let (start, length) = match edge {
        Edge::Start => {
            let mut delta = frames.min(placement.length - 1);
            if let Some(earliest) = room.0 {
                delta = delta.max(earliest - placement.start);
            }
            (placement.start + delta, placement.length - delta)
        }
        Edge::End => {
            let mut delta = frames.max(1 - placement.length);
            if let Some(latest) = room.1 {
                delta = delta.min(latest - placement.end());
            }
            (placement.start, placement.length + delta)
        }
    };
    super::trim::Trimmed {
        from: placement.source_in,
        to: placement.source_out,
        start,
        length,
        clamped: length - placement.length
            != match edge {
                Edge::Start => -frames,
                Edge::End => frames,
            },
    }
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
    if a.motion.is_some() || b.motion.is_some() {
        return Err(EditError::Refused(
            "a roll moves a cut between clips that play forwards".to_owned(),
        ));
    }
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

/// The edits to tracks and links (#36), compiled.
fn track_edit(project: &Project, edit: &Edit, kept: Vec<ClipId>) -> Result<Compiled, EditError> {
    Ok(Compiled::from(match edit {
        Edit::RemoveTrack { track } => {
            find_track(project, *track)?;
            let remaining = kept
                .into_iter()
                .filter(|c| find(project, *c).is_ok_and(|(t, _)| t.id != *track))
                .collect();
            (vec![Change::RemoveTrack(*track)], remaining)
        }
        Edit::MoveTrack { track, index } => (move_track(project, *track, *index)?, kept),
        Edit::RenameTrack { track, name } => {
            let current = find_track(project, *track)?;
            let header = Header {
                name: name.trim().to_owned(),
                ..Header::of(current)
            };
            (header_change(current, header), kept)
        }
        Edit::SetTrack {
            track,
            muted,
            solo,
            locked,
            collapsed,
        } => {
            let current = find_track(project, *track)?;
            let now = Header::of(current);
            let header = Header {
                muted: muted.unwrap_or(now.muted),
                solo: solo.unwrap_or(now.solo),
                locked: locked.unwrap_or(now.locked),
                collapsed: collapsed.unwrap_or(now.collapsed),
                ..now
            };
            (header_change(current, header), kept)
        }
        Edit::Unlink { clips } => {
            let mut changes = Vec::new();
            for id in with_links(project, clips) {
                let (_, clip) = find(project, id)?;
                if clip.link.is_some() {
                    changes.push(Change::ReplaceClip(Clip {
                        link: None,
                        ..clip.clone()
                    }));
                }
            }
            (changes, kept)
        }
        _ => (Vec::new(), kept),
    }))
}

/// `edit` with the clips linked to the ones it moves or deletes added, or
/// `None` when it is not an edit that follows links.
fn widen(project: &Project, edit: &Edit) -> Result<Option<Edit>, EditError> {
    Ok(match edit {
        Edit::MoveClips { moves } => Some(Edit::MoveClips {
            moves: with_linked_moves(project, moves)?,
        }),
        Edit::RemoveClips { clips } => Some(Edit::RemoveClips {
            clips: with_links(project, clips),
        }),
        Edit::RippleDelete { clips } => Some(Edit::RippleDelete {
            clips: with_links(project, clips),
        }),
        _ => None,
    })
}

/// `clips` and every clip linked to one of them, each once, in order.
fn with_links(project: &Project, clips: &[ClipId]) -> Vec<ClipId> {
    let links: BTreeSet<ClipId> = clips
        .iter()
        .filter_map(|&id| find(project, id).ok().and_then(|(_, c)| c.link))
        .collect();
    let mut all: Vec<ClipId> = clips.to_vec();
    for (_, clip) in project.clips() {
        if clip.link.is_some_and(|link| links.contains(&link)) && !all.contains(&clip.id) {
            all.push(clip.id);
        }
    }
    all
}

/// `moves` and, for every clip linked to a moving one, the same move in
/// time on its own track — unless it is moved explicitly.
fn with_linked_moves(project: &Project, moves: &[ClipMove]) -> Result<Vec<ClipMove>, EditError> {
    let mut all = moves.to_vec();
    for step in moves {
        let (_, clip) = find(project, step.clip)?;
        let Some(link) = clip.link else { continue };
        let delta = step.start - clip.start;
        for (track, partner) in project.clips() {
            if partner.link == Some(link) && !all.iter().any(|m| m.clip == partner.id) {
                all.push(ClipMove {
                    clip: partner.id,
                    track: track.id,
                    start: partner.start + delta,
                });
            }
        }
    }
    Ok(all)
}

fn header_change(track: &Track, header: Header) -> Vec<Change> {
    if header == Header::of(track) {
        Vec::new()
    } else {
        vec![Change::Header(track.id, header)]
    }
}

/// Move `id` to `index` of the track list, keeping video tracks above audio.
fn move_track(project: &Project, id: TrackId, index: u32) -> Result<Vec<Change>, EditError> {
    let tracks = &project.sequence.tracks;
    let from = tracks
        .iter()
        .position(|t| t.id == id)
        .ok_or(EditError::NoTrack(id))?;
    let kind = find_track(project, id)?.kind;
    let videos = tracks.iter().filter(|t| t.kind == TrackKind::Video).count();
    let wanted = usize::try_from(index).unwrap_or(usize::MAX);
    let to = match kind {
        TrackKind::Video => wanted.min(videos.saturating_sub(1)),
        TrackKind::Audio => wanted.clamp(videos, tracks.len().saturating_sub(1)),
    };
    Ok(if to == from {
        Vec::new()
    } else {
        vec![
            Change::RemoveTrack(id),
            Change::InsertTrack(to, find_track(project, id)?.clone()),
        ]
    })
}

/// Detach the sound of each video clip in `clips` (#36).
fn detach_audio(
    project: &Project,
    facts: &Facts,
    clips: &[ClipId],
    kept: Vec<ClipId>,
) -> Result<Compiled, EditError> {
    let timeline = timeline_of(project)?;
    let mut changes = Vec::new();
    let mut selection = kept;
    let mut next = next_id(project);
    let mut next_track = project
        .sequence
        .tracks
        .iter()
        .map(|t| t.id)
        .max()
        .unwrap_or(0)
        + 1;
    // Audio tracks as they will be, with what is being placed on them.
    let mut lanes: Vec<(TrackId, Vec<(i64, i64)>)> = timeline
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| {
            (
                t.id,
                t.placements.iter().map(|p| (p.start, p.end())).collect(),
            )
        })
        .collect();
    for &id in &clips.iter().copied().collect::<BTreeSet<_>>() {
        let (track, placement) = placed(&timeline, id)?;
        let (_, clip) = find(project, id)?;
        if track.kind != TrackKind::Video || clip.detached {
            continue;
        }
        if placement.motion.is_some() {
            return Err(EditError::Refused(format!(
                "clip {id} is held or reversed: it has no sound to detach"
            )));
        }
        let Some(sound) = facts
            .extents
            .values()
            .find(|e| e.source == clip.source && e.kind == TrackKind::Audio)
        else {
            return Err(EditError::Refused(format!(
                "clip {id}'s file has no sound to detach"
            )));
        };
        let (from, to) = sound_range(placement, sound)?;
        let (start, end) = (placement.start, placement.end());
        let lane = lanes
            .iter_mut()
            .find(|(_, taken)| taken.iter().all(|&(a, b)| end <= a || b <= start));
        let lane_id = if let Some((lane_id, taken)) = lane {
            taken.push((start, end));
            *lane_id
        } else {
            let track = Track::new(next_track, TrackKind::Audio, Vec::new());
            changes.push(Change::InsertTrack(project.sequence.tracks.len(), track));
            lanes.push((next_track, vec![(start, end)]));
            next_track += 1;
            next_track - 1
        };
        // The pictures keep what shapes them; the sound takes the speed and
        // the audio chain with it.
        let (picture_ops, sound_ops): (Vec<Operation>, Vec<Operation>) = clip
            .operations
            .iter()
            .partition(|operation| super::evaluate::audio_operation(operation).is_none());
        let mut audio_ops = vec![Operation::Trim { from, to }];
        audio_ops.extend(
            picture_ops
                .iter()
                .filter(|operation| matches!(operation, Operation::Speed { .. }))
                .copied(),
        );
        audio_ops.extend(sound_ops);
        changes.push(Change::ReplaceClip(Clip {
            detached: true,
            link: Some(clip.link.unwrap_or(id)),
            operations: picture_ops,
            ..clip.clone()
        }));
        changes.push(Change::InsertClip {
            track: lane_id,
            index: None,
            clip: Clip {
                id: next,
                source: clip.source,
                stream: sound.stream,
                time_base: sound.time_base,
                start,
                detached: false,
                link: Some(clip.link.unwrap_or(id)),
                operations: audio_ops,
            },
        });
        selection.push(next);
        next += 1;
    }
    Ok(Compiled {
        changes,
        selection,
        clamped: false,
    })
}

/// The ticks of `sound` that play under `placement`'s pictures: the same
/// moments of the file, in the sound's own time base, lasting exactly as
/// many frames as the pictures.
fn sound_range(placement: &Placement, sound: &StreamExtent) -> Result<(i64, i64), EditError> {
    let id = placement.clip;
    let played = Rational {
        num: sound.time_base.num.saturating_mul(placement.speed.den),
        den: sound.time_base.den.saturating_mul(placement.speed.num),
    };
    let from = rescale(
        placement.source_in,
        placement.time_base,
        sound.time_base,
        Rounding::Down,
    )
    .ok_or_else(|| too_long(id))?
    .max(sound.start);
    let to = from
        .checked_add(
            rescale(
                placement.length,
                placement.sequence_time_base,
                played,
                Rounding::Down,
            )
            .ok_or_else(|| too_long(id))?,
        )
        .ok_or_else(|| too_long(id))?
        .min(sound.end);
    if to <= from {
        return Err(EditError::Refused(format!(
            "clip {id} plays none of its file's sound"
        )));
    }
    Ok((from, to))
}

/// The track a change touches the clips of, if it touches any: what a lock
/// is checked against.
fn touched_track(project: &Project, change: &Change) -> Option<TrackId> {
    match change {
        Change::InsertClip { track, .. } => Some(*track),
        Change::RemoveClip(id) => find(project, *id).ok().map(|(t, _)| t.id),
        Change::ReplaceClip(clip) => find(project, clip.id).ok().map(|(t, _)| t.id),
        Change::RemoveTrack(id) => Some(*id),
        _ => None,
    }
}

/// Place a new clip (the `AddClip` edit), adopting its settings when it is
/// the sequence's first (#57).
fn add_clip(
    project: &Project,
    facts: &Facts,
    edit: &Edit,
) -> Result<(Vec<Change>, Vec<ClipId>), EditError> {
    let Edit::AddClip {
        track,
        source,
        stream,
        time_base,
        start,
        from,
        to,
    } = edit
    else {
        return Ok((Vec::new(), Vec::new()));
    };
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
    Ok((changes, vec![id]))
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
            ..Track::default()
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
    if !super::speed::in_range(ratio) {
        return Err(EditError::Refused(format!(
            "a speed of {}/{} is outside the supported range of 0.1× to 100×",
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
        // Every edit, from every entry point, becomes these changes: the one
        // place a lock cannot be routed around.
        if let Some(track) = compiled
            .changes
            .iter()
            .filter_map(|change| touched_track(&self.project, change))
            .find(|&track| find_track(&self.project, track).is_ok_and(|t| t.locked))
        {
            return Err(EditError::Locked(track));
        }
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

    /// Record what the library shows of `source` — or with `None`, that it
    /// could not be read.
    pub fn describe_asset(&mut self, source: SourceId, asset: Option<super::asset::AssetInfo>) {
        match asset {
            Some(asset) => self.facts.assets.insert(source, asset),
            None => self.facts.assets.remove(&source),
        };
    }

    /// What the library shows of each source it could read.
    #[must_use]
    pub fn assets(&self) -> BTreeMap<SourceId, super::asset::AssetInfo> {
        self.facts
            .assets
            .iter()
            .filter(|(id, _)| self.project.sources.contains_key(id))
            .map(|(&id, asset)| (id, asset.clone()))
            .collect()
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

    /// What each video clip's speed does to its pictures at export (#56):
    /// the model's one answer, which the inspector states while the user is
    /// choosing and the planner (#39) reads for the same graph. A clip whose
    /// source's pictures are unknown — offline, unreadable — is not listed.
    #[must_use]
    pub fn speed_verdicts(
        &self,
        timeline: &super::evaluate::Timeline,
    ) -> BTreeMap<ClipId, super::speed::SpeedVerdict> {
        timeline
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Video)
            .flat_map(|track| &track.placements)
            .filter(|placement| self.project.sources.contains_key(&placement.source))
            .filter_map(|placement| {
                let shape = self.facts.geometry.get(&placement.source)?;
                Some((placement.clip, super::speed::verdict(placement, shape)))
            })
            .collect()
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
                ..Track::default()
            },
            Track {
                id: 2,
                kind: TrackKind::Video,
                clips: Vec::new(),
                ..Track::default()
            },
            Track {
                id: 3,
                kind: TrackKind::Audio,
                clips: Vec::new(),
                ..Track::default()
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
    fn a_speed_outside_the_supported_range_is_refused() {
        let mut document = document();
        for ratio in [Rational { num: 1, den: 11 }, Rational { num: 101, den: 1 }] {
            let refused = document.apply(
                &Edit::SetSpeed {
                    clips: vec![2],
                    ratio,
                },
                &at(&[2], 0),
            );
            assert!(matches!(refused, Err(EditError::Refused(_))), "{ratio:?}");
        }
        assert!(document.history().entries.is_empty());
    }

    #[test]
    fn each_video_clip_gets_the_speed_rule_s_verdict_for_the_same_graph() {
        let mut document = document();
        document.describe_source(
            1,
            Some(StreamGeometry {
                width: 1920,
                height: 1080,
                frame_rate: Rational { num: 25, den: 1 },
                pixel_aspect: Rational { num: 1, den: 1 },
                variable_frame_rate: false,
                hdr: false,
            }),
        );
        for (num, lossless) in [(4, true), (10, false)] {
            document
                .apply(
                    &Edit::SetSpeed {
                        clips: vec![2],
                        ratio: Rational { num, den: 1 },
                    },
                    &at(&[2], 0),
                )
                .expect("speed");
            let timeline = evaluate(document.project()).expect("evaluates");
            let verdicts = document.speed_verdicts(&timeline);
            assert_eq!(verdicts.keys().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
            let placement = timeline.tracks[0]
                .placements
                .iter()
                .find(|placement| placement.clip == 2)
                .expect("placed");
            let shape = document.facts.geometry[&1];
            // What the inspector states is what the rule decides for the
            // placement the planner reads.
            assert_eq!(
                verdicts[&2],
                super::super::speed::verdict(placement, &shape)
            );
            assert_eq!(verdicts[&2].tier.is_lossless(), lossless, "{num}×");
        }
        document.describe_source(1, None);
        let timeline = evaluate(document.project()).expect("evaluates");
        assert!(document.speed_verdicts(&timeline).is_empty());
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
                ..Track::default()
            },
            Track {
                id: 2,
                kind: TrackKind::Audio,
                clips: Vec::new(),
                ..Track::default()
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
                kind: TrackKind::Video,
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

    fn placement_of(document: &Document, clip: ClipId) -> Placement {
        evaluate(document.project())
            .expect("evaluates")
            .placements()
            .find(|p| p.clip == clip)
            .cloned()
            .expect("placed")
    }

    #[test]
    fn a_split_gives_two_clips_that_cover_the_original_exactly() {
        let mut document = bounded();
        let whole = placement_of(&document, 1);
        let applied = document
            .apply(
                &Edit::Split {
                    clips: vec![1],
                    at: 11,
                },
                &at(&[1], 11),
            )
            .expect("split");
        assert_eq!(applied.context.selection, vec![4], "the right half");
        assert_eq!(spans(&document, 0)[..2], [(1, 0, 11), (4, 11, 30)]);
        let (left, right) = (placement_of(&document, 1), placement_of(&document, 4));
        // The halves meet in the source: no tick in neither, none in both.
        assert_eq!(left.source_out, right.source_in);
        assert_eq!(left.source_in, whole.source_in);
        // The frame on screen at the cut is the one that was there.
        assert_eq!(right.source_at(11), whole.source_at(11));
        for frame in 0..11 {
            assert_eq!(left.source_at(frame), whole.source_at(frame));
        }
        assert_eq!(document.history().entries, vec!["Split"]);
    }

    #[test]
    fn split_then_join_is_the_original_byte_for_byte() {
        // 1/15360 at 30 fps: 512 ticks a frame, as an MP4 stores it.
        let fine = Rational {
            num: 1,
            den: 15_360,
        };
        for at_frame in 1..30 {
            let mut document = document();
            document
                .apply(
                    &Edit::AddClip {
                        track: 2,
                        source: 1,
                        stream: 0,
                        time_base: fine,
                        start: 0,
                        from: 1024,
                        to: 1024 + 15_360,
                    },
                    &EditContext::default(),
                )
                .expect("add");
            let original = json(&document);
            document
                .apply(
                    &Edit::Split {
                        clips: vec![4],
                        at: at_frame,
                    },
                    &EditContext::default(),
                )
                .expect("split");
            document
                .apply(&Edit::Join { left: 4, right: 5 }, &EditContext::default())
                .expect("join");
            assert_eq!(json(&document), original, "at {at_frame}");
        }
    }

    #[test]
    fn split_then_join_shows_the_same_clip_whatever_the_time_base() {
        // 1/1000 at 30 fps: 33⅓ ticks a frame. A sub-frame tail may be left
        // off, but the clip on screen is the same, frame for frame.
        for at_frame in 1..30 {
            let mut document = bounded();
            let whole = placement_of(&document, 1);
            document
                .apply(
                    &Edit::Split {
                        clips: vec![1],
                        at: at_frame,
                    },
                    &EditContext::default(),
                )
                .expect("split");
            document
                .apply(&Edit::Join { left: 1, right: 4 }, &EditContext::default())
                .expect("join");
            let joined = placement_of(&document, 1);
            assert_eq!((joined.start, joined.length), (whole.start, whole.length));
            for frame in 0..30 {
                assert_eq!(joined.source_at(frame), whole.source_at(frame), "{frame}");
            }
            assert!(whole.source_out - joined.source_out < 34);
        }
    }

    #[test]
    fn a_split_with_no_clip_named_cuts_everything_under_the_frame() {
        let mut document = bounded();
        document
            .apply(
                &Edit::AddClip {
                    track: 2,
                    source: 1,
                    stream: 0,
                    time_base: TB,
                    start: 0,
                    from: 0,
                    to: 1000,
                },
                &EditContext::default(),
            )
            .expect("add");
        document
            .apply(
                &Edit::Split {
                    clips: Vec::new(),
                    at: 15,
                },
                &EditContext::default(),
            )
            .expect("split");
        assert_eq!(spans(&document, 0).len(), 4);
        assert_eq!(spans(&document, 1).len(), 2);
    }

    #[test]
    fn nothing_splits_at_a_clip_edge_or_on_an_empty_track() {
        let mut document = bounded();
        for (clips, frame) in [(vec![1], 0), (vec![1], 30), (vec![2], 75), (vec![], 75)] {
            assert!(
                matches!(
                    document.apply(&Edit::Split { clips, at: frame }, &EditContext::default()),
                    Err(EditError::Refused(_))
                ),
                "at {frame}"
            );
        }
        assert!(document.history().entries.is_empty());
    }

    #[test]
    fn a_duplicate_lands_right_after_its_original_and_pushes_the_rest() {
        let mut document = bounded();
        let applied = document
            .apply(&Edit::Duplicate { clips: vec![1] }, &EditContext::default())
            .expect("duplicate");
        assert_eq!(applied.context.selection, vec![4]);
        assert_eq!(
            spans(&document, 0),
            vec![(1, 0, 30), (4, 30, 60), (2, 60, 90), (3, 120, 150)]
        );
        document.undo();
        assert_eq!(
            spans(&document, 0),
            vec![(1, 0, 30), (2, 30, 60), (3, 90, 120)]
        );
    }

    #[test]
    fn a_freeze_frame_holds_the_frame_and_is_marked_for_re_encoding() {
        let mut document = bounded();
        let shown = placement_of(&document, 2).source_at(40);
        document
            .apply(
                &Edit::FreezeFrame {
                    clip: 2,
                    at: 40,
                    frames: 90,
                },
                &EditContext::default(),
            )
            .expect("freeze");
        // Clip 2 is cut at 40, the hold sits there, the rest follows it.
        assert_eq!(
            spans(&document, 0),
            vec![
                (1, 0, 30),
                (2, 30, 40),
                (5, 40, 130),
                (4, 130, 150),
                (3, 180, 210)
            ]
        );
        let hold = placement_of(&document, 5);
        assert_eq!(hold.motion, Some(Motion::Hold));
        assert_eq!(hold.forced, Some(crate::tier::ReEncodeReason::FreezeFrame));
        assert_eq!(hold.source_at(40), shown);
        assert_eq!(hold.source_at(129), shown);
        // Trimming a hold changes only how long it lasts.
        document
            .apply(&edge(5, Edge::End, -30, true), &EditContext::default())
            .expect("trim hold");
        assert_eq!(spans(&document, 0)[2], (5, 40, 100));
        assert_eq!(spans(&document, 0)[3], (4, 100, 120));
        assert_eq!(document.history().entries.len(), 2);
    }

    #[test]
    fn a_reversed_clip_plays_its_source_backwards_and_is_marked() {
        let mut document = bounded();
        let forward: Vec<Option<i64>> = (30..60)
            .map(|frame| placement_of(&document, 2).source_at(frame))
            .collect();
        document
            .apply(
                &Edit::SetReverse {
                    clips: vec![2],
                    reverse: true,
                },
                &EditContext::default(),
            )
            .expect("reverse");
        let reversed = placement_of(&document, 2);
        assert_eq!(reversed.forced, Some(crate::tier::ReEncodeReason::Reverse));
        assert_eq!(reversed.source_at(30), Some(999));
        assert!(reversed.source_at(59) <= forward[1]);
        // Trimming the timeline start of a reversed clip trims its source's end.
        document
            .apply(&edge(2, Edge::Start, 10, false), &EditContext::default())
            .expect("trim");
        let trimmed = placement_of(&document, 2);
        assert_eq!((trimmed.start, trimmed.end()), (40, 60));
        assert_eq!(trimmed.source_in, 0);
        assert!(trimmed.source_out < 1000);
        document
            .apply(
                &Edit::SetReverse {
                    clips: vec![2],
                    reverse: false,
                },
                &EditContext::default(),
            )
            .expect("forwards");
        assert_eq!(placement_of(&document, 2).motion, None);
        assert_eq!(document.history().entries.len(), 3);
    }

    /// `bounded()`, with source 1 also known to have sound: stream 1, 48 kHz,
    /// ten seconds.
    fn with_sound() -> Document {
        let mut document = bounded();
        let audio = Rational {
            num: 1,
            den: 48_000,
        };
        document.describe_streams(
            1,
            &[
                StreamExtent {
                    source: 1,
                    stream: 0,
                    kind: TrackKind::Video,
                    time_base: TB,
                    start: 0,
                    end: 1500,
                },
                StreamExtent {
                    source: 1,
                    stream: 1,
                    kind: TrackKind::Audio,
                    time_base: audio,
                    start: 0,
                    end: 480_000,
                },
            ],
        );
        document
    }

    fn clip_of(document: &Document, id: ClipId) -> Clip {
        document
            .project()
            .clips()
            .find(|(_, c)| c.id == id)
            .map(|(_, c)| c.clone())
            .expect("clip")
    }

    #[test]
    fn detaching_audio_makes_a_linked_sound_clip_of_the_same_file() {
        let mut document = with_sound();
        let applied = document
            .apply(&Edit::DetachAudio { clips: vec![2] }, &at(&[2], 0))
            .expect("detach");
        let pictures = clip_of(&document, 2);
        assert!(pictures.detached);
        assert_eq!(pictures.link, Some(2));
        let sound = clip_of(&document, 4);
        assert_eq!(applied.context.selection, vec![2, 4]);
        assert_eq!((sound.source, sound.stream, sound.link), (1, 1, Some(2)));
        // On the audio track, exactly under the pictures.
        assert_eq!(spans(&document, 2), vec![(4, 30, 60)]);
        let placement = placement_of(&document, 2);
        assert!(placement.silent);
        assert!(!placement_of(&document, 4).silent);
        // Again: nothing more to detach.
        document
            .apply(
                &Edit::DetachAudio { clips: vec![2] },
                &EditContext::default(),
            )
            .expect("again");
        assert_eq!(document.history().entries.len(), 1);
        document.undo();
        assert!(!clip_of(&document, 2).detached);
        assert!(spans(&document, 2).is_empty());
    }

    #[test]
    fn detaching_a_clip_whose_track_is_taken_makes_a_new_audio_track() {
        let mut document = with_sound();
        document
            .apply(
                &Edit::DetachAudio { clips: vec![1] },
                &EditContext::default(),
            )
            .expect("first");
        // Clip 2 is right after clip 1: the same audio track has room.
        document
            .apply(
                &Edit::DetachAudio { clips: vec![2] },
                &EditContext::default(),
            )
            .expect("second");
        assert_eq!(document.project().sequence.tracks.len(), 3);
        // A clip on the second video track at the same time needs a new one.
        document
            .apply(
                &Edit::AddClip {
                    track: 2,
                    source: 1,
                    stream: 0,
                    time_base: TB,
                    start: 0,
                    from: 0,
                    to: 1000,
                },
                &EditContext::default(),
            )
            .expect("add");
        let added = document
            .project()
            .clips()
            .map(|(_, c)| c.id)
            .max()
            .expect("clip");
        document
            .apply(
                &Edit::DetachAudio { clips: vec![added] },
                &EditContext::default(),
            )
            .expect("third");
        let tracks = &document.project().sequence.tracks;
        assert_eq!(tracks.len(), 4);
        assert_eq!(tracks[3].kind, TrackKind::Audio);
    }

    #[test]
    fn linked_clips_move_and_delete_together_until_unlinked() {
        let mut document = with_sound();
        document
            .apply(
                &Edit::DetachAudio { clips: vec![2] },
                &EditContext::default(),
            )
            .expect("detach");
        document
            .apply(
                &Edit::MoveClips {
                    moves: vec![ClipMove {
                        clip: 2,
                        track: 1,
                        start: 200,
                    }],
                },
                &EditContext::default(),
            )
            .expect("move");
        assert_eq!(spans(&document, 2), vec![(4, 200, 230)]);
        document
            .apply(&Edit::Unlink { clips: vec![4] }, &EditContext::default())
            .expect("unlink");
        assert_eq!(clip_of(&document, 2).link, None);
        document
            .apply(
                &Edit::MoveClips {
                    moves: vec![ClipMove {
                        clip: 2,
                        track: 1,
                        start: 300,
                    }],
                },
                &EditContext::default(),
            )
            .expect("move alone");
        assert_eq!(spans(&document, 2), vec![(4, 200, 230)]);
        // Linked again by a fresh detach elsewhere, and deleted together.
        document
            .apply(
                &Edit::DetachAudio { clips: vec![1] },
                &EditContext::default(),
            )
            .expect("detach 1");
        document
            .apply(
                &Edit::RemoveClips { clips: vec![1] },
                &EditContext::default(),
            )
            .expect("delete");
        assert_eq!(spans(&document, 2), vec![(4, 200, 230)]);
    }

    #[test]
    fn a_locked_track_refuses_every_edit_to_its_clips() {
        let mut document = with_sound();
        document
            .apply(
                &Edit::SetTrack {
                    track: 1,
                    muted: None,
                    solo: None,
                    locked: Some(true),
                    collapsed: None,
                },
                &EditContext::default(),
            )
            .expect("lock");
        let original = json(&document);
        let refused = [
            Edit::MoveClips {
                moves: vec![ClipMove {
                    clip: 1,
                    track: 2,
                    start: 0,
                }],
            },
            Edit::TrimEdge {
                clip: 1,
                edge: Edge::End,
                frames: -5,
                ripple: false,
            },
            Edit::Roll {
                left: 1,
                right: 2,
                frames: 3,
            },
            Edit::SetSpeed {
                clips: vec![1],
                ratio: Rational { num: 2, den: 1 },
            },
            Edit::RemoveClips { clips: vec![1] },
            Edit::RippleDelete { clips: vec![1] },
            Edit::Split {
                clips: Vec::new(),
                at: 15,
            },
            Edit::Duplicate { clips: vec![1] },
            Edit::FreezeFrame {
                clip: 1,
                at: 10,
                frames: 30,
            },
            Edit::SetReverse {
                clips: vec![1],
                reverse: true,
            },
            Edit::DetachAudio { clips: vec![1] },
            Edit::AddClip {
                track: 1,
                source: 1,
                stream: 0,
                time_base: TB,
                start: 300,
                from: 0,
                to: 1000,
            },
            Edit::RemoveTrack { track: 1 },
            // A move onto a locked track from elsewhere is refused too.
        ];
        for edit in &refused {
            assert_eq!(
                document.apply(edit, &EditContext::default()),
                Err(EditError::Locked(1)),
                "{edit:?}"
            );
        }
        assert_eq!(json(&document), original);
        // Its switches still work, and unlocking lets edits through again.
        document
            .apply(
                &Edit::RenameTrack {
                    track: 1,
                    name: " Main ".to_owned(),
                },
                &EditContext::default(),
            )
            .expect("rename");
        assert_eq!(document.project().sequence.tracks[0].name, "Main");
        document
            .apply(
                &Edit::SetTrack {
                    track: 1,
                    muted: None,
                    solo: None,
                    locked: Some(false),
                    collapsed: None,
                },
                &EditContext::default(),
            )
            .expect("unlock");
        document
            .apply(&refused[4], &EditContext::default())
            .expect("now it deletes");
    }

    #[test]
    fn mute_and_solo_decide_what_is_seen_and_heard() {
        let mut document = with_sound();
        let set = |track, muted, solo| Edit::SetTrack {
            track,
            muted,
            solo,
            locked: None,
            collapsed: None,
        };
        let heard = |document: &Document| -> Vec<(TrackId, bool, bool)> {
            evaluate(document.project())
                .expect("evaluates")
                .tracks
                .iter()
                .map(|t| (t.id, t.visible, t.audible))
                .collect()
        };
        assert_eq!(
            heard(&document),
            vec![(1, true, true), (2, true, true), (3, false, true)]
        );
        document
            .apply(&set(3, None, Some(true)), &EditContext::default())
            .expect("solo");
        assert_eq!(
            heard(&document),
            vec![(1, true, false), (2, true, false), (3, false, true)]
        );
        // Soloing a muted track: muted wins, so nothing is heard.
        document
            .apply(&set(3, Some(true), None), &EditContext::default())
            .expect("mute");
        assert!(heard(&document).iter().all(|&(_, _, audible)| !audible));
        document
            .apply(&set(1, Some(true), None), &EditContext::default())
            .expect("hide");
        assert!(!heard(&document)[0].1);
        assert_eq!(document.history().entries.len(), 3);
    }

    #[test]
    fn the_top_video_track_is_in_front_and_moving_it_changes_that() {
        let mut document = with_sound();
        document
            .apply(
                &Edit::AddClip {
                    track: 2,
                    source: 1,
                    stream: 0,
                    time_base: TB,
                    start: 10,
                    from: 0,
                    to: 500,
                },
                &EditContext::default(),
            )
            .expect("add");
        let picture = |document: &Document| -> Vec<(ClipId, i64, i64)> {
            evaluate(document.project())
                .expect("evaluates")
                .picture()
                .iter()
                .map(|p| (p.clip, p.start, p.end()))
                .collect()
        };
        // Track 1 is on top: clip 1 covers clip 4 entirely.
        assert_eq!(
            picture(&document),
            vec![(1, 0, 30), (2, 30, 60), (3, 90, 120)]
        );
        document
            .apply(
                &Edit::MoveTrack { track: 2, index: 0 },
                &EditContext::default(),
            )
            .expect("move up");
        // Now clip 4 (10..25) is in front, and clip 1 shows around it.
        assert_eq!(
            picture(&document),
            vec![
                (1, 0, 10),
                (4, 10, 25),
                (1, 25, 30),
                (2, 30, 60),
                (3, 90, 120)
            ]
        );
        // The pieces of clip 1 play the same source ticks it did.
        let whole = placement_of(&document, 1);
        let pieces = evaluate(document.project()).expect("evaluates").picture();
        let after = pieces.iter().find(|p| p.start == 25).expect("piece");
        assert_eq!(after.source_at(25), whole.source_at(25));
        // Video tracks cannot go below audio ones.
        document
            .apply(
                &Edit::MoveTrack { track: 2, index: 9 },
                &EditContext::default(),
            )
            .expect("clamped");
        assert_eq!(document.project().sequence.tracks[1].id, 2);
    }

    #[test]
    fn removing_a_track_takes_its_clips_and_undo_brings_them_back() {
        let mut document = with_sound();
        let original = json(&document);
        document
            .apply(&Edit::RemoveTrack { track: 1 }, &at(&[1, 2], 0))
            .expect("remove");
        assert_eq!(document.project().sequence.tracks.len(), 2);
        assert_eq!(document.project().clips().count(), 0);
        document.undo();
        assert_eq!(json(&document), original);
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
        match random.below(11) {
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
            6 => Edit::RemoveClips { clips: vec![clip] },
            7 => Edit::Split {
                clips: if random.below(2) == 0 {
                    vec![clip]
                } else {
                    Vec::new()
                },
                at: random.int(300),
            },
            8 => Edit::Duplicate { clips: vec![clip] },
            9 => Edit::FreezeFrame {
                clip,
                at: random.int(300),
                frames: 1 + random.int(60),
            },
            _ => Edit::SetReverse {
                clips: vec![clip],
                reverse: random.below(2) == 0,
            },
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
