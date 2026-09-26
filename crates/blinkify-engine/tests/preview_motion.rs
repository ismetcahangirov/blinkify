//! Held and reversed clips in the preview (#113), against the export that
//! renders them (#55): the frame on screen at every sequence frame is the one
//! the evaluator names and the one the export writes there, a hold lasts as
//! long in both, a reversed clip is heard backwards, and a long reversed clip
//! plays in bounded memory.
//!
//! The sources are VP9 made here by the sidecar's software encoder, so the
//! export runs on every machine (H.264 needs a hardware encoder, ADR-0003),
//! in MP4, whose 1/15360 time base puts every frame of 30 fps on a whole
//! tick: the evaluator's arithmetic is exact there, and preview and export
//! can be compared frame for frame.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::integer_division
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::plan::{ExportPlan, plan};
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{Flow, JobOptions, Orchestrator, Priority, SidecarCommand};
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, PlaybackState, Player, PlayerOptions, PreviewSpeed,
    ShownFrame, SourceMedia, TransportCommand, reverse_memory_bound,
};
use blinkify_engine::probe::{Prober, Rational};
use blinkify_engine::project::evaluate::{Motion, Timeline, evaluate};
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;
use blinkify_engine::time::{self, MICROSECONDS, Rounding};
use common::fixture::{Source, encoders};

const WIDTH: usize = 320;
const HEIGHT: usize = 180;
const FRAME_BYTES: usize = WIDTH * HEIGHT * 4;

/// `seconds` of 30 fps VP9 in MP4, a keyframe every second.
fn vp9(name: &str, seconds: u32) -> Source {
    let path = common::scratch(&format!("motion-{name}")).join("source.mp4");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .lavfi_input(&format!(
                    "testsrc2=size={WIDTH}x{HEIGHT}:rate=30:duration={seconds}"
                ))
                .option("-pix_fmt", "yuv420p")
                .option("-c:v", "libvpx-vp9")
                .option("-deadline", "realtime")
                .option("-b:v", "0")
                .option("-crf", "30")
                .option("-g", "30")
                .option("-keyint_min", "30")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("encodes");
    Source::at(path, Some(encoders()))
}

fn media(orchestrator: &Orchestrator, path: &Path) -> Arc<SourceMedia> {
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index =
        Arc::new(KeyframeIndex::open(path, &info, orchestrator.clone(), None).expect("index"));
    Arc::new(SourceMedia::new(path, info, index).expect("playable"))
}

/// One video track of `clips` over `path`, in a sequence of `settings`.
fn project(path: &Path, settings: SequenceSettings, clips: Vec<Clip>) -> Project {
    let mut project = Project::new("motion", settings);
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(path.to_path_buf()).export_source()).expect("ref"),
    );
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, clips)];
    project
}

/// The graph as both consumers see it: the evaluated timeline the preview
/// plays and the plan the export runs, from one evaluation.
fn both(source: &Source, clips: Vec<Clip>) -> (Timeline, ExportPlan) {
    let settings = SequenceSettings::matching(&source.video().geometry).expect("valid");
    let project = project(&source.path, settings, clips);
    let timeline = evaluate(&project).expect("evaluates");
    let plan = plan(
        &timeline,
        &project.sequence.settings,
        &BTreeMap::from([(1, source.facts.clone())]),
    )
    .expect("plans");
    (timeline, plan)
}

fn player(orchestrator: &Orchestrator, plan: PlaybackPlan, audio: AudioChoice) -> Player {
    Player::new(
        orchestrator.clone(),
        plan,
        &PlayerOptions {
            audio,
            // No bound: frames at the source's size, as the export decodes.
            max_width: 0,
            max_height: 0,
            default_device: DefaultDevice::System,
            models: None,
        },
    )
}

/// The newest frame once the player has stopped resolving and nothing newer
/// arrives for a moment.
fn landed(player: &Player) -> Arc<ShownFrame> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while player.status().resolving {
        assert!(Instant::now() < deadline, "the frame never arrived");
        let _ = player.next_frame(u64::MAX, Duration::from_millis(10));
    }
    let mut last = player
        .next_frame(0, Duration::from_secs(10))
        .expect("a frame");
    while let Some(newer) = player.next_frame(last.seq, Duration::from_millis(200)) {
        last = newer;
    }
    last
}

/// Where sequence frame `n` of 30 fps is, rounded up as a seek to it is.
fn at_frame(n: i64) -> i64 {
    time::rescale(n, Rational { num: 1, den: 30 }, MICROSECONDS, Rounding::Up).expect("µs")
}

/// Where sequence frame `n` of 30 fps starts, rounded down as a segment's
/// start is.
fn frame_start(n: i64) -> i64 {
    time::rescale(
        n,
        Rational { num: 1, den: 30 },
        MICROSECONDS,
        Rounding::Down,
    )
    .expect("µs")
}

/// Every frame of `path` in RGBA, in order.
fn decoded(path: &Path) -> Vec<Vec<u8>> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(path)
                .option("-map", "0:v:0")
                .option("-fps_mode", "passthrough")
                .option("-pix_fmt", "rgba")
                .option("-f", "rawvideo")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("sink").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("decodes");
    let bytes = bytes.lock().expect("bytes");
    bytes.chunks(FRAME_BYTES).map(<[u8]>::to_vec).collect()
}

/// The source frame at `pts`, decoded on its own: from the start of the
/// file, no seek — what the preview must show bit for bit.
fn reference_frame(path: &Path, pts: i64) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    common::orchestrator()
        .run(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .flag("-copyts")
                .flag("-noautorotate")
                .input(path)
                .option("-map", "0:v:0")
                .option("-vf", format!("select=eq(pts\\,{pts}),format=rgba"))
                .option("-fps_mode", "passthrough")
                .option("-frames:v", "1")
                .option("-f", "rawvideo")
                .output_stdout(),
            Priority::Foreground,
            JobOptions::default().on_chunk(move |chunk| {
                sink.lock().expect("sink").extend_from_slice(chunk);
                Flow::Continue
            }),
        )
        .wait()
        .expect("reference decode");
    bytes.lock().expect("bytes").clone()
}

/// Peak signal to noise, in dB, of two RGBA pictures over their colour.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "pictures of one size");
    let (mut sum, mut count) = (0.0_f64, 0.0_f64);
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        for c in 0..3 {
            let d = f64::from(pa[c]) - f64::from(pb[c]);
            sum += d * d;
            count += 1.0;
        }
    }
    let mse = sum / count;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0_f64 * 255.0 / mse).log10()
    }
}

/// The output frame `picture` is: the index of the one it matches best, and
/// how well.
fn best_match(picture: &[u8], output: &[Vec<u8>]) -> (usize, f64) {
    output
        .iter()
        .enumerate()
        .map(|(i, frame)| (i, psnr(picture, frame)))
        .fold((0, f64::NEG_INFINITY), |best, next| {
            if next.1 > best.1 { next } else { best }
        })
}

fn picture(frame: &ShownFrame) -> &[u8] {
    &frame.picture.as_ref().expect("a picture").pixels
}

#[test]
fn a_held_clip_shows_the_frame_the_export_writes_for_as_long() {
    let orchestrator = common::orchestrator();
    let source = vp9("hold", 4);
    let (k0, k1, k2, k3) = (
        source.keyframe(0),
        source.keyframe(1),
        source.keyframe(2),
        source.keyframe(3),
    );
    assert_eq!(source.video().time_base, Rational { num: 1, den: 15360 });
    let held = k1 + 5 * 512;
    let (timeline, export_plan) = both(
        &source,
        vec![
            source.clip(1, 0, k0, k1, &[]),
            source.clip(2, 30, held, held + 1, &[Operation::Freeze { frames: 45 }]),
            source.clip(3, 75, k2, k3, &[]),
        ],
    );
    let target = common::scratch("motion-hold-export").join("held.mkv");
    source
        .export(&export_plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    let exported = decoded(&target);
    assert_eq!(exported.len(), 30 + 45 + 30);

    let sources = BTreeMap::from([(1, media(&orchestrator, &source.path))]);
    let plan = PlaybackPlan::from_timeline(&timeline, &sources).expect("plan");
    // The same length as the export's: sequence frames 30 to 75.
    let (_, frozen) = plan.segment_at(at_frame(30)).expect("the frozen");
    assert_eq!(frozen.motion(), Some(Motion::Hold));
    assert_eq!(frozen.timeline_start, frame_start(30));
    assert_eq!(frozen.timeline_end(), frame_start(75));
    assert_eq!(
        plan.segment_at(at_frame(75)).map(|(_, s)| s.clip),
        Some(Some(3))
    );

    let reference = reference_frame(&source.path, held);
    let player = player(&orchestrator, plan, AudioChoice::Silent);
    for n in [30, 52, 74] {
        player.command(TransportCommand::Seek {
            position: at_frame(n),
        });
        let shown = landed(&player);
        let frame = shown.picture.as_ref().expect("a picture");
        assert_eq!(frame.pts, held, "frame {n}");
        assert!(
            picture(&shown) == reference.as_slice(),
            "frame {n}: not the source's frame"
        );
        // The frame's place moves on with the sequence frame, as the export
        // writes one frame per sequence frame.
        assert_eq!(shown.frame_number, n, "frame {n}");
        // The export wrote this picture here, and not what is either side.
        let here = psnr(picture(&shown), &exported[n as usize]);
        assert!(here > 30.0, "frame {n}: PSNR {here} against the export");
        for other in [29, 75] {
            assert!(
                psnr(picture(&shown), &exported[other]) < here - 3.0,
                "frame {n} also matches export frame {other}"
            );
        }
    }
    // Every frame the export wrote for the hold is that picture.
    let mut worst = f64::INFINITY;
    for frame in &exported[30..75] {
        worst = worst.min(psnr(&reference, frame));
    }
    assert!(worst > 30.0, "the export's hold, worst PSNR {worst}");
    player.command(TransportCommand::Seek {
        position: at_frame(75),
    });
    assert_eq!(landed(&player).picture.as_ref().expect("picture").pts, k2);

    // Stepping goes through the hold a sequence frame at a time, and out of
    // it onto the neighbours' frames.
    player.command(TransportCommand::Seek {
        position: at_frame(29),
    });
    let _ = landed(&player);
    let step = |frames: i32| {
        player.command(TransportCommand::Step { frames });
        let shown = landed(&player);
        (
            shown.frame_number,
            shown.picture.as_ref().expect("picture").pts,
        )
    };
    assert_eq!(step(1), (30, held));
    assert_eq!(step(2), (32, held));
    assert_eq!(step(-3), (29, k0 + 29 * 512));
    player.command(TransportCommand::Seek {
        position: at_frame(74),
    });
    let _ = landed(&player);
    assert_eq!(step(1), (75, k2));
    assert_eq!(step(-1), (74, held));
    player.close();
}

#[test]
fn a_reversed_clip_shows_frame_n_where_the_export_writes_frame_n() {
    let orchestrator = common::orchestrator();
    let source = vp9("reverse", 4);
    let (from, to) = (source.keyframe(0), source.keyframe(3));
    let (timeline, export_plan) = both(
        &source,
        vec![source.clip(1, 0, from, to, &[Operation::Reverse])],
    );
    let target = common::scratch("motion-reverse-export").join("reversed.mkv");
    source
        .export(&export_plan, &target, AudioTarget::Opus { kilobits: 128 })
        .expect("exports");
    let exported = decoded(&target);
    assert_eq!(exported.len(), 90);

    let media = media(&orchestrator, &source.path);
    let stream = media.video.as_ref().expect("video").index;
    let placement = timeline.placements().next().expect("the clip").clone();
    let sources = BTreeMap::from([(1, Arc::clone(&media))]);
    let plan = PlaybackPlan::from_timeline(&timeline, &sources).expect("plan");
    let player = player(&orchestrator, plan, AudioChoice::Silent);
    // Across the clip, both sides of every group of pictures and of the
    // decoder's chunks.
    for n in [0, 1, 2, 14, 29, 30, 31, 45, 59, 60, 61, 75, 88, 89] {
        player.command(TransportCommand::Seek {
            position: at_frame(n),
        });
        let shown = landed(&player);
        let pts = shown.picture.as_ref().expect("a picture").pts;
        // The frame the evaluator names for sequence frame n.
        let named = media
            .index
            .frame_at_or_before(stream, placement.source_at(n).expect("in the clip"))
            .expect("table")
            .expect("a frame");
        assert_eq!(pts, named, "frame {n}");
        assert!(
            picture(&shown) == reference_frame(&source.path, pts).as_slice(),
            "frame {n}: not the source's frame"
        );
        // …and the frame the export wrote at n: its best match is n itself.
        let (best, score) = best_match(picture(&shown), &exported);
        assert_eq!(best, n as usize, "frame {n} matches export frame {best}");
        assert!(score > 30.0, "frame {n}: PSNR {score} against the export");
    }

    // A step forward on the timeline is a step back through the source.
    player.command(TransportCommand::Seek {
        position: at_frame(30),
    });
    let before = landed(&player).picture.as_ref().expect("picture").pts;
    player.command(TransportCommand::Step { frames: 1 });
    let stepped = landed(&player);
    assert_eq!(stepped.frame_number, 31);
    assert!(stepped.picture.as_ref().expect("picture").pts < before);

    // Played, it runs backwards without stalling.
    player.command(TransportCommand::Seek { position: 0 });
    let _ = landed(&player);
    player.command(TransportCommand::Play);
    let mut shown: Vec<i64> = Vec::new();
    let mut last = 0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.status().state == PlaybackState::Playing {
        assert!(Instant::now() < deadline, "playback never ended");
        if let Some(frame) = player.next_frame(last, Duration::from_millis(100)) {
            last = frame.seq;
            if let Some(picture) = &frame.picture
                && shown.last() != Some(&picture.pts)
            {
                shown.push(picture.pts);
            }
        }
    }
    assert!(
        shown.windows(2).all(|pair| pair[1] < pair[0]),
        "not backwards: {shown:?}"
    );
    assert!(shown.len() >= 60, "{} of 90 frames shown", shown.len());
    assert_eq!(player.stats().error, None);
    player.close();
}

#[test]
fn a_long_reversed_clip_plays_in_bounded_memory() {
    let orchestrator = common::orchestrator();
    let source = vp9("reverse-long", 20);
    let (from, to) = (source.first(), source.end());
    let (timeline, _) = both(
        &source,
        vec![source.clip(1, 0, from, to, &[Operation::Reverse])],
    );
    let sources = BTreeMap::from([(1, media(&orchestrator, &source.path))]);
    let plan = PlaybackPlan::from_timeline(&timeline, &sources).expect("plan");
    let bound = reverse_memory_bound(FRAME_BYTES);
    // Decoded whole, the clip is far more than the bound.
    assert!(bound * 4 < 600 * FRAME_BYTES, "{bound} bytes");

    let player = player(&orchestrator, plan, AudioChoice::Silent);
    player.command(TransportCommand::SetSpeed {
        speed: PreviewSpeed::Double,
    });
    let _ = landed(&player);
    player.command(TransportCommand::Play);
    let started = Instant::now();
    let mut last = 0;
    let mut pictures = 0;
    // Eight seconds of the clip, a dozen chunks and more.
    while started.elapsed() < Duration::from_secs(4) {
        if let Some(frame) = player.next_frame(last, Duration::from_millis(100)) {
            last = frame.seq;
            pictures += 1;
        }
    }
    let (_, position) = player.position();
    player.close();
    assert!(position > 6_000_000, "played to {position}");
    assert!(pictures > 60, "{pictures} frames shown");
    let peak = player.reverse_peak_bytes();
    assert!(peak > 0, "nothing was measured");
    assert!(peak <= bound, "held {peak} bytes, bound {bound}");
}

/// Pictures and a 440 Hz tone that rises from silence to 0.8 over four
/// seconds: backwards, it falls.
fn rising_tone(dir: &Path) -> PathBuf {
    let path = dir.join("tone.mkv");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .lavfi_input("testsrc2=size=160x90:rate=30:duration=4")
                .lavfi_input(
                    "aevalsrc=exprs=0.2*t*sin(2*PI*440*t)|0.2*t*sin(2*PI*440*t):s=48000:d=4",
                )
                .option("-c:v", "mjpeg")
                .option("-c:a", "flac")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("source");
    path
}

/// Root mean square of `samples` (interleaved stereo) from `from` to `to`
/// seconds at 48 kHz.
fn rms(samples: &[f32], from: f64, to: f64) -> f64 {
    let at = |seconds: f64| ((seconds * 48_000.0) as usize * 2).min(samples.len());
    let window = &samples[at(from)..at(to)];
    let sum: f64 = window.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / window.len().max(1) as f64).sqrt()
}

#[test]
fn a_reversed_clip_is_heard_backwards_and_a_held_one_is_silent() {
    let orchestrator = common::orchestrator();
    let dir = common::scratch("motion-sound");
    let path = rising_tone(&dir);
    let tb = Rational { num: 1, den: 1000 };
    let settings = SequenceSettings {
        frame_rate: Rational { num: 30, den: 1 },
        ..SequenceSettings::default()
    };
    let sources = BTreeMap::from([(1, media(&orchestrator, &path))]);

    // A hold is silent: nothing on any sound track sounds under it.
    let held = project(
        &path,
        settings,
        vec![
            Clip::new(1, 1, 0, tb, 0, vec![Operation::Trim { from: 0, to: 1000 }]),
            Clip::new(
                2,
                1,
                0,
                tb,
                30,
                vec![
                    Operation::Trim {
                        from: 2000,
                        to: 2001,
                    },
                    Operation::Freeze { frames: 60 },
                ],
            ),
        ],
    );
    let plan =
        PlaybackPlan::from_timeline(&evaluate(&held).expect("evaluates"), &sources).expect("plan");
    for (_, segments) in plan.sound_tracks() {
        assert!(
            segments.iter().all(|s| s.clip != Some(2)),
            "the hold sounds"
        );
    }
    assert_eq!(
        plan.segment_at(at_frame(60)).and_then(|(_, s)| s.motion()),
        Some(Motion::Hold)
    );

    // A reversed clip falls from loud to quiet, with no gap between the
    // decoder's chunks.
    let reversed = project(
        &path,
        settings,
        vec![Clip::new(
            1,
            1,
            0,
            tb,
            0,
            vec![Operation::Trim { from: 0, to: 4000 }, Operation::Reverse],
        )],
    );
    let plan = PlaybackPlan::from_timeline(&evaluate(&reversed).expect("evaluates"), &sources)
        .expect("plan");
    let samples = Arc::new(Mutex::new(Vec::new()));
    let player = player(
        &orchestrator,
        plan,
        AudioChoice::Capture {
            sample_rate: 48_000,
            samples: Arc::clone(&samples),
        },
    );
    let _ = landed(&player);
    player.command(TransportCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.position().1 < 3_800_000 {
        assert!(Instant::now() < deadline, "playback stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
    player.close();
    let captured = samples.lock().expect("samples").clone();
    // The capture runs from the player's start; the clip's first sound — at
    // its loudest, backwards — is where playback began.
    let onset = captured
        .iter()
        .position(|sample| sample.abs() > 0.05)
        .expect("something was heard");
    let heard = &captured[onset - onset % 2..];
    // At timeline τ the source is at 4 − τ, where the tone is 0.2 (4 − τ).
    let early = rms(heard, 0.25, 0.75);
    let late = rms(heard, 3.0, 3.5);
    assert!(early > 0.3, "early {early}");
    assert!(
        early > 3.0 * late,
        "early {early}, late {late}: not backwards"
    );
    for step in 0..60 {
        let from = 0.3 + f64::from(step) * 0.05;
        let level = rms(heard, from, from + 0.05);
        assert!(level > 0.05, "a gap at {from:.2} s: {level}");
    }
}
