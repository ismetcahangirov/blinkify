//! The audio-only export path (#43): the sound is filtered and re-encoded in
//! the same pass as the pictures are copied, and the pictures come back
//! exactly as they went in.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::integer_division,
    clippy::cast_precision_loss
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::execute::{
    ExportError, ExportInput, ExportOutcome, ExportRequest, export,
};
use blinkify_engine::export::facts::source_facts;
use blinkify_engine::export::plan::{ExportPlan, Media, SourceFacts, plan};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::probe::{MediaInfo, Prober, Rational};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::tier::{ExportTier, ReEncodeReason};

struct Source {
    path: PathBuf,
    info: Arc<MediaInfo>,
    facts: SourceFacts,
}

fn source(name: &str) -> Source {
    let path = common::corpus(name);
    let orchestrator = common::orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let index = KeyframeIndex::open(&path, &info, orchestrator, None).expect("index");
    index.complete_in_background(|_| {}).expect("indexed");
    let facts = source_facts(&info, Some(&index));
    Source { path, info, facts }
}

impl Source {
    fn video_tb(&self) -> Rational {
        self.facts.video.as_ref().expect("video").time_base
    }

    fn keyframe(&self, i: usize) -> i64 {
        self.facts.video.as_ref().expect("video").keyframes[i].pts
    }

    fn audio_stream(&self) -> u32 {
        self.facts.default_audio.expect("audio")
    }

    fn inputs(&self) -> BTreeMap<u32, ExportInput> {
        BTreeMap::from([(
            1,
            ExportInput {
                source: MediaAsset::new(self.path.clone()).export_source(),
                info: Arc::clone(&self.info),
            },
        )])
    }

    fn plan(&self, tracks: Vec<Track>) -> ExportPlan {
        let video = self.facts.video.as_ref().expect("video");
        let mut project = Project::new(
            "t",
            SequenceSettings::matching(&video.geometry).expect("valid"),
        );
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

    /// A video clip of `from..to` ticks at sequence frame `start`.
    fn clip(&self, id: u32, start: i64, from: i64, to: i64, operations: &[Operation]) -> Clip {
        let mut clip = Clip::new(
            id,
            1,
            self.facts.video.as_ref().expect("video").stream,
            self.video_tb(),
            start,
            vec![Operation::Trim { from, to }],
        );
        for operation in operations {
            clip.push(*operation);
        }
        clip
    }
}

fn run(
    plan: &ExportPlan,
    source: &Source,
    target: &Path,
    audio: AudioTarget,
) -> Result<ExportOutcome, ExportError> {
    let inputs = source.inputs();
    export(
        &common::orchestrator(),
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

/// The source's video packets from the in-point's keyframe, shown in range.
fn expected_video(source: &Source, from: i64, to: i64) -> Vec<String> {
    let all = common::packet_hashes(&source.path, "v:0");
    let start = all.iter().position(|p| p.pts == from).expect("in-point");
    all[start..]
        .iter()
        .filter(|p| p.pts >= from && p.pts < to)
        .map(|p| p.md5.clone())
        .collect()
}

/// A stream's duration in seconds, from its packets.
fn duration(path: &Path, selector: &str) -> f64 {
    let output = common::orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", selector.to_owned())
                .option("-show_entries", "packet=pts_time,duration_time")
                .option("-of", "csv=p=0")
                .input(path),
            Priority::Foreground,
        )
        .expect("ffprobe");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',');
            let pts: f64 = fields.next()?.parse().ok()?;
            let length: f64 = fields.next()?.parse().ok()?;
            Some(pts + length)
        })
        .fold(0.0, f64::max)
}

/// The MD5 of a stream decoded to PCM, as `format`.
fn decoded_md5(command: SidecarCommand) -> String {
    let collected = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&collected);
    common::orchestrator()
        .run(
            command.option("-f", "md5").output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("lock").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decodes");
    let text = String::from_utf8_lossy(&collected.lock().expect("lock")).into_owned();
    text.trim().to_owned()
}

#[test]
fn a_gain_change_re_encodes_the_sound_and_copies_every_picture() {
    let source = source("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan(vec![Track::new(
        1,
        TrackKind::Video,
        vec![source.clip(1, 0, from, to, &[Operation::Gain { db: 6.0 }])],
    )]);
    let audio_tiers: Vec<ExportTier> = plan
        .segments
        .iter()
        .filter(|s| s.media == Media::Audio)
        .map(|s| s.tier)
        .collect();
    assert_eq!(
        audio_tiers,
        vec![ExportTier::FullReEncode {
            reason: ReEncodeReason::AudioFilter
        }]
    );
    let target = common::scratch("export-audio-gain").join("louder.mp4");
    let outcome = run(&plan, &source, &target, AudioTarget::default()).expect("exports");

    assert_eq!(
        common::md5s(&common::packet_hashes(&target, "v:0")),
        expected_video(&source, from, to)
    );
    // The codec was chosen, and is named for the report.
    let encoding = outcome.audio.expect("sound was encoded");
    assert_eq!(encoding.codec, "aac");
    assert_eq!(encoding.kilobits, Some(256));
    assert!(!encoding.matches_copied);
    // Nothing on this path decodes the pictures: the one process that reads
    // the video stream copies it, and the encoder maps only the sound.
    let video = format!("0:{}", source.facts.video.as_ref().expect("video").stream);
    for command in &outcome.commands {
        if command.contains(&format!("-map {video}")) {
            assert!(command.contains("-c copy"), "{command}");
        }
        assert!(!command.contains("-c:v"), "{command}");
    }
    assert!(outcome.commands.iter().any(|c| c.contains("volume=6dB")));
    // Sound and pictures end together, to within one audio frame.
    let pictures = duration(&target, "v:0");
    let sound = duration(&target, "a:0");
    assert!((pictures - sound).abs() < 0.03, "{pictures} vs {sound}");
    assert!(common::decode_errors(&target).is_empty());
}

#[test]
fn a_lossless_target_decodes_to_exactly_the_filtered_sound() {
    let source = source("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(3));
    let plan = source.plan(vec![Track::new(
        1,
        TrackKind::Video,
        vec![source.clip(1, 0, from, to, &[Operation::Gain { db: -4.5 }])],
    )]);
    let target = common::scratch("export-audio-flac").join("quieter.mkv");
    let outcome = run(&plan, &source, &target, AudioTarget::Flac).expect("exports");
    let encoding = outcome.audio.expect("encoded");
    assert!(encoding.lossless);
    assert_eq!(encoding.codec, "flac");

    // The same processing, applied by FFmpeg directly to the source, at the
    // lossless targets' 24 bits.
    let tb = source.video_tb();
    let seconds = |ticks: i64| ticks as f64 * tb.num as f64 / tb.den as f64;
    let reference = decoded_md5(
        SidecarCommand::ffmpeg()
            .option("-v", "error")
            .flag("-copyts")
            .input(&source.path)
            .option("-map", format!("0:{}", source.audio_stream()))
            .option(
                "-af",
                format!(
                    "atrim=start={:.6}:end={:.6},asetpts=PTS-STARTPTS,volume=-4.5dB,aresample=48000,aformat=channel_layouts=stereo",
                    seconds(from),
                    seconds(to)
                ),
            )
            .option("-c:a", "pcm_s24le"),
    );
    let exported = decoded_md5(
        SidecarCommand::ffmpeg()
            .option("-v", "error")
            .input(&target)
            .option("-map", "0:a:0")
            .option("-c:a", "pcm_s24le"),
    );
    assert_eq!(exported, reference);
}

#[test]
fn a_sound_between_copies_is_encoded_to_match_them() {
    // Three clips of one file: the middle one louder. Its sound is encoded
    // to the copied sound's codec, and the sound either side is copied.
    let source = source("h264-high-closed-gop.mp4");
    let k: Vec<i64> = (0..5).map(|i| source.keyframe(i)).collect();
    let tb = source.video_tb();
    let frames = |ticks: i64| ticks * 30 * tb.num / tb.den;
    let first = source.clip(1, 0, k[0], k[1], &[]);
    let second = source.clip(2, frames(k[1]), k[1], k[2], &[Operation::Gain { db: 3.0 }]);
    let third = source.clip(3, frames(k[2]), k[2], k[4], &[]);
    let plan = source.plan(vec![Track::new(
        1,
        TrackKind::Video,
        vec![first, second, third],
    )]);
    let target = common::scratch("export-audio-hybrid").join("hybrid.mp4");
    let outcome = run(&plan, &source, &target, AudioTarget::default()).expect("exports");
    let encoding = outcome.audio.expect("encoded");
    assert!(encoding.matches_copied);
    assert_eq!(encoding.codec, "aac");
    assert!(common::decode_errors(&target).is_empty());
    let source_audio = common::md5s(&common::packet_hashes(&source.path, "a:0"));
    let output_audio = common::md5s(&common::packet_hashes(&target, "a:0"));
    // The copied stretches are the source's own packets.
    let copied = output_audio
        .iter()
        .filter(|hash| source_audio.contains(hash))
        .count();
    assert!(
        copied > output_audio.len() / 2,
        "{copied} of {}",
        output_audio.len()
    );
    assert!(copied < output_audio.len());
}

#[test]
fn overlapping_sounds_are_mixed() {
    let source = source("h264-high-closed-gop.mp4");
    let (from, to) = (source.keyframe(1), source.keyframe(4));
    let music = Clip::new(
        2,
        1,
        source.audio_stream(),
        Rational {
            num: 1,
            den: 48_000,
        },
        10,
        vec![Operation::Trim {
            from: 0,
            to: 48_000,
        }],
    );
    let plan = source.plan(vec![
        Track::new(1, TrackKind::Video, vec![source.clip(1, 0, from, to, &[])]),
        Track::new(2, TrackKind::Audio, vec![music]),
    ]);
    assert!(plan.segments.iter().any(|s| s.tier
        == ExportTier::FullReEncode {
            reason: ReEncodeReason::AudioMix
        }));
    let target = common::scratch("export-audio-mix").join("mixed.mp4");
    run(&plan, &source, &target, AudioTarget::default()).expect("exports");
    assert!(common::decode_errors(&target).is_empty());
    let pictures = duration(&target, "v:0");
    let sound = duration(&target, "a:0");
    assert!((pictures - sound).abs() < 0.03, "{pictures} vs {sound}");
}

#[test]
fn an_invalid_container_and_codec_pair_is_refused_before_anything_runs() {
    let source = source("vp9.webm");
    let video = source.facts.video.as_ref().expect("video");
    let end = video.end.expect("end");
    let plan = source.plan(vec![Track::new(
        1,
        TrackKind::Video,
        vec![source.clip(
            1,
            0,
            video.keyframes[0].pts.max(0),
            end,
            &[Operation::Gain { db: 2.0 }],
        )],
    )]);
    let dir = common::scratch("export-audio-refused");
    let target = dir.join("out.webm");
    // WebM holds Opus or Vorbis; AAC and FLAC are refused before any
    // process starts, and nothing is written.
    for audio in [AudioTarget::default(), AudioTarget::Flac] {
        assert!(matches!(
            run(&plan, &source, &target, audio),
            Err(ExportError::CodecNotInContainer { .. })
        ));
        assert!(!target.exists());
    }
    // Opus is what it takes.
    let outcome =
        run(&plan, &source, &target, AudioTarget::Opus { kilobits: 192 }).expect("exports");
    assert_eq!(outcome.audio.expect("encoded").codec, "opus");
    assert!(common::decode_errors(&target).is_empty());
}

#[test]
fn an_operation_the_preview_does_not_play_yet_is_refused() {
    let source = source("h264-high-closed-gop.mp4");
    let plan = source.plan(vec![Track::new(
        1,
        TrackKind::Video,
        vec![source.clip(
            1,
            0,
            source.keyframe(1),
            source.keyframe(2),
            &[Operation::Denoise { strength: 0.5 }],
        )],
    )]);
    let target = common::scratch("export-audio-denoise").join("out.mp4");
    assert!(matches!(
        run(&plan, &source, &target, AudioTarget::default()),
        Err(ExportError::Unsupported(_))
    ));
    assert!(!target.exists());
}
