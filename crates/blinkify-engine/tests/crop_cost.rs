//! The cost of a crop and of a reframe, stated before they are made (#130,
//! #132), is the plan's own: what the inspector and the reframe dialog say
//! is what the planner does afterwards, and the statement returns to "copied"
//! when the crop is reset.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp
)]

mod common;

use std::collections::BTreeMap;
use std::sync::OnceLock;

use blinkify_engine::capability::{EncoderSource, VideoCodec};
use blinkify_engine::export::cost::{clips_cost, reframe_impact};
use blinkify_engine::export::plan::{
    AudioFacts, EncodingSignature, ExportPlan, KeyframePoint, Media, SourceFacts, VideoFacts, plan,
};
use blinkify_engine::export::profile::EncoderChoice;
use blinkify_engine::probe::{ChromaSubsampling, Rational};
use blinkify_engine::project::crop::{Aspect, CropRect};
use blinkify_engine::project::edit::{Document, Edit, EditContext};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::reframe::ReframeBasis;
use blinkify_engine::project::settings::{SequenceSettings, StreamGeometry};
use blinkify_engine::project::{
    Clip, ClipId, Operation, Project, SourceId, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::ExportTier;

fn source_ref(id: u32) -> SourceRef {
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| common::scratch("crop-cost"));
    let path = dir.join(format!("{id}.mp4"));
    if !path.is_file() {
        std::fs::write(&path, format!("source {id}")).expect("write");
    }
    SourceRef::of(&MediaAsset::new(path).export_source()).expect("source")
}

/// 30 fps in a 1/15360 time base, a keyframe every second.
const TB: Rational = Rational {
    num: 1,
    den: 15_360,
};
const SECOND: i64 = 15_360;

fn hd() -> StreamGeometry {
    StreamGeometry {
        width: 1920,
        height: 1080,
        frame_rate: Rational { num: 30, den: 1 },
        pixel_aspect: Rational { num: 1, den: 1 },
        variable_frame_rate: false,
        hdr: false,
        chroma: Some(ChromaSubsampling::Yuv420),
        rotation: 0,
    }
}

/// A phone held upright: coded 1920 × 1080, displayed 1080 × 1920.
fn phone() -> StreamGeometry {
    StreamGeometry {
        width: 1080,
        height: 1920,
        rotation: 90,
        ..hd()
    }
}

fn signature(width: u32, height: u32, sample_rate: Option<u32>) -> EncodingSignature {
    EncodingSignature {
        codec: Some(if sample_rate.is_some() { "aac" } else { "h264" }.to_owned()),
        profile: Some("High".to_owned()),
        level: None,
        pixel_format: None,
        width,
        height,
        colour: [None, None, None, None],
        sample_rate,
        channels: sample_rate.map(|_| 2),
        configuration: Some("same".to_owned()),
    }
}

fn encoder() -> EncoderChoice {
    EncoderChoice {
        codec: VideoCodec::H264,
        encoder: "libx264".to_owned(),
        source: EncoderSource::Software,
        profile: "high".to_owned(),
        pixel_format: "yuv420p".to_owned(),
        bit_depth: 8,
        level: None,
        colour: [None, None, None, None],
    }
}

/// A 20-second source of `geometry` with sound, keyframes every second.
fn facts(geometry: StreamGeometry) -> SourceFacts {
    SourceFacts {
        video: Some(VideoFacts {
            stream: 0,
            time_base: TB,
            geometry,
            // The coded size, which is what the parameter sets say.
            encoding: signature(1920, 1080, None),
            keyframes: (0..20)
                .map(|s| KeyframePoint {
                    pts: s * SECOND,
                    open: false,
                    random_access: true,
                })
                .collect(),
            keyframes_complete: true,
            end: Some(20 * SECOND),
            reorders: true,
            encoder: Ok(encoder()),
        }),
        audio: BTreeMap::from([(
            1,
            AudioFacts {
                stream: 1,
                time_base: Rational {
                    num: 1,
                    den: 48_000,
                },
                encoding: signature(0, 0, Some(48_000)),
            },
        )]),
        default_audio: Some(1),
    }
}

/// Three-second clips of `sources`, one after another on one video track,
/// in a 1080p30 sequence; the document knows each source's shape.
fn document(sources: &[(SourceId, StreamGeometry)], clips: &[SourceId]) -> Document {
    let mut project = Project::new("Trip", SequenceSettings::matching(&hd()).expect("valid"));
    for &(id, _) in sources {
        project.sources.insert(id, source_ref(id));
    }
    let placed = clips
        .iter()
        .zip(0..)
        .map(|(&source, index): (&SourceId, u32)| {
            Clip::new(
                index + 1,
                source,
                0,
                TB,
                i64::from(index) * 90,
                vec![Operation::Trim {
                    from: 0,
                    to: 3 * SECOND,
                }],
            )
        })
        .collect();
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, placed)];
    let mut document = Document::new(project).expect("valid");
    for &(id, geometry) in sources {
        document.describe_source(id, Some(geometry));
    }
    document
}

fn plan_of(project: &Project, facts: &BTreeMap<SourceId, SourceFacts>) -> ExportPlan {
    let timeline = evaluate(project).expect("evaluates");
    plan(&timeline, &project.sequence.settings, facts).expect("plans")
}

fn apply(document: &mut Document, edit: &Edit) {
    document
        .apply(edit, &EditContext::default())
        .expect("applies");
}

fn tiers(plan: &ExportPlan, clip: ClipId, media: Media) -> Vec<ExportTier> {
    plan.segments
        .iter()
        .filter(|s| s.media == media && s.sources.iter().any(|source| source.clip == clip))
        .map(|s| s.tier)
        .collect()
}

#[test]
fn a_crop_s_cost_is_the_plan_s_and_a_reset_states_the_copy_again() {
    let sources = BTreeMap::from([(1, facts(hd()))]);
    let mut document = document(&[(1, hd())], &[1, 1, 1]);
    apply(
        &mut document,
        &Edit::CropToAspect {
            clips: vec![2],
            aspect: Aspect::Vertical,
        },
    );
    let plan = plan_of(document.project(), &sources);
    let cost = clips_cost(&plan, &[2]);
    assert_eq!(cost.clips, 1);
    assert_eq!(cost.cropped_clips, 1);
    assert!((cost.cropped_seconds - 3.0).abs() < 1e-9, "{cost:?}");
    assert_eq!((cost.other_seconds, cost.copied_seconds), (0.0, 0.0));
    assert!(cost.other_reasons.is_empty());
    let sound = cost.sound.expect("it has sound");
    assert_eq!(sound.re_encoded_seconds, 0.0, "its sound is still copied");
    assert!((sound.copied_seconds - 3.0).abs() < 1e-9);
    // What the statement says is what the planner does.
    assert!(
        tiers(&plan, 2, Media::Video)
            .iter()
            .all(|tier| matches!(tier, ExportTier::FullReEncode { .. }))
    );
    assert!(
        tiers(&plan, 2, Media::Audio)
            .iter()
            .all(|tier| *tier == ExportTier::StreamCopy)
    );
    // Its neighbour is untouched by it.
    let beside = clips_cost(&plan, &[1]);
    assert_eq!(beside.cropped_clips, 0);
    assert!((beside.copied_seconds - 3.0).abs() < 1e-9);

    apply(&mut document, &Edit::ResetCrop { clips: vec![2] });
    let reset = clips_cost(&plan_of(document.project(), &sources), &[2]);
    assert_eq!(reset.cropped_clips, 0);
    assert_eq!(reset.cropped_seconds, 0.0);
    assert!(
        (reset.copied_seconds - 3.0).abs() < 1e-9,
        "copied bit for bit"
    );
}

#[test]
fn a_cropped_clip_re_encoded_for_another_reason_says_so() {
    let sources = BTreeMap::from([(1, facts(hd()))]);
    let mut document = document(&[(1, hd())], &[1]);
    apply(
        &mut document,
        &Edit::SetReverse {
            clips: vec![1],
            reverse: true,
        },
    );
    apply(
        &mut document,
        &Edit::CropToAspect {
            clips: vec![1],
            aspect: Aspect::Square,
        },
    );
    let cost = clips_cost(&plan_of(document.project(), &sources), &[1]);
    // The reverse is named: it would re-encode the clip on its own.
    assert_eq!(cost.cropped_clips, 0);
    assert!((cost.other_seconds - 3.0).abs() < 1e-9);
    assert!(cost.other_reasons.iter().any(|r| r.contains("backwards")));
    assert!(cost.sound.expect("sound").re_encoded_seconds > 0.0);
}

#[test]
fn a_landscape_project_reframed_to_vertical_states_the_three_clips_it_re_encodes() {
    let sources = BTreeMap::from([(1, facts(hd()))]);
    let mut document = document(&[(1, hd())], &[1, 1, 1]);
    let original = document.project().to_json().expect("json");
    let before = plan_of(document.project(), &sources);
    assert!(before.summary.lossless);

    let (reframe, after) = document.preview_reframe(Aspect::Vertical).expect("preview");
    let stated = reframe_impact(reframe.clone(), &before, &plan_of(&after, &sources));
    assert_eq!(stated.re_encoded_clips, vec![1, 2, 3]);
    assert!((stated.re_encoded_seconds - 9.0).abs() < 1e-9);
    assert!(stated.copied_clips.is_empty());
    assert_eq!(
        (
            stated.reframe.settings.width,
            stated.reframe.settings.height
        ),
        (608, 1080)
    );

    apply(
        &mut document,
        &Edit::Reframe {
            aspect: Aspect::Vertical,
        },
    );
    let applied = plan_of(document.project(), &sources);
    assert_eq!(
        reframe_impact(reframe, &before, &applied),
        stated,
        "the statement is the plan's afterwards"
    );
    // One undo returns the project byte for byte.
    document.undo();
    assert_eq!(document.project().to_json().expect("json"), original);
}

#[test]
fn a_vertical_clip_in_a_landscape_project_stays_copied_after_the_reframe() {
    let sources = BTreeMap::from([(1, facts(hd())), (2, facts(phone()))]);
    let mut document = document(&[(1, hd()), (2, phone())], &[1, 1, 1, 2]);
    let before = plan_of(document.project(), &sources);
    assert!(
        tiers(&before, 4, Media::Video)
            .iter()
            .all(|tier| matches!(tier, ExportTier::FullReEncode { .. })),
        "the phone clip is re-encoded to fit the 16:9 sequence"
    );

    let (reframe, after) = document.preview_reframe(Aspect::Vertical).expect("preview");
    assert_eq!(reframe.basis, ReframeBasis::Native { source: 2 });
    let stated = reframe_impact(reframe.clone(), &before, &plan_of(&after, &sources));
    assert_eq!(stated.re_encoded_clips, vec![1, 2, 3]);
    assert!((stated.re_encoded_seconds - 9.0).abs() < 1e-9);
    assert_eq!(stated.copied_clips, vec![4]);
    assert!(stated.still_re_encoded_clips.is_empty());
    assert_eq!(stated.reframe.scaled_up, vec![1, 2, 3]);

    apply(
        &mut document,
        &Edit::Reframe {
            aspect: Aspect::Vertical,
        },
    );
    assert_eq!(
        document.project().to_json().expect("json"),
        after.to_json().expect("json")
    );
    let applied = plan_of(document.project(), &sources);
    assert_eq!(reframe_impact(reframe, &before, &applied), stated);
    assert!(
        tiers(&applied, 4, Media::Video)
            .iter()
            .all(|tier| *tier == ExportTier::StreamCopy),
        "the 9:16 clip is copied"
    );
    // Every clip's sound is still copied: a crop never reaches it.
    for clip in 1..=4 {
        assert!(
            tiers(&applied, clip, Media::Audio)
                .iter()
                .all(|tier| *tier == ExportTier::StreamCopy),
            "clip {clip}"
        );
    }
    // Each crop can then be moved with the ordinary controls.
    apply(
        &mut document,
        &Edit::SetCropSides {
            clips: vec![1],
            x: Some(0),
            y: None,
            width: None,
            height: None,
        },
    );
    assert_eq!(
        evaluate(document.project())
            .expect("evaluates")
            .placements()
            .find(|p| p.clip == 1)
            .and_then(|p| p.crop),
        Some(CropRect {
            x: 0,
            y: 0,
            width: 608,
            height: 1080
        })
    );
}
