//! The edit-graph evaluator (#30): the one place the graph is interpreted.
//!
//! A preview that disagrees with the export is found out only after a long
//! render, so there is one evaluator with two consumers. The preview
//! (`PlaybackPlan::from_timeline`) and the export
//! ([`crate::export::ExportContent`]) both start from the [`Timeline`] made
//! here, and neither reads a clip's operations itself — they cannot: the
//! field is private to [`crate::project`], and `pnpm evaluator:check` keeps
//! the renderer from interpreting its copy of the graph.
//!
//! The evaluator is pure. Graph in, placements out; no file, no clock, no
//! media. What a source *is* — its frames, its proxy — belongs to the
//! consumer. That is also why a proxy cannot change the result: the
//! evaluator never sees one.
//!
//! What it decides, per clip:
//!
//! - **The source range**: the last trim. Trims are absolute ranges of the
//!   source, so a later one replaces an earlier one.
//! - **The speed**: the product of every speed change, reduced.
//! - **Where it sits**: its start, and its length in sequence frames, rounded
//!   up so a partial last frame still shows.
//! - **The audio chain**: gain, denoise and normalise, in the order given.

use serde::Serialize;
use thiserror::Error;
use ts_rs::TS;

use super::{Clip, ClipId, Operation, Project, ProjectError, SourceId, TrackId, TrackKind};
use crate::probe::Rational;
use crate::tier::ReEncodeReason;
use crate::time::{Rounding, rescale};

/// One step of a clip's audio chain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, TS)]
#[serde(tag = "op", rename_all = "kebab-case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AudioOperation {
    Gain { db: f64 },
    Denoise { strength: f64 },
    Normalise { target_lufs: f64 },
}

/// The audio-chain step `operation` is, if it is one.
#[must_use]
pub fn audio_operation(operation: &Operation) -> Option<AudioOperation> {
    match *operation {
        Operation::Gain { db } => Some(AudioOperation::Gain { db }),
        Operation::Denoise { strength } => Some(AudioOperation::Denoise { strength }),
        Operation::Normalise { target_lufs } => Some(AudioOperation::Normalise { target_lufs }),
        Operation::Trim { .. }
        | Operation::Speed { .. }
        | Operation::Freeze { .. }
        | Operation::Reverse => None,
    }
}

/// Why `operation` forces a full re-encode of its clip, if it does — the
/// reason the planner records and the export report states (#35).
#[must_use]
pub fn forces_re_encode(operation: &Operation) -> Option<ReEncodeReason> {
    match operation {
        Operation::Freeze { .. } => Some(ReEncodeReason::FreezeFrame),
        Operation::Reverse => Some(ReEncodeReason::Reverse),
        _ => None,
    }
}

/// How a clip moves through its source, when not forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Motion {
    /// From the out-point back to the in-point.
    Reverse,
    /// The in-point's frame, held.
    Hold,
}

/// A clip, resolved: which source ticks play, where, how fast, through what.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Placement {
    pub track: TrackId,
    pub kind: TrackKind,
    pub clip: ClipId,
    pub source: SourceId,
    pub stream: u32,
    /// The source stream's time base: the unit of `sourceIn` and `sourceOut`.
    pub time_base: Rational,
    #[ts(type = "number")]
    pub source_in: i64,
    /// The first source tick not played.
    #[ts(type = "number")]
    pub source_out: i64,
    /// Sequence frames.
    #[ts(type = "number")]
    pub start: i64,
    /// Sequence frames.
    #[ts(type = "number")]
    pub length: i64,
    /// Reduced: `3/1`, never `6/2`.
    pub speed: Rational,
    pub audio: Vec<AudioOperation>,
    /// The sequence's time base, which `start` and `length` count.
    pub sequence_time_base: Rational,
    /// Backwards or held; absent when the clip plays forwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub motion: Option<Motion>,
    /// Why the clip must be re-encoded whole, if something forces it — a
    /// hold or a reverse. Recorded here so the planner and the export report
    /// cannot miss it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub forced: Option<ReEncodeReason>,
}

impl Placement {
    /// The first sequence frame after the clip.
    #[must_use]
    pub fn end(&self) -> i64 {
        self.start.saturating_add(self.length)
    }

    #[must_use]
    pub fn covers(&self, position: i64) -> bool {
        self.start <= position && position < self.end()
    }

    /// The time base in which one source tick is one tick of timeline time:
    /// the source's, stretched by the speed.
    #[must_use]
    pub fn played_time_base(&self) -> Rational {
        Rational {
            num: self.time_base.num.saturating_mul(self.speed.den),
            den: self.time_base.den.saturating_mul(self.speed.num),
        }
    }

    /// The source tick on screen at sequence frame `position`: the last one
    /// at or before that moment. `None` outside the clip.
    #[must_use]
    pub fn source_at(&self, position: i64) -> Option<i64> {
        if !self.covers(position) {
            return None;
        }
        if self.motion == Some(Motion::Hold) {
            return Some(self.source_in);
        }
        let offset = rescale(
            position - self.start,
            self.sequence_time_base,
            self.played_time_base(),
            Rounding::Down,
        )?;
        let tick = if self.motion == Some(Motion::Reverse) {
            self.source_out.checked_sub(1)?.checked_sub(offset)?
        } else {
            self.source_in.checked_add(offset)?
        };
        (self.source_in <= tick && tick < self.source_out).then_some(tick)
    }

    /// The clip's operations as the evaluator resolved them: one trim, the
    /// speed if it is not 1, then the audio chain.
    #[must_use]
    pub fn operations(&self) -> Vec<Operation> {
        let mut operations = vec![Operation::Trim {
            from: self.source_in,
            to: self.source_out,
        }];
        if self.speed != (Rational { num: 1, den: 1 }) {
            operations.push(Operation::Speed { ratio: self.speed });
        }
        match self.motion {
            Some(Motion::Hold) => operations.push(Operation::Freeze {
                frames: self.length,
            }),
            Some(Motion::Reverse) => operations.push(Operation::Reverse),
            None => {}
        }
        operations.extend(self.audio.iter().map(|operation| match *operation {
            AudioOperation::Gain { db } => Operation::Gain { db },
            AudioOperation::Denoise { strength } => Operation::Denoise { strength },
            AudioOperation::Normalise { target_lufs } => Operation::Normalise { target_lufs },
        }));
        operations
    }
}

/// One track, evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EvaluatedTrack {
    pub id: TrackId,
    pub kind: TrackKind,
    /// In timeline order, none overlapping.
    pub placements: Vec<Placement>,
}

/// The whole graph, evaluated: what the preview plays and the export writes.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Timeline {
    pub time_base: Rational,
    pub frame_rate: Rational,
    pub tracks: Vec<EvaluatedTrack>,
}

/// What applies at one position: the diagnostic view's content.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OperationsAt {
    /// Sequence frames.
    #[ts(type = "number")]
    pub position: i64,
    pub clips: Vec<AppliedClip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppliedClip {
    pub track: TrackId,
    pub kind: TrackKind,
    pub clip: ClipId,
    pub source: SourceId,
    /// The source tick on screen, in the clip's time base.
    #[ts(type = "number")]
    pub source_tick: i64,
    pub operations: Vec<Operation>,
}

impl Timeline {
    /// Every placement, track by track.
    pub fn placements(&self) -> impl Iterator<Item = &Placement> {
        self.tracks.iter().flat_map(|track| track.placements.iter())
    }

    /// The first sequence frame after the last clip.
    #[must_use]
    pub fn length(&self) -> i64 {
        self.placements().map(Placement::end).max().unwrap_or(0)
    }

    /// What applies at sequence frame `position`, track by track.
    #[must_use]
    pub fn at(&self, position: i64) -> OperationsAt {
        let clips = self
            .tracks
            .iter()
            .filter_map(|track| {
                let placement = placement_at(&track.placements, position)?;
                Some(AppliedClip {
                    track: placement.track,
                    kind: placement.kind,
                    clip: placement.clip,
                    source: placement.source,
                    source_tick: placement.source_at(position)?,
                    operations: placement.operations(),
                })
            })
            .collect();
        OperationsAt { position, clips }
    }
}

/// The placement of `placements` (in order, none overlapping) covering
/// `position`.
#[must_use]
pub fn placement_at(placements: &[Placement], position: i64) -> Option<&Placement> {
    let after = placements.partition_point(|p| p.start <= position);
    placements
        .get(after.checked_sub(1)?)
        .filter(|placement| placement.covers(position))
}

/// Why a graph could not be evaluated.
#[derive(Debug, Error, PartialEq)]
pub enum EvaluateError {
    #[error(transparent)]
    Invalid(#[from] ProjectError),
    #[error("clip {0} has no trim, so how much of its source plays is unknown")]
    Untrimmed(ClipId),
    #[error("clips {first} and {second} overlap on track {track}")]
    Overlap {
        track: TrackId,
        first: ClipId,
        second: ClipId,
    },
    #[error("clip {0}'s timing does not fit in 64 bits")]
    Overflow(ClipId),
}

/// Evaluate the whole graph.
///
/// # Errors
///
/// An inconsistent graph ([`Project::validate`]), an untrimmed clip, two
/// clips on one track that overlap, or timing out of range. Never a panic.
pub fn evaluate(project: &Project) -> Result<Timeline, EvaluateError> {
    project.validate()?;
    let settings = &project.sequence.settings;
    let sequence = settings.time_base();
    let mut tracks = Vec::with_capacity(project.sequence.tracks.len());
    for track in &project.sequence.tracks {
        let mut placements = track
            .clips
            .iter()
            .map(|clip| place(track.id, track.kind, clip, sequence))
            .collect::<Result<Vec<_>, _>>()?;
        placements.sort_by_key(|placement| (placement.start, placement.clip));
        for pair in placements.windows(2) {
            if let [first, second] = pair
                && second.start < first.end()
            {
                return Err(EvaluateError::Overlap {
                    track: track.id,
                    first: first.clip,
                    second: second.clip,
                });
            }
        }
        tracks.push(EvaluatedTrack {
            id: track.id,
            kind: track.kind,
            placements,
        });
    }
    Ok(Timeline {
        time_base: sequence,
        frame_rate: settings.frame_rate,
        tracks,
    })
}

fn place(
    track: TrackId,
    kind: TrackKind,
    clip: &Clip,
    sequence: Rational,
) -> Result<Placement, EvaluateError> {
    let overflow = || EvaluateError::Overflow(clip.id);
    let mut trim = None;
    let mut speed = (1_i64, 1_i64);
    let mut audio = Vec::new();
    let mut hold = None;
    let mut reverse = false;
    for operation in &clip.operations {
        match *operation {
            Operation::Trim { from, to } => trim = Some((from, to)),
            Operation::Speed { ratio } => {
                speed = reduce(
                    speed.0.checked_mul(ratio.num).ok_or_else(overflow)?,
                    speed.1.checked_mul(ratio.den).ok_or_else(overflow)?,
                );
            }
            Operation::Gain { db } => audio.push(AudioOperation::Gain { db }),
            Operation::Denoise { strength } => audio.push(AudioOperation::Denoise { strength }),
            Operation::Normalise { target_lufs } => {
                audio.push(AudioOperation::Normalise { target_lufs });
            }
            Operation::Freeze { frames } => hold = Some(frames),
            Operation::Reverse => reverse = true,
        }
    }
    let (source_in, source_out) = trim.ok_or(EvaluateError::Untrimmed(clip.id))?;
    let mut placement = Placement {
        track,
        kind,
        clip: clip.id,
        source: clip.source,
        stream: clip.stream,
        time_base: clip.time_base,
        source_in,
        source_out,
        start: clip.start,
        length: 0,
        speed: Rational {
            num: speed.0,
            den: speed.1,
        },
        audio,
        sequence_time_base: sequence,
        // A hold wins over a reverse: one frame has no direction.
        motion: if hold.is_some() {
            Some(Motion::Hold)
        } else if reverse {
            Some(Motion::Reverse)
        } else {
            None
        },
        forced: clip.operations.iter().find_map(forces_re_encode),
    };
    let played = placement.played_time_base();
    if played.num <= 0 || played.den <= 0 {
        return Err(overflow());
    }
    placement.length = match hold {
        Some(frames) => frames,
        None => {
            rescale(source_out - source_in, played, sequence, Rounding::Up).ok_or_else(overflow)?
        }
    };
    placement
        .start
        .checked_add(placement.length)
        .ok_or_else(overflow)?;
    Ok(placement)
}

fn reduce(num: i64, den: i64) -> (i64, i64) {
    let (mut a, mut b) = (num.unsigned_abs(), den.unsigned_abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let divisor = i64::try_from(a.max(1)).unwrap_or(1);
    // Exact: the divisor divides both.
    (num.div_euclid(divisor), den.div_euclid(divisor))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::project::{Fingerprint, SequenceSettings, SourceRef, Track};

    fn project(clips: Vec<Clip>) -> Project {
        let mut project = Project::new("t", SequenceSettings::default());
        project.sources.insert(
            1,
            SourceRef::for_test(
                "a.mp4",
                Fingerprint {
                    size: 1,
                    modified: None,
                    content_hash: "0".to_owned(),
                },
            ),
        );
        project.sequence.tracks.push(Track {
            id: 1,
            kind: TrackKind::Video,
            clips,
        });
        project
    }

    const TB: Rational = Rational {
        num: 1,
        den: 90_000,
    };

    fn clip(id: ClipId, start: i64, operations: Vec<Operation>) -> Clip {
        Clip::new(id, 1, 0, TB, start, operations)
    }

    #[test]
    fn timing_is_exact_in_integer_ticks() {
        let trim = Operation::Trim {
            from: 225_000,
            to: 288_000,
        };
        let mut ntsc = project(vec![clip(1, 10, vec![trim])]);
        ntsc.sequence.settings.frame_rate = Rational {
            num: 30_000,
            den: 1001,
        };
        let timeline = evaluate(&ntsc).expect("evaluate");
        let placement = &timeline.tracks[0].placements[0];
        // 0.7 s at 29.97 fps is 20.979 frames: the partial last one counts.
        assert_eq!(placement.length, 21);
        assert_eq!(placement.source_at(10), Some(225_000));
        assert_eq!(placement.source_at(9), None);
        // Frame 20 of the clip starts at 20 × 3003 ticks of 90 kHz.
        assert_eq!(placement.source_at(30), Some(225_000 + 60_060));
        assert_eq!(placement.source_at(31), None);

        let fast = project(vec![clip(
            1,
            10,
            vec![
                trim,
                Operation::Speed {
                    ratio: Rational { num: 2, den: 1 },
                },
                Operation::Speed {
                    ratio: Rational { num: 3, den: 2 },
                },
            ],
        )]);
        let timeline = evaluate(&fast).expect("evaluate");
        let placement = &timeline.tracks[0].placements[0];
        assert_eq!(placement.speed, Rational { num: 3, den: 1 });
        // 0.7 s at triple speed: 7 frames of 30 fps, exactly.
        assert_eq!(placement.length, 7);
        assert_eq!(placement.source_at(16), Some(225_000 + 6 * 9000));
    }

    #[test]
    fn the_operations_at_a_position_are_resolved_in_order() {
        let timeline = evaluate(&project(vec![
            clip(
                2,
                30,
                vec![Operation::Trim {
                    from: 0,
                    to: 90_000,
                }],
            ),
            clip(
                1,
                0,
                vec![
                    Operation::Trim { from: 0, to: 9 },
                    Operation::Gain { db: -3.0 },
                    Operation::Trim {
                        from: 90_000,
                        to: 180_000,
                    },
                    Operation::Normalise { target_lufs: -16.0 },
                ],
            ),
        ]))
        .expect("evaluate");
        // Sorted into timeline order whatever the file's order.
        let ids: Vec<ClipId> = timeline.placements().map(|p| p.clip).collect();
        assert_eq!(ids, vec![1, 2]);
        let at = timeline.at(15);
        assert_eq!(at.clips.len(), 1);
        assert_eq!(at.clips[0].source_tick, 90_000 + 15 * 3000);
        assert_eq!(
            at.clips[0].operations,
            vec![
                Operation::Trim {
                    from: 90_000,
                    to: 180_000
                },
                Operation::Gain { db: -3.0 },
                Operation::Normalise { target_lufs: -16.0 },
            ]
        );
        // The gap between them has nothing, and the clip boundary is exact.
        assert_eq!(timeline.at(29).clips[0].clip, 1);
        assert_eq!(timeline.at(30).clips[0].clip, 2);
        assert!(timeline.at(60).clips.is_empty());
        assert_eq!(timeline.length(), 60);
    }

    #[test]
    fn an_unusable_graph_is_an_error_not_a_panic() {
        let untrimmed = project(vec![clip(1, 0, vec![Operation::Gain { db: 1.0 }])]);
        assert_eq!(evaluate(&untrimmed), Err(EvaluateError::Untrimmed(1)));

        let overlapping = project(vec![
            clip(
                1,
                0,
                vec![Operation::Trim {
                    from: 0,
                    to: 90_000,
                }],
            ),
            clip(
                2,
                29,
                vec![Operation::Trim {
                    from: 0,
                    to: 90_000,
                }],
            ),
        ]);
        assert_eq!(
            evaluate(&overlapping),
            Err(EvaluateError::Overlap {
                track: 1,
                first: 1,
                second: 2
            })
        );

        let mut orphan = project(vec![clip(1, 0, vec![Operation::Trim { from: 0, to: 1 }])]);
        orphan.sources.clear();
        assert!(matches!(evaluate(&orphan), Err(EvaluateError::Invalid(_))));

        let huge = project(vec![clip(
            1,
            i64::MAX - 1,
            vec![Operation::Trim {
                from: 0,
                to: 90_000,
            }],
        )]);
        assert_eq!(evaluate(&huge), Err(EvaluateError::Overflow(1)));

        let absurd = project(vec![clip(
            1,
            0,
            vec![
                Operation::Trim { from: 0, to: 1 },
                Operation::Speed {
                    ratio: Rational {
                        num: i64::MAX,
                        den: 1,
                    },
                },
                Operation::Speed {
                    ratio: Rational {
                        num: i64::MAX,
                        den: 1,
                    },
                },
            ],
        )]);
        assert_eq!(evaluate(&absurd), Err(EvaluateError::Overflow(1)));
    }
}
