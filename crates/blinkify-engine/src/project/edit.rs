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

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use super::evaluate::evaluate;
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
    /// Play `from..to` of the clip's source, starting at `start` — trimming
    /// either edge.
    TrimClip {
        clip: ClipId,
        #[ts(type = "number")]
        from: i64,
        #[ts(type = "number")]
        to: i64,
        #[ts(type = "number")]
        start: i64,
    },
    /// Play the clips at `ratio` times normal speed; `1/1` is normal.
    SetSpeed {
        clips: Vec<ClipId>,
        ratio: Rational,
    },
    RemoveClips {
        clips: Vec<ClipId>,
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
            Self::TrimClip { .. } => "Trim clip".to_owned(),
            Self::SetSpeed { clips: ids, .. } => {
                clips(ids.len(), "Change speed", "Change speed of")
            }
            Self::RemoveClips { clips: ids } => clips(ids.len(), "Delete clip", "Delete"),
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
}

/// One primitive change to the graph. Applying it returns its inverse.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Change {
    Name(String),
    Settings(SequenceSettings),
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

/// A clip with the trim replaced by `from..to`: one trim, where the first
/// one was, or first.
fn retrimmed(clip: &Clip, from: i64, to: i64, start: i64) -> Clip {
    let mut operations = Vec::with_capacity(clip.operations.len() + 1);
    let mut placed = false;
    for operation in &clip.operations {
        if matches!(operation, Operation::Trim { .. }) {
            if !placed {
                operations.push(Operation::Trim { from, to });
                placed = true;
            }
        } else {
            operations.push(*operation);
        }
    }
    if !placed {
        operations.insert(0, Operation::Trim { from, to });
    }
    Clip {
        start,
        operations,
        ..clip.clone()
    }
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

/// What an edit compiles to: the primitive changes, and the selection after.
type Compiled = (Vec<Change>, Vec<ClipId>);

/// The primitive changes `edit` makes to `project`, and the selection after.
fn compile(project: &Project, edit: &Edit, context: &EditContext) -> Result<Compiled, EditError> {
    let kept = context.selection.clone();
    Ok(match edit {
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
            (vec![insert], vec![id])
        }
        Edit::MoveClips { moves } => (move_clips(project, moves)?, kept),
        Edit::TrimClip {
            clip,
            from,
            to,
            start,
        } => (trim_clip(project, *clip, *from, *to, *start)?, kept),
        Edit::SetSpeed { clips, ratio } => (set_speed(project, clips, *ratio)?, kept),
        Edit::RemoveClips { clips } => {
            let mut changes = Vec::new();
            for &id in &clips.iter().copied().collect::<BTreeSet<_>>() {
                find(project, id)?;
                changes.push(Change::RemoveClip(id));
            }
            let remaining = kept.into_iter().filter(|c| !clips.contains(c)).collect();
            (changes, remaining)
        }
    })
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

fn trim_clip(
    project: &Project,
    id: ClipId,
    from: i64,
    to: i64,
    start: i64,
) -> Result<Vec<Change>, EditError> {
    let (track, current) = find(project, id)?;
    let trimmed = retrimmed(current, from, to, start);
    Ok(if trimmed == *current {
        Vec::new()
    } else if trimmed.start == current.start {
        vec![Change::ReplaceClip(trimmed)]
    } else {
        // A new start can change the clip's place in timeline order.
        vec![
            Change::RemoveClip(id),
            Change::InsertClip {
                track: track.id,
                index: None,
                clip: trimmed,
            },
        ]
    })
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
    pub fn apply(&mut self, edit: &Edit, context: &EditContext) -> Result<EditContext, EditError> {
        let (changes, selection) = compile(&self.project, edit, context)?;
        let after = EditContext {
            selection,
            playhead: context.playhead,
        };
        self.commit(edit.label(), changes, context, after)
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
                &Edit::TrimClip {
                    clip: 1,
                    from: 500,
                    to: 500,
                    start: 0
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
        assert_eq!(after, at(&[1], 42));
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
        assert_eq!(after.selection, vec![4]);
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
        let trimmed = retrimmed(&clip, 4, 6, 9);
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
            4 => {
                let from = random.int(5000);
                Edit::TrimClip {
                    clip,
                    from,
                    to: from + 1 + random.int(3000),
                    start: random.int(600),
                }
            }
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
