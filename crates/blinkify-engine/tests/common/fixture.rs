//! A corpus file ready to export: probed, fully indexed, and able to build a
//! plan from clips and run it. Shared by the export tests of Epic #6.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blinkify_engine::capability::{self, EncoderCapabilities};
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportOutcome, ExportRequest, export,
};
use blinkify_engine::export::facts::source_facts;
use blinkify_engine::export::plan::{ExportPlan, SourceFacts, VideoFacts, plan};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::CancelToken;
use blinkify_engine::probe::{MediaInfo, Prober, Rational};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;

pub struct Source {
    pub path: PathBuf,
    pub info: Arc<MediaInfo>,
    pub facts: SourceFacts,
}

impl Source {
    /// The corpus file `name`, probed and indexed end to end. The machine's
    /// encoders are not probed: the plan declines anything it would encode.
    pub fn corpus(name: &str) -> Self {
        Self::at(super::corpus(name), None)
    }

    /// The corpus file `name`, planned with this machine's real encoders:
    /// what a smart-cut or a re-encode is made with here.
    pub fn with_encoders(name: &str) -> Self {
        Self::at(super::corpus(name), Some(encoders()))
    }

    pub fn at(path: PathBuf, encoders: Option<&EncoderCapabilities>) -> Self {
        let orchestrator = super::orchestrator();
        let info = Prober::new(orchestrator.clone())
            .probe(&path)
            .expect("probe");
        let index = KeyframeIndex::open(&path, &info, orchestrator, None).expect("index");
        index.complete_in_background(|_| {}).expect("indexed");
        let facts = source_facts(&info, Some(&index), encoders);
        Self { path, info, facts }
    }

    pub fn video(&self) -> &VideoFacts {
        self.facts.video.as_ref().expect("video")
    }

    pub fn keyframe(&self, i: usize) -> i64 {
        self.video().keyframes.get(i).expect("keyframe").pts
    }

    /// The first shown keyframe: at or after zero.
    pub fn first(&self) -> i64 {
        self.video()
            .keyframes
            .iter()
            .map(|k| k.pts)
            .find(|pts| *pts >= 0)
            .expect("a keyframe")
    }

    pub fn end(&self) -> i64 {
        self.video().end.expect("end")
    }

    /// Ticks of the video stream to seconds.
    pub fn seconds(&self, ticks: i64) -> f64 {
        let tb = self.video().time_base;
        #[allow(clippy::cast_precision_loss)]
        let seconds = ticks as f64 * tb.num as f64 / tb.den as f64;
        seconds
    }

    /// A video clip of `from..to` ticks at sequence frame `start`.
    pub fn clip(&self, id: u32, start: i64, from: i64, to: i64, operations: &[Operation]) -> Clip {
        let mut clip = Clip::new(
            id,
            1,
            self.video().stream,
            self.video().time_base,
            start,
            vec![Operation::Trim { from, to }],
        );
        for operation in operations {
            clip.push(*operation);
        }
        clip
    }

    /// The plan of `tracks` over this file, in a sequence of its own shape.
    pub fn plan(&self, tracks: Vec<Track>) -> ExportPlan {
        self.plan_in(
            SequenceSettings::matching(&self.video().geometry).expect("valid"),
            tracks,
        )
    }

    pub fn plan_in(&self, settings: SequenceSettings, tracks: Vec<Track>) -> ExportPlan {
        let mut project = Project::new("t", settings);
        project.sources.insert(
            1,
            SourceRef::of(&MediaAsset::new(self.path.clone()).export_source()).expect("ref"),
        );
        project.sequence.tracks = tracks;
        let timeline = evaluate(&project).expect("evaluates");
        plan(
            &timeline,
            &project.sequence.settings,
            &BTreeMap::from([(1, self.facts.clone())]),
        )
        .expect("plans")
    }

    /// One video track of `clips`.
    pub fn plan_clips(&self, clips: Vec<Clip>) -> ExportPlan {
        self.plan(vec![Track::new(1, TrackKind::Video, clips)])
    }

    pub fn inputs(&self) -> BTreeMap<u32, ExportInput> {
        BTreeMap::from([(
            1,
            ExportInput {
                source: MediaAsset::new(self.path.clone()).export_source(),
                info: Arc::clone(&self.info),
            },
        )])
    }

    pub fn export(
        &self,
        plan: &ExportPlan,
        target: &Path,
        audio: AudioTarget,
    ) -> Result<ExportOutcome, ExportError> {
        let inputs = self.inputs();
        export(
            &super::orchestrator(),
            ExportRequest {
                plan,
                inputs: &inputs,
                target,
                overwrite: false,
                audio,
                cancel: CancelToken::default(),
                on_progress: None,
            },
        )
    }

    /// The source's video packet hashes from the in-point's keyframe, shown
    /// inside `from..to`, in decode order: what a copy must contain.
    pub fn expected_video(&self, from: i64, to: i64) -> Vec<String> {
        let all = super::packet_hashes(&self.path, "v:0");
        let start = all.iter().position(|p| p.pts == from).expect("in-point");
        all.iter()
            .skip(start)
            .filter(|p| p.pts >= from && p.pts < to)
            .map(|p| p.md5.clone())
            .collect()
    }
}

/// This machine's encoder capability profile, probed once per test binary.
pub fn encoders() -> &'static EncoderCapabilities {
    static PROFILE: std::sync::OnceLock<EncoderCapabilities> = std::sync::OnceLock::new();
    PROFILE.get_or_init(|| capability::probe(&super::orchestrator()))
}

/// A clip's speed as a ratio.
pub const fn speed(num: i64, den: i64) -> Operation {
    Operation::Speed {
        ratio: Rational { num, den },
    }
}
