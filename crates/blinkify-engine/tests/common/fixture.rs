//! A corpus file ready to export: probed, fully indexed, and able to build a
//! plan from clips and run it. Shared by the export tests of Epic #6.

#![allow(
    clippy::cast_precision_loss,
    clippy::indexing_slicing,
    clippy::expect_used,
    clippy::missing_panics_doc
)]

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use blinkify_engine::capability::VideoCodec;
use blinkify_engine::capability::{self, EncoderCapabilities};
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportOutcome, ExportRequest, export,
};
use blinkify_engine::export::facts::source_facts;
use blinkify_engine::export::nut;
use blinkify_engine::export::plan::{ExportPlan, SourceFacts, VideoFacts, plan};
use blinkify_engine::export::seam::is_parameter_set;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
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

/// Every video packet of `path` as a stream copy reads it: its presentation
/// timestamp in the NUT time base, keyframe flag, and a hash of its payload
/// **without in-band parameter sets** — the hash boundary a smart-cut is held
/// to: a seam re-sends the source's SPS and PPS before the next copied
/// keyframe, and those are the stream's configuration, not its pictures.
pub fn packets(path: &Path, codec: VideoCodec) -> Vec<(f64, bool, u64)> {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    super::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .flag("-copyts")
                .input(path)
                .option("-map", "0:v:0")
                .option("-c", "copy")
                .option("-output_ts_offset", "100")
                .option("-f", "nut")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("reads");
    let bytes = collected.lock().expect("lock").clone();
    let mut reader = nut::Reader::open(bytes.as_slice()).expect("nut");
    let length_size = match codec {
        VideoCodec::H264 => Some(usize::from(reader.header().streams[0].extradata[4] & 3) + 1),
        VideoCodec::Hevc => Some(usize::from(reader.header().streams[0].extradata[21] & 3) + 1),
        _ => None,
    };
    let base = reader.header().streams[0].time_base;
    // Seconds of the source's own timeline: the reader's offset taken off.
    let seconds = |pts: i64| pts as f64 * base.num as f64 / base.den as f64 - 100.0;
    let mut found = Vec::new();
    while let Some(packet) = reader.next_packet().expect("packet") {
        let mut hasher = DefaultHasher::new();
        match length_size {
            Some(size) => {
                let mut at = 0;
                while at + size <= packet.data.len() {
                    let length = packet.data[at..at + size]
                        .iter()
                        .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
                    let unit = &packet.data[at + size..(at + size + length).min(packet.data.len())];
                    if !is_parameter_set(codec, unit) {
                        unit.hash(&mut hasher);
                    }
                    at += size + length;
                }
            }
            None => packet.data.hash(&mut hasher),
        }
        found.push((seconds(packet.pts), packet.key, hasher.finish()));
    }
    found
}

/// The smallest PSNR, in dB, of `output`'s frames against the source's
/// frames shown in `from..to` (ticks of the source stream).
pub fn min_psnr(output: &Path, source: &Path, from: i64, to: i64) -> f64 {
    psnr_between(
        output,
        source,
        "[0:v]setpts=PTS-STARTPTS[o]",
        &format!("[1:v]trim=start_pts={from}:end_pts={to},setpts=PTS-STARTPTS[r]"),
    )
}

/// The smallest PSNR of `output`'s frames `from_frame..to_frame` against the
/// source's frame at `held` (its ticks), repeated: a held frame, checked.
pub fn min_psnr_of_hold(
    output: &Path,
    source: &Path,
    from_frame: i64,
    to_frame: i64,
    held: i64,
) -> f64 {
    psnr_between(
        output,
        source,
        &format!("[0:v]trim=start_frame={from_frame}:end_frame={to_frame},setpts=N/30/TB[o]"),
        &format!(
            "[1:v]trim=start_pts={held},select=eq(n\\,0),loop=loop={}:size=1:start=0,setpts=N/30/TB[r]",
            to_frame - from_frame - 1
        ),
    )
}

/// The smallest PSNR of `output`'s frames against the source's `from..to`
/// played backwards: a reverse, checked.
pub fn min_psnr_reversed(output: &Path, source: &Path, from: i64, to: i64) -> f64 {
    psnr_between(
        output,
        source,
        "[0:v]setpts=N/30/TB[o]",
        &format!(
            "[1:v]trim=start_pts={from}:end_pts={to},setpts=PTS-STARTPTS,reverse,setpts=N/30/TB[r]"
        ),
    )
}

/// The smallest per-frame PSNR between `[o]` of `output` and `[r]` of
/// `source`, each made by its filter chain.
fn psnr_between(output: &Path, source: &Path, output_chain: &str, source_chain: &str) -> f64 {
    let result = super::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "info")
                .input(output)
                .flag("-copyts")
                .input(source)
                .option(
                    "-filter_complex",
                    format!("{output_chain};{source_chain};[o][r]psnr"),
                )
                .output_null(),
            Priority::Foreground,
        )
        .expect("compares");
    // The summary on stderr: `PSNR y:… average:… min:… max:…`.
    let summary = result
        .stderr_tail
        .iter()
        .find(|line| line.contains("PSNR") && line.contains("min:"))
        .cloned()
        .unwrap_or_default();
    summary
        .split("min:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map_or(0.0, |value| {
            if value == "inf" {
                f64::INFINITY
            } else {
                value.parse().unwrap_or(0.0)
            }
        })
}

/// A four-second file with a keyframe every second, made here with the
/// sidecar's own software encoder: the corpus's VP9 and AV1 files have one
/// keyframe each, and a smart-cut needs a GOP to copy between two seams.
pub fn keyframed(name: &str, arguments: &[(&'static str, &str)]) -> Source {
    let path = super::scratch(&format!("smartcut-source-{name}")).join("source.mkv");
    let mut command = SidecarCommand::ffmpeg()
        .option("-v", "error")
        .lavfi_input("testsrc2=size=640x360:rate=30:duration=4")
        .option("-pix_fmt", "yuv420p")
        .option("-g", "30")
        .option("-keyint_min", "30");
    for (name, value) in arguments {
        command = command.option(name, (*value).to_owned());
    }
    super::orchestrator()
        .run_to_end(command.output_file(&path), Priority::Foreground)
        .expect("encodes");
    Source::at(path, Some(encoders()))
}
