//! The edit graph and the project file (#32).
//!
//! This is the document every other part of Blinkify reads: what the user
//! cut, placed and adjusted, over original sources, as data. It is what makes
//! editing non-destructive, and what the export planner (Epic #6) will
//! compile into FFmpeg operations.
//!
//! The rules it is built to:
//!
//! - **Time is integer.** A clip's trim is in its source's own time base, a
//!   position on the timeline is in the sequence's; floating-point seconds
//!   appear nowhere, because they accumulate error and eventually put a cut a
//!   frame from where the user put it.
//! - **Operations are data.** Trim, speed, gain, denoise, normalise — each is
//!   an [`Operation`] carrying its parameters. Nothing here *interprets* them;
//!   the shared evaluator (#30) is the one place that does.
//! - **Only originals.** A [`SourceRef`] is made from an
//!   [`ExportSource`](crate::proxy::ExportSource), which can only ever hold the
//!   original file — so no node can refer to a proxy, a render or a temporary
//!   file.
//! - **A file a later build can open.** `schemaVersion` is in the file from
//!   the first commit, older versions are migrated forwards by
//!   [`migrate`], and a file from a future version is refused with a reason
//!   rather than misread.
//! - **Byte-identical saves.** Maps are ordered, fields are in declaration
//!   order, and the output ends with one newline, so saving an unchanged
//!   project changes no byte — no spurious diff, no "unsaved changes" prompt.

pub mod edit;
pub mod evaluate;
pub mod migrate;
pub mod settings;
pub mod source;
pub mod split;
pub mod trim;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

pub use settings::{
    ColourPolicy, CopyEligibility, CopyNote, Mismatch, SequenceSettings, SettingsError,
    StreamGeometry, copy_eligibility,
};
pub use source::{Fingerprint, RelinkError, SourceRef, SourceStatus};

use crate::probe::Rational;
use crate::proxy::ExportSource;

/// The schema this build writes, and the newest it reads.
pub const SCHEMA_VERSION: u32 = 3;

/// The project file extension, without the dot.
pub const EXTENSION: &str = "blinkify";

/// A source file of the project.
pub type SourceId = u32;

/// A track of the sequence.
pub type TrackId = u32;

/// A clip on a track.
pub type ClipId = u32;

/// A whole project: its sources and its sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    /// Every file a clip refers to, by id.
    pub sources: BTreeMap<SourceId, SourceRef>,
    pub sequence: Sequence,
}

/// The timeline: its settings and its tracks, top to bottom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Sequence {
    /// Resolution, frame rate, aspect and colour — defined, and bound to copy
    /// eligibility, by #57; referenced here, never duplicated.
    pub settings: SequenceSettings,
    /// Whether the settings are still waiting for the first clip: the next
    /// video clip placed on an empty sequence gives it its own geometry and
    /// frame rate (#57). Cleared the moment the settings are chosen or
    /// adopted.
    pub match_first_clip: bool,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    /// In timeline order.
    pub clips: Vec<Clip>,
}

/// A piece of a source placed on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Clip {
    pub id: ClipId,
    pub source: SourceId,
    /// The source stream the clip plays: its video on a video track, its
    /// audio on an audio track.
    pub stream: u32,
    /// The time base of that stream — the unit of every trim.
    pub time_base: Rational,
    /// Where the clip starts on the timeline, in ticks of the sequence's time
    /// base (one tick per frame; see [`SequenceSettings::time_base`]).
    #[ts(type = "number")]
    pub start: i64,
    /// What is done to the clip, in order. Data only, and private: the
    /// evaluator ([`evaluate`], #30) is the one reader, so there is no second
    /// interpretation to disagree with it.
    ///
    /// ```compile_fail
    /// # use blinkify_engine::project::Clip;
    /// fn interpret(clip: &Clip) -> usize {
    ///     clip.operations.len() // private: go through `project::evaluate`
    /// }
    /// ```
    operations: Vec<Operation>,
}

/// One thing done to a clip, with its parameters.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "kebab-case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum Operation {
    /// Use the source from `from` up to, not including, `to` — ticks of the
    /// clip's time base.
    Trim {
        #[ts(type = "number")]
        from: i64,
        #[ts(type = "number")]
        to: i64,
    },
    /// Play at `ratio` times normal speed: 2/1 is double, 1/2 half.
    Speed { ratio: Rational },
    /// Change the level by `db` decibels.
    Gain { db: f64 },
    /// Reduce noise, from 0 (off) to 1 (strongest).
    Denoise { strength: f64 },
    /// Bring the clip to an integrated loudness, in LUFS.
    Normalise { target_lufs: f64 },
    /// Hold the first frame of the trim for `frames` sequence frames (#35).
    /// The picture is one the source never showed for that long, so the
    /// clip is re-encoded at export.
    Freeze {
        #[ts(type = "number")]
        frames: i64,
    },
    /// Play the trim backwards (#35). Re-encoded at export: packets cannot
    /// be copied in reverse order.
    Reverse,
}

impl Clip {
    #[must_use]
    pub fn new(
        id: ClipId,
        source: SourceId,
        stream: u32,
        time_base: Rational,
        start: i64,
        operations: Vec<Operation>,
    ) -> Self {
        Self {
            id,
            source,
            stream,
            time_base,
            start,
            operations,
        }
    }

    /// Do `operation` to the clip, after everything already done to it.
    pub fn push(&mut self, operation: Operation) {
        self.operations.push(operation);
    }
}

/// Why a project file could not be opened.
#[derive(Debug, Error, PartialEq)]
pub enum ProjectError {
    #[error("the project file is damaged: {0}")]
    Corrupt(String),
    #[error(
        "this project was saved by a newer version of Blinkify (schema {found}); this version reads up to schema {supported}"
    )]
    FutureVersion { found: u32, supported: u32 },
    #[error("the project is inconsistent: {0}")]
    Invalid(String),
    #[error("the project file could not be read or written: {0}")]
    Io(String),
}

impl Project {
    /// An empty project with the given sequence settings, chosen.
    #[must_use]
    pub fn new(name: &str, settings: SequenceSettings) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.to_owned(),
            sources: BTreeMap::new(),
            sequence: Sequence {
                settings,
                match_first_clip: false,
                tracks: Vec::new(),
            },
        }
    }

    /// An empty project whose sequence takes its settings from the first
    /// video clip placed on it: the default (#57).
    #[must_use]
    pub fn matching_first_clip(name: &str) -> Self {
        let mut project = Self::new(name, SequenceSettings::default());
        project.sequence.match_first_clip = true;
        project
    }

    /// The project as its file: pretty JSON, deterministic, one trailing
    /// newline.
    ///
    /// # Errors
    ///
    /// Only if a value cannot be represented in JSON — a non-finite gain.
    pub fn to_json(&self) -> Result<String, ProjectError> {
        let mut text = serde_json::to_string_pretty(self)
            .map_err(|error| ProjectError::Invalid(error.to_string()))?;
        text.push('\n');
        Ok(text)
    }

    /// Read a project file's contents: migrate it to this build's schema,
    /// then check it is consistent.
    ///
    /// # Errors
    ///
    /// See [`ProjectError`].
    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|error| ProjectError::Corrupt(error.to_string()))?;
        let value = migrate::to_current(value)?;
        let project: Self = serde_json::from_value(value)
            .map_err(|error| ProjectError::Corrupt(error.to_string()))?;
        project.validate()?;
        Ok(project)
    }

    /// Read a project file.
    ///
    /// # Errors
    ///
    /// The file cannot be read, or as [`Project::from_json`].
    pub fn load(path: &Path) -> Result<Self, ProjectError> {
        let text =
            std::fs::read_to_string(path).map_err(|error| ProjectError::Io(error.to_string()))?;
        Self::from_json(&text)
    }

    /// Write the project file: to a sibling first, then renamed over, so a
    /// crash mid-save never leaves half a project.
    ///
    /// # Errors
    ///
    /// The project cannot be serialised or the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), ProjectError> {
        let text = self.to_json()?;
        let partial = path.with_extension(format!("{EXTENSION}.saving"));
        std::fs::write(&partial, text).map_err(|error| ProjectError::Io(error.to_string()))?;
        std::fs::rename(&partial, path).map_err(|error| ProjectError::Io(error.to_string()))
    }

    /// Add an original file as a source, or find it if it is one already.
    ///
    /// # Errors
    ///
    /// The file cannot be read.
    pub fn add_source(&mut self, source: &ExportSource) -> std::io::Result<SourceId> {
        let reference = SourceRef::of(source)?;
        if let Some((&id, _)) = self
            .sources
            .iter()
            .find(|(_, known)| known.fingerprint().same_content(reference.fingerprint()))
        {
            return Ok(id);
        }
        let id = self.sources.keys().next_back().map_or(1, |last| last + 1);
        self.sources.insert(id, reference);
        Ok(id)
    }

    /// Look at every source. What opening a project shows the user: the
    /// project opens whatever this says, and a source that is not present
    /// marks its clips.
    #[must_use]
    pub fn check_sources(&self) -> BTreeMap<SourceId, SourceStatus> {
        self.sources
            .iter()
            .map(|(&id, source)| (id, source.check()))
            .collect()
    }

    /// The clips that play `source`, in track order.
    #[must_use]
    pub fn clips_of(&self, source: SourceId) -> Vec<ClipId> {
        self.clips()
            .filter(|(_, clip)| clip.source == source)
            .map(|(_, clip)| clip.id)
            .collect()
    }

    /// Point `source` at `candidate`: the same content, somewhere else.
    ///
    /// # Errors
    ///
    /// No such source, or [`RelinkError`].
    pub fn relink(
        &mut self,
        source: SourceId,
        candidate: &ExportSource,
    ) -> Result<(), RelinkError> {
        let known = self
            .sources
            .get(&source)
            .ok_or_else(|| RelinkError::Unreadable(format!("no source {source}")))?;
        let relinked = known.relink(candidate)?;
        self.sources.insert(source, relinked);
        Ok(())
    }

    /// Every clip, with the track it is on.
    pub fn clips(&self) -> impl Iterator<Item = (&Track, &Clip)> {
        self.sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
    }

    /// Check the things the types cannot: every id unique, every clip's
    /// source present, every parameter in range.
    ///
    /// # Errors
    ///
    /// [`ProjectError::Invalid`], naming the first problem.
    pub fn validate(&self) -> Result<(), ProjectError> {
        let invalid = |message: String| Err(ProjectError::Invalid(message));
        if self.schema_version != SCHEMA_VERSION {
            return invalid(format!("schema {} after migration", self.schema_version));
        }
        if let Err(error) = self.sequence.settings.validate() {
            return invalid(error.to_string());
        }
        let mut tracks = BTreeSet::new();
        let mut clips = BTreeSet::new();
        for track in &self.sequence.tracks {
            if track.id == 0 {
                // The preview's main track is 0; a project's start at 1.
                return invalid("track 0 is reserved".to_owned());
            }
            if !tracks.insert(track.id) {
                return invalid(format!("track {} appears twice", track.id));
            }
            for clip in &track.clips {
                if !clips.insert(clip.id) {
                    return invalid(format!("clip {} appears twice", clip.id));
                }
                if !self.sources.contains_key(&clip.source) {
                    return invalid(format!("clip {} refers to no source", clip.id));
                }
                if clip.time_base.num <= 0 || clip.time_base.den <= 0 {
                    return invalid(format!("clip {} has no time base", clip.id));
                }
                for operation in &clip.operations {
                    check_operation(clip.id, operation)?;
                }
            }
        }
        Ok(())
    }
}

fn check_operation(clip: ClipId, operation: &Operation) -> Result<(), ProjectError> {
    let valid = match *operation {
        Operation::Trim { from, to } => from < to,
        Operation::Speed { ratio } => ratio.num > 0 && ratio.den > 0,
        Operation::Gain { db } => db.is_finite(),
        Operation::Denoise { strength } => (0.0..=1.0).contains(&strength),
        Operation::Normalise { target_lufs } => target_lufs.is_finite() && target_lufs < 0.0,
        Operation::Freeze { frames } => frames >= 1,
        Operation::Reverse => true,
    };
    if valid {
        Ok(())
    } else {
        Err(ProjectError::Invalid(format!(
            "clip {clip} has an operation out of range: {operation:?}"
        )))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    fn project() -> Project {
        let mut project = Project::new("Holiday", SequenceSettings::default());
        project.sources.insert(
            1,
            SourceRef::for_test(
                "C:\\clips\\beach.mp4",
                Fingerprint {
                    size: 10,
                    modified: Some(5),
                    content_hash: "ab".to_owned(),
                },
            ),
        );
        project.sequence.tracks.push(Track {
            id: 1,
            kind: TrackKind::Video,
            clips: vec![Clip {
                id: 1,
                source: 1,
                stream: 0,
                time_base: Rational { num: 1, den: 15360 },
                start: 0,
                operations: vec![
                    Operation::Trim {
                        from: 512,
                        to: 15360,
                    },
                    Operation::Speed {
                        ratio: Rational { num: 2, den: 1 },
                    },
                    Operation::Gain { db: -3.5 },
                ],
            }],
        });
        project
    }

    #[test]
    fn a_save_of_a_loaded_project_is_byte_identical() {
        let text = project().to_json().expect("json");
        let again = Project::from_json(&text)
            .expect("load")
            .to_json()
            .expect("json");
        assert_eq!(text, again);
        assert!(text.ends_with("}\n"));
        assert!(text.contains(&format!("\"schemaVersion\": {SCHEMA_VERSION}")));
    }

    /// xorshift64*: enough randomness to generate graphs, and a fixed seed
    /// so a failure reproduces.
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

        #[allow(clippy::cast_precision_loss)]
        fn float(&mut self) -> f64 {
            // Full 53-bit mantissas: the values a slider actually produces
            // are not round, and a float that does not survive the text is
            // exactly what this test is for.
            (self.next() >> 11) as f64 / (1_u64 << 53) as f64
        }
    }

    fn generated(random: &mut Random) -> Project {
        let rates = [(24, 1), (25, 1), (30_000, 1001), (60, 1), (50, 1)];
        let (num, den) = rates[usize::try_from(random.below(5)).expect("small")];
        let mut project = Project::new(
            ["", "Holiday", "Ünïcødé \"quoted\" \\ name", "a\nb"]
                [usize::try_from(random.below(4)).expect("small")],
            SequenceSettings {
                frame_rate: Rational { num, den },
                ..SequenceSettings::default()
            },
        );
        let sources = random.below(4) + 1;
        for id in 1..=sources {
            project.sources.insert(
                u32::try_from(id * 3).expect("small"),
                SourceRef::for_test(
                    &format!("D:\\Footage {id}\\clip é{}.mov", random.below(100)),
                    Fingerprint {
                        size: random.next() >> 1,
                        modified: (random.below(2) == 0).then(|| random.next() >> 20),
                        content_hash: format!("{:032x}", random.next()),
                    },
                ),
            );
        }
        let source_ids: Vec<SourceId> = project.sources.keys().copied().collect();
        let mut clip_id = 0;
        for track in 0..=random.below(4) {
            let mut clips = Vec::new();
            for _ in 0..random.below(6) {
                clip_id += 1 + u32::try_from(random.below(3)).expect("small");
                let mut operations = Vec::new();
                for _ in 0..random.below(5) {
                    operations.push(match random.below(5) {
                        0 => {
                            let from = random.int(1 << 40);
                            Operation::Trim {
                                from,
                                to: from + 1 + random.int(1 << 30),
                            }
                        }
                        1 => Operation::Speed {
                            ratio: Rational {
                                num: 1 + random.int(8),
                                den: 1 + random.int(8),
                            },
                        },
                        2 => Operation::Gain {
                            db: (random.float() - 0.5) * 96.0,
                        },
                        3 => Operation::Denoise {
                            strength: random.float(),
                        },
                        _ => Operation::Normalise {
                            target_lufs: -1.0 - random.float() * 40.0,
                        },
                    });
                }
                clips.push(Clip {
                    id: clip_id,
                    source: source_ids[usize::try_from(random.below(sources)).expect("small")],
                    stream: u32::try_from(random.below(3)).expect("small"),
                    time_base: Rational {
                        num: 1,
                        den: [90_000, 15_360, 48_000, 1000]
                            [usize::try_from(random.below(4)).expect("small")],
                    },
                    start: random.int(1 << 32),
                    operations,
                });
            }
            project.sequence.tracks.push(Track {
                id: u32::try_from(track + 1).expect("small"),
                kind: if random.below(2) == 0 {
                    TrackKind::Video
                } else {
                    TrackKind::Audio
                },
                clips,
            });
        }
        project
    }

    #[test]
    fn generated_graphs_round_trip_byte_for_byte() {
        let mut random = Random(0x9e37_79b9_7f4a_7c15);
        for case in 0..2000 {
            let project = generated(&mut random);
            project
                .validate()
                .unwrap_or_else(|error| panic!("case {case}: {error}"));
            let text = project.to_json().expect("json");
            let loaded =
                Project::from_json(&text).unwrap_or_else(|error| panic!("case {case}: {error}"));
            assert_eq!(loaded, project, "case {case}");
            assert_eq!(loaded.to_json().expect("json"), text, "case {case}");
        }
    }

    #[test]
    fn operations_are_tagged_data() {
        let text = project().to_json().expect("json");
        assert!(text.contains("\"op\": \"trim\""), "{text}");
        assert!(text.contains("\"op\": \"speed\""), "{text}");
        assert!(text.contains("\"db\": -3.5"), "{text}");
    }

    #[test]
    fn an_inconsistent_project_is_refused_with_a_reason() {
        let mut orphan = project();
        orphan.sources.clear();
        assert!(
            matches!(orphan.validate(), Err(ProjectError::Invalid(m)) if m.contains("no source"))
        );
        let mut backwards = project();
        backwards.sequence.tracks[0].clips[0].operations[0] = Operation::Trim { from: 9, to: 9 };
        assert!(backwards.validate().is_err());
        let mut twice = project();
        let clip = twice.sequence.tracks[0].clips[0].clone();
        twice.sequence.tracks[0].clips.push(clip);
        assert!(twice.validate().is_err());
    }
}
