//! The export planner's decisions (#39), each asserted with its reason: a
//! copy where the edit allows one, a seam or a re-encode where it does not,
//! and the plan tiling the timeline for any graph.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::integer_division
)]

mod common;

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;

use blinkify_engine::export::plan::{
    AudioFacts, Cause, Decline, EncodingField, EncodingSignature, ExportPlan, KeyframeAlternative,
    KeyframePoint, Media, PlanError, SeamWindow, Segment, SourceFacts, VideoFacts, plan,
};
use blinkify_engine::probe::Rational;
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::settings::{SequenceSettings, StreamGeometry, copy_eligibility};
use blinkify_engine::project::speed;
use blinkify_engine::project::{
    Clip, ClipId, Operation, Project, SourceId, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::{ExportTier, ReEncodeReason, SeamReason};

/// A file for each source id, so a project can refer to it. The planner never
/// reads it; the project only checks that it exists when made.
fn source_ref(id: u32) -> SourceRef {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| common::scratch("export-planner"));
    let path = dir.join(format!("{id}.mp4"));
    if !path.is_file() {
        std::fs::write(&path, format!("source {id}")).expect("write");
    }
    SourceRef::of(&MediaAsset::new(path).export_source()).expect("source")
}

/// 30 fps source in a 1/15360 time base: one frame is 512 ticks, a second
/// 15360. Keyframes every second.
const TB: Rational = Rational {
    num: 1,
    den: 15_360,
};
const SECOND: i64 = 15_360;
const FRAME: i64 = 512;
const AUDIO_TB: Rational = Rational {
    num: 1,
    den: 48_000,
};

fn geometry() -> StreamGeometry {
    StreamGeometry {
        width: 1920,
        height: 1080,
        frame_rate: Rational { num: 30, den: 1 },
        pixel_aspect: Rational { num: 1, den: 1 },
        variable_frame_rate: false,
        hdr: false,
    }
}

fn signature(configuration: &str) -> EncodingSignature {
    EncodingSignature {
        codec: Some("h264".to_owned()),
        profile: Some("High".to_owned()),
        level: Some(40),
        pixel_format: Some("yuv420p".to_owned()),
        width: 1920,
        height: 1080,
        colour: [None, None, None, None],
        sample_rate: None,
        channels: None,
        configuration: Some(configuration.to_owned()),
    }
}

fn audio_signature() -> EncodingSignature {
    EncodingSignature {
        codec: Some("aac".to_owned()),
        profile: Some("LC".to_owned()),
        level: None,
        pixel_format: None,
        width: 0,
        height: 0,
        colour: [None, None, None, None],
        sample_rate: Some(48_000),
        channels: Some(2),
        configuration: Some("aac".to_owned()),
    }
}

/// A 20-second source with a keyframe every second and B-frames.
fn source(configuration: &str) -> SourceFacts {
    SourceFacts {
        video: Some(VideoFacts {
            stream: 0,
            time_base: TB,
            geometry: geometry(),
            encoding: signature(configuration),
            keyframes: (0..20)
                .map(|s| KeyframePoint {
                    pts: s * SECOND,
                    open: false,
                })
                .collect(),
            keyframes_complete: true,
            end: Some(20 * SECOND),
            reorders: true,
        }),
        audio: BTreeMap::from([(
            1,
            AudioFacts {
                stream: 1,
                time_base: AUDIO_TB,
                encoding: audio_signature(),
            },
        )]),
        default_audio: Some(1),
    }
}

fn project(sources: u32, tracks: Vec<Track>) -> Project {
    let mut project = Project::new("t", SequenceSettings::matching(&geometry()).expect("valid"));
    for id in 1..=sources {
        project.sources.insert(id, source_ref(id));
    }
    project.sequence.tracks = tracks;
    project
}

fn video_clip(id: ClipId, source: SourceId, start: i64, from: i64, to: i64) -> Clip {
    Clip::new(id, source, 0, TB, start, vec![Operation::Trim { from, to }])
}

fn video_track(clips: Vec<Clip>) -> Track {
    Track::new(1, TrackKind::Video, clips)
}

fn plan_of(project: &Project, facts: &BTreeMap<SourceId, SourceFacts>) -> ExportPlan {
    let timeline = evaluate(project).expect("evaluates");
    plan(&timeline, &project.sequence.settings, facts).expect("plans")
}

fn one_source() -> BTreeMap<SourceId, SourceFacts> {
    BTreeMap::from([(1, source("sps-a"))])
}

fn of(plan: &ExportPlan, media: Media) -> Vec<&Segment> {
    plan.segments.iter().filter(|s| s.media == media).collect()
}

#[test]
fn keyframe_aligned_cuts_plan_as_a_copy_of_everything() {
    let project = project(
        1,
        vec![video_track(vec![
            video_clip(1, 1, 0, 2 * SECOND, 5 * SECOND),
            video_clip(2, 1, 90, 8 * SECOND, 20 * SECOND),
        ])],
    );
    let plan = plan_of(&project, &one_source());
    assert!(plan.summary.lossless, "{:#?}", plan.segments);
    assert_eq!(plan.summary.video.re_encoded, 0);
    assert_eq!(plan.summary.audio.re_encoded, 0);
    assert_eq!(plan.summary.video.copied, plan.length);
    for segment in &plan.segments {
        assert_eq!(segment.tier, ExportTier::StreamCopy);
        assert!(segment.causes.is_empty());
    }
}

#[test]
fn one_cut_off_a_keyframe_re_encodes_only_the_window_up_to_the_next_one() {
    // In at 2 s + 10 frames: the window is up to the keyframe at 3 s.
    let project = project(
        1,
        vec![video_track(vec![video_clip(
            1,
            1,
            0,
            2 * SECOND + 10 * FRAME,
            6 * SECOND,
        )])],
    );
    let plan = plan_of(&project, &one_source());
    let video = of(&plan, Media::Video);
    assert_eq!(video.len(), 1);
    assert_eq!(
        video[0].tier,
        ExportTier::SmartCut {
            reason: SeamReason::InPointNotKeyframeAligned
        }
    );
    assert_eq!(
        video[0].causes,
        vec![Cause::InPointNotKeyframe {
            keyframe_before: Some(2 * SECOND),
            keyframe_after: Some(3 * SECOND),
        }]
    );
    assert_eq!(
        video[0].windows,
        vec![SeamWindow {
            from: 2 * SECOND + 10 * FRAME,
            to: 3 * SECOND
        }]
    );
    // 20 frames re-encoded, the other 90 copied.
    assert_eq!(plan.summary.video.re_encoded, 20);
    assert_eq!(plan.summary.video.copied, 90);
    assert!(!plan.summary.lossless);
    // The sound is a copy regardless: audio packets are all sync points.
    assert!(of(&plan, Media::Audio).iter().all(|s| s.tier.is_lossless()));
}

#[test]
fn an_out_point_off_a_keyframe_is_a_seam_only_where_pictures_reorder() {
    let clip = video_clip(1, 1, 0, 2 * SECOND, 4 * SECOND + 7 * FRAME);
    let project = project(1, vec![video_track(vec![clip])]);
    let plan = plan_of(&project, &one_source());
    let video = of(&plan, Media::Video);
    assert_eq!(
        video[0].tier,
        ExportTier::SmartCut {
            reason: SeamReason::OutPointNotKeyframeAligned
        }
    );
    assert_eq!(
        video[0].windows,
        vec![SeamWindow {
            from: 4 * SECOND,
            to: 4 * SECOND + 7 * FRAME
        }]
    );

    // Without B-frames every picture is decoded before the next is shown,
    // so the copy simply stops.
    let mut facts = one_source();
    facts
        .get_mut(&1)
        .and_then(|f| f.video.as_mut())
        .expect("video")
        .reorders = false;
    let plan = plan_of(&project, &facts);
    assert!(plan.summary.lossless);
}

#[test]
fn an_out_point_on_an_open_gop_keyframe_is_a_seam() {
    let mut facts = one_source();
    let video = facts.get_mut(&1).and_then(|f| f.video.as_mut()).expect("v");
    for keyframe in &mut video.keyframes {
        keyframe.open = true;
    }
    // Starting on an open keyframe is fine; ending on one is not.
    let project = project(
        1,
        vec![video_track(vec![video_clip(
            1,
            1,
            0,
            2 * SECOND,
            4 * SECOND,
        )])],
    );
    let plan = plan_of(&project, &facts);
    let video = of(&plan, Media::Video);
    assert_eq!(
        video[0].causes,
        vec![Cause::OpenGopAtOutPoint {
            keyframe: 4 * SECOND
        }]
    );
    assert_eq!(
        video[0].tier,
        ExportTier::SmartCut {
            reason: SeamReason::OpenGopBoundary
        }
    );
}

#[test]
fn a_gain_change_copies_the_pictures_and_re_encodes_only_the_sound() {
    let mut clip = video_clip(1, 1, 0, 2 * SECOND, 5 * SECOND);
    clip.push(Operation::Gain { db: 6.0 });
    let project = project(1, vec![video_track(vec![clip])]);
    let plan = plan_of(&project, &one_source());
    let video = of(&plan, Media::Video);
    let audio = of(&plan, Media::Audio);
    assert_eq!(video.len(), 1);
    assert_eq!(video[0].tier, ExportTier::StreamCopy);
    assert_eq!(audio.len(), 1);
    assert_eq!(
        audio[0].tier,
        ExportTier::FullReEncode {
            reason: ReEncodeReason::AudioFilter
        }
    );
    assert_eq!(audio[0].causes, vec![Cause::AudioChain]);
    // The sound's range is the same seconds, in the audio's own ticks.
    assert_eq!(audio[0].sources[0].source_in, 2 * 48_000);
    assert_eq!(audio[0].sources[0].source_out, 5 * 48_000);
    assert_eq!(plan.summary.video.re_encoded, 0);
    assert_eq!(plan.summary.audio.copied, 0);
}

#[test]
fn two_sources_with_different_parameters_do_not_share_a_copied_stream() {
    let project = project(
        2,
        vec![video_track(vec![
            video_clip(1, 1, 0, 0, 10 * SECOND),
            video_clip(2, 2, 300, 0, 2 * SECOND),
        ])],
    );
    let facts = BTreeMap::from([(1, source("sps-a")), (2, source("sps-b"))]);
    let plan = plan_of(&project, &facts);
    let video = of(&plan, Media::Video);
    // The longer source sets the output's parameters and is copied.
    assert_eq!(video[0].tier, ExportTier::StreamCopy);
    assert_eq!(
        video[1].tier,
        ExportTier::FullReEncode {
            reason: ReEncodeReason::IncompatibleSourceParameters
        }
    );
    assert_eq!(
        video[1].causes,
        vec![Cause::IncompatibleEncoding {
            reference: 1,
            fields: vec![EncodingField::Configuration],
        }]
    );
    assert!(
        video[1].causes[0]
            .sentence()
            .contains("codec configuration")
    );

    // The same parameters from two files are one stream.
    let same = BTreeMap::from([(1, source("sps-a")), (2, source("sps-a"))]);
    assert!(plan_of(&project, &same).summary.lossless);
}

#[test]
fn a_sequence_not_in_the_sources_shape_re_encodes_it_and_says_how() {
    let mut project = project(
        1,
        vec![video_track(vec![video_clip(1, 1, 0, 0, 2 * SECOND)])],
    );
    project.sequence.settings.width = 1280;
    project.sequence.settings.height = 720;
    project.sequence.settings.frame_rate = Rational { num: 25, den: 1 };
    let plan = plan_of(&project, &one_source());
    let video = of(&plan, Media::Video);
    assert_eq!(
        video[0].tier,
        ExportTier::FullReEncode {
            reason: ReEncodeReason::SequenceSettingsDiffer
        }
    );
    // Each mismatch is its own cause, and it is the one the settings'
    // predicate reports — the planner does not work it out again.
    let expected: Vec<Cause> = copy_eligibility(&project.sequence.settings, &geometry())
        .mismatches
        .into_iter()
        .map(|mismatch| Cause::SequenceMismatch { mismatch })
        .collect();
    assert_eq!(expected.len(), 2);
    assert_eq!(video[0].causes, expected);
    assert!(video[0].causes[0].sentence().contains("1920×1080"));
}

#[test]
fn every_re_encode_names_its_cause() {
    let mut held = video_clip(1, 1, 0, 2 * SECOND, 3 * SECOND);
    held.push(Operation::Freeze { frames: 30 });
    let mut reversed = video_clip(2, 1, 30, 4 * SECOND, 5 * SECOND);
    reversed.push(Operation::Reverse);
    let mut fast = video_clip(3, 1, 90, 6 * SECOND, 7 * SECOND);
    // 30 fps × 10 is 300 fps: no file carries it.
    fast.push(Operation::Speed {
        ratio: Rational { num: 10, den: 1 },
    });
    let project = project(1, vec![video_track(vec![held, reversed, fast])]);
    let plan = plan_of(&project, &one_source());
    for segment in &plan.segments {
        if segment.tier.is_lossless() {
            assert!(segment.causes.is_empty());
            continue;
        }
        assert!(!segment.causes.is_empty(), "{segment:#?}");
        for cause in &segment.causes {
            assert!(!cause.sentence().is_empty());
        }
    }
    let video = of(&plan, Media::Video);
    let reasons: Vec<ExportTier> = video.iter().map(|s| s.tier).collect();
    assert_eq!(
        reasons,
        vec![
            ExportTier::FullReEncode {
                reason: ReEncodeReason::FreezeFrame
            },
            ExportTier::FullReEncode {
                reason: ReEncodeReason::Reverse
            },
            // The gap between 60 and 90.
            ExportTier::FullReEncode {
                reason: ReEncodeReason::Gap
            },
            ExportTier::FullReEncode {
                reason: ReEncodeReason::SpeedFrameRateOutsideContainer
            },
        ]
    );
}

#[test]
fn the_speed_decision_is_the_inspectors() {
    for (num, den) in [(1, 1), (2, 1), (8, 1), (1, 2), (1, 10)] {
        let mut clip = video_clip(1, 1, 0, 0, 10 * SECOND);
        clip.push(Operation::Speed {
            ratio: Rational { num, den },
        });
        let project = project(1, vec![video_track(vec![clip])]);
        let timeline = evaluate(&project).expect("evaluates");
        let facts = one_source();
        let plan = plan(&timeline, &project.sequence.settings, &facts).expect("plans");
        let verdict = speed::verdict(&timeline.picture()[0], &geometry());
        assert_eq!(
            of(&plan, Media::Video)[0].tier.is_lossless(),
            verdict.tier.is_lossless(),
            "{num}/{den}"
        );
    }
}

#[test]
fn an_unindexed_cut_point_plans_as_a_re_encode_not_a_guess() {
    let mut facts = one_source();
    let video = facts.get_mut(&1).and_then(|f| f.video.as_mut()).expect("v");
    video.keyframes.retain(|k| k.pts < 5 * SECOND);
    video.keyframes_complete = false;
    let project = project(
        1,
        vec![video_track(vec![
            // On a keyframe the index already knows: provably a copy.
            video_clip(1, 1, 0, 2 * SECOND, 4 * SECOND),
            // Past what is indexed: not provable either way.
            video_clip(2, 1, 60, 12 * SECOND + FRAME, 14 * SECOND),
        ])],
    );
    let plan = plan_of(&project, &facts);
    let video = of(&plan, Media::Video);
    assert_eq!(video[0].tier, ExportTier::StreamCopy);
    assert_eq!(
        video[1].tier,
        ExportTier::FullReEncode {
            reason: ReEncodeReason::CopyNotProvable
        }
    );
    assert_eq!(video[1].causes, vec![Cause::KeyframesUnknown]);
}

#[test]
fn an_hdr_seam_is_declined_with_the_keyframe_cut_offered() {
    let mut facts = one_source();
    facts
        .get_mut(&1)
        .and_then(|f| f.video.as_mut())
        .expect("v")
        .geometry
        .hdr = true;
    let mut project = project(
        1,
        vec![video_track(vec![video_clip(
            1,
            1,
            0,
            2 * SECOND + 3 * FRAME,
            6 * SECOND,
        )])],
    );
    project.sequence.settings = SequenceSettings::matching(&StreamGeometry {
        hdr: true,
        ..geometry()
    })
    .expect("valid");
    let plan = plan_of(&project, &facts);
    let video = of(&plan, Media::Video);
    assert_eq!(video[0].decline, Some(Decline::HdrWouldBeRendered));
    assert_eq!(
        video[0].alternative,
        Some(KeyframeAlternative {
            source_in: 2 * SECOND,
            source_out: 6 * SECOND
        })
    );
    assert!(!plan.summary.exportable);

    // Copied as recorded, an HDR source is simply lossless.
    project.sequence.tracks[0].clips[0] = video_clip(1, 1, 0, 2 * SECOND, 6 * SECOND);
    let plan = plan_of(&project, &facts);
    assert!(plan.summary.lossless && plan.summary.exportable);
}

#[test]
fn a_detached_sound_on_its_own_track_is_still_a_copy_and_mixing_is_not() {
    let mut picture = video_clip(1, 1, 0, 0, 4 * SECOND);
    picture.detached = true;
    let sound = Clip::new(
        2,
        1,
        1,
        AUDIO_TB,
        0,
        vec![Operation::Trim {
            from: 0,
            to: 4 * 48_000,
        }],
    );
    let music = Clip::new(
        3,
        1,
        1,
        AUDIO_TB,
        60,
        vec![Operation::Trim {
            from: 0,
            to: 48_000,
        }],
    );
    let project = project(
        1,
        vec![
            video_track(vec![picture]),
            Track::new(2, TrackKind::Audio, vec![sound]),
            Track::new(3, TrackKind::Audio, vec![music]),
        ],
    );
    let plan = plan_of(&project, &one_source());
    let audio = of(&plan, Media::Audio);
    let tiers: Vec<(i64, i64, ExportTier)> =
        audio.iter().map(|s| (s.start, s.end(), s.tier)).collect();
    assert_eq!(
        tiers,
        vec![
            (0, 60, ExportTier::StreamCopy),
            (
                60,
                90,
                ExportTier::FullReEncode {
                    reason: ReEncodeReason::AudioMix
                }
            ),
            (90, 120, ExportTier::StreamCopy),
        ]
    );
    assert_eq!(audio[1].sources.len(), 2);
    assert_eq!(of(&plan, Media::Video)[0].tier, ExportTier::StreamCopy);
}

/// xorshift64*, fixed seed: a failure reproduces.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn int(&mut self, bound: u64) -> i64 {
        i64::try_from(self.next() % bound).expect("small")
    }
}

fn generated(random: &mut Random, clips_per_track: usize) -> Project {
    let mut tracks = Vec::new();
    let mut id = 0;
    for track in 1..=3_u32 {
        let kind = if track == 3 {
            TrackKind::Audio
        } else {
            TrackKind::Video
        };
        let mut start = random.int(40);
        let mut clips = Vec::new();
        for _ in 0..clips_per_track {
            id += 1;
            let from = random.int(15 * 30) * FRAME;
            let to = from + (1 + random.int(4 * 30)) * FRAME;
            let mut clip = if kind == TrackKind::Audio {
                Clip::new(
                    id,
                    1,
                    1,
                    AUDIO_TB,
                    start,
                    vec![Operation::Trim {
                        from: from * 48_000 / SECOND,
                        to: to * 48_000 / SECOND,
                    }],
                )
            } else {
                video_clip(
                    id,
                    1 + u32::try_from(random.int(2)).expect("small"),
                    start,
                    from,
                    to,
                )
            };
            match random.int(6) {
                0 => clip.push(Operation::Gain { db: -3.0 }),
                1 => clip.push(Operation::Speed {
                    ratio: Rational {
                        num: 1 + random.int(4),
                        den: 1 + random.int(4),
                    },
                }),
                2 if kind == TrackKind::Video => clip.push(Operation::Reverse),
                _ => {}
            }
            // The end of the clip: `length` counts from the timeline's start.
            let end = evaluate(&project(2, vec![Track::new(1, kind, vec![clip.clone()])]))
                .expect("evaluates")
                .length();
            start = end + random.int(3) * random.int(20);
            clips.push(clip);
        }
        let mut track = Track::new(track, kind, clips);
        track.muted = random.int(8) == 0;
        tracks.push(track);
    }
    project(2, tracks)
}

#[test]
fn every_plan_tiles_the_timeline_with_no_gap_or_overlap() {
    let mut random = Random(0x5eed_1234_abcd_0001);
    let facts = BTreeMap::from([(1, source("sps-a")), (2, source("sps-b"))]);
    for case in 0..300 {
        let clips = 1 + usize::try_from(random.int(8)).expect("small");
        let project = generated(&mut random, clips);
        let timeline = evaluate(&project).expect("evaluates");
        let plan = plan(&timeline, &project.sequence.settings, &facts)
            .unwrap_or_else(|error| panic!("case {case}: {error}"));
        for media in [Media::Video, Media::Audio] {
            let segments = of(&plan, media);
            if segments.is_empty() {
                continue;
            }
            let mut at = 0;
            for segment in &segments {
                assert_eq!(segment.start, at, "case {case} {media:?}: {segments:#?}");
                assert!(segment.length > 0, "case {case}");
                at = segment.end();
            }
            assert_eq!(at, plan.length, "case {case} {media:?}");
        }
        // The summary accounts for every frame of every stream exactly once.
        let video = of(&plan, Media::Video);
        if !video.is_empty() {
            assert_eq!(
                plan.summary.video.copied + plan.summary.video.re_encoded,
                plan.length,
                "case {case}"
            );
        }
        // A copy has no cause and anything else has one.
        for segment in &plan.segments {
            assert_eq!(
                segment.tier.is_lossless(),
                segment.causes.is_empty(),
                "case {case}: {segment:#?}"
            );
        }
    }
}

#[test]
fn a_long_timeline_plans_fast_enough_to_follow_every_edit() {
    let mut random = Random(0xfeed_beef_0000_0042);
    let project = generated(&mut random, 2000);
    let facts = BTreeMap::from([(1, source("sps-a")), (2, source("sps-b"))]);
    let timeline = evaluate(&project).expect("evaluates");
    let clips = timeline.placements().count();
    assert_eq!(clips, 6000);
    // The fastest of a few runs: the budget is the planner's cost, not the
    // load of the other tests running beside it.
    let mut elapsed = std::time::Duration::MAX;
    let mut plan_made = None;
    for _ in 0..5 {
        let started = Instant::now();
        let made = plan(&timeline, &project.sequence.settings, &facts).expect("plans");
        elapsed = elapsed.min(started.elapsed());
        plan_made = Some(made);
    }
    let plan = plan_made.expect("planned");
    assert!(!plan.segments.is_empty());
    // Measured on the reference laptop: 18 ms optimised, 104 ms in this
    // unoptimised test build — about 3 µs a clip where it ships. A real
    // project of a few hundred clips re-plans well inside one frame. The
    // budget is for the unoptimised build, with headroom for a slower CI
    // runner; a regression by a factor of three still fails it.
    assert!(
        elapsed.as_millis() < 300,
        "planning 6000 clips took {elapsed:?}"
    );
}

#[test]
fn an_empty_timeline_or_an_unread_source_is_an_error() {
    let empty = project(1, Vec::new());
    let timeline = evaluate(&empty).expect("evaluates");
    assert_eq!(
        plan(&timeline, &empty.sequence.settings, &one_source()),
        Err(PlanError::Empty)
    );
    let unread = project(1, vec![video_track(vec![video_clip(1, 1, 0, 0, SECOND)])]);
    let timeline = evaluate(&unread).expect("evaluates");
    assert_eq!(
        plan(&timeline, &unread.sequence.settings, &BTreeMap::new()),
        Err(PlanError::UnknownSource(1))
    );
}
