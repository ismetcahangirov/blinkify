//! A clip's crop in the preview (#129), against the export that renders it
//! (#128): the preview decoder cuts the same rectangle out with the same
//! filter, so the frame on screen and the frame written show the same
//! region — for a landscape source and a turned one, and from a proxy. A
//! crop dragged while playing never stops the sound, a crop changed during
//! a seek lands on the new crop, and a cropped source that goes offline is
//! reported rather than panicking.
//!
//! **PSNR thresholds.** Preview against export: 35 dB, the smallest over the
//! frames compared — the sequence is the crop's own size, so neither side
//! scales and the difference is the export's encode alone. Preview against
//! FFmpeg's own crop of the decoded source: 45 dB, the RGB conversion's
//! rounding. A proxy against the original at the proxy's own resolution:
//! 18 dB — the proxy is a 540-line MJPEG copy, and its rectangle is rounded
//! outwards by up to three of its pixels, so on `testsrc2`'s fine detail it
//! measures about 23 dB where it shows the same region, while a region moved
//! by a sixth of the picture measures about 3 dB.

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
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use blinkify_engine::cache::Cache;
use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::plan::plan;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Flow, JobOptions, Priority, SidecarCommand};
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, Player, PlayerOptions, Segment, ShownFrame,
    SourceMedia, TransportCommand,
};
use blinkify_engine::probe::{Prober, Rational};
use blinkify_engine::project::crop::CropRect;
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, Operation, Project, SequenceSettings, SourceRef, Track, TrackKind,
};
use blinkify_engine::proxy::{MediaAsset, Proxies};
use blinkify_engine::time::{self, MICROSECONDS, Rounding};
use common::fixture::{Source, turned};

const EXPORT_THRESHOLD: f64 = 35.0;
const REFERENCE_THRESHOLD: f64 = 45.0;
const PROXY_THRESHOLD: f64 = 18.0;

fn rect(x: u32, y: u32, width: u32, height: u32) -> CropRect {
    CropRect {
        x,
        y,
        width,
        height,
    }
}

fn media(path: &Path) -> SourceMedia {
    let orchestrator = common::orchestrator();
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index = Arc::new(KeyframeIndex::open(path, &info, orchestrator, None).expect("index"));
    SourceMedia::new(path, info, index).expect("playable")
}

/// One clip of the whole source, cropped to `crop`, in `settings`.
fn project(source: &Source, settings: SequenceSettings, crop: CropRect) -> Project {
    let mut project = Project::new("crop", settings);
    project.sources.insert(
        1,
        SourceRef::of(&MediaAsset::new(source.path.clone()).export_source()).expect("ref"),
    );
    let clip: Clip = source.clip(
        1,
        0,
        source.keyframe(0),
        source.end(),
        &[Operation::crop(crop)],
    );
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, vec![clip])];
    project
}

fn player(plan: PlaybackPlan, bound: (u32, u32), audio: AudioChoice) -> Player {
    Player::new(
        common::orchestrator(),
        plan,
        &PlayerOptions {
            audio,
            max_width: bound.0,
            max_height: bound.1,
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

/// Raw RGBA of one frame FFmpeg decodes from `path` — autorotated, as a
/// player shows it — through `filter`.
fn frame_of(path: &Path, filter: &str, copyts: bool) -> Vec<u8> {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&bytes);
    let mut command = SidecarCommand::ffmpeg().option("-v", "error");
    if copyts {
        command = command.flag("-copyts");
    }
    common::orchestrator()
        .run(
            command
                .input(path)
                .option("-map", "0:v:0")
                .option("-vf", format!("{filter},format=rgba"))
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
        .expect("decodes");
    bytes.lock().expect("bytes").clone()
}

/// PSNR of two RGBA pictures over their colour channels, in dB.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "the pictures are not the same size");
    let (mut sum, mut count) = (0.0, 0.0);
    for (pa, pb) in a.chunks(4).zip(b.chunks(4)) {
        for channel in 0..3 {
            let d = f64::from(pa[channel]) - f64::from(pb[channel]);
            sum += d * d;
            count += 1.0;
        }
    }
    let mse = sum / count;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0 * 255.0 / mse).log10()
    }
}

/// The frame on screen at sequence frame `n`, upright, with its size.
fn shown_upright(player: &Player, n: i64) -> (Vec<u8>, u32, u32, Arc<ShownFrame>) {
    player.command(TransportCommand::Seek {
        position: at_frame(n),
    });
    let shown = landed(player);
    let picture = shown.picture.as_ref().expect("a picture");
    let (w, h) = if shown.rotation % 180 == 90 {
        (picture.height, picture.width)
    } else {
        (picture.width, picture.height)
    };
    (
        common::rotate_counter_clockwise(picture, shown.rotation),
        w,
        h,
        Arc::clone(&shown),
    )
}

#[test]
fn a_cropped_preview_frame_is_the_exported_frame_upright_or_turned() {
    // A landscape clip, and one turned a quarter: 640 × 360 coded, drawn on
    // 360 × 640 upright. Each sequence is the crop's own shape.
    for (rotation, crop) in [(0, rect(100, 60, 320, 180)), (90, rect(40, 100, 180, 320))] {
        let source = turned(&format!("preview-crop-{rotation}"), (640, 360), rotation);
        let settings = SequenceSettings {
            width: crop.width,
            height: crop.height,
            frame_rate: Rational { num: 30, den: 1 },
            ..SequenceSettings::default()
        };
        let project = project(&source, settings, crop);
        let timeline = evaluate(&project).expect("evaluates");
        let export_plan = plan(
            &timeline,
            &settings,
            &BTreeMap::from([(1, source.facts.clone())]),
        )
        .expect("plans");
        let target = common::scratch(&format!("preview-crop-{rotation}-out")).join("out.mp4");
        source
            .export(&export_plan, &target, AudioTarget::default())
            .expect("exports");

        let sources = BTreeMap::from([(1, Arc::new(media(&source.path)))]);
        let preview = PlaybackPlan::from_timeline(&timeline, &sources).expect("plan");
        let player = player(preview, (0, 0), AudioChoice::Silent);
        for n in [0, 17, 45, 89] {
            let (upright, w, h, shown) = shown_upright(&player, n);
            let picture = shown.picture.as_ref().expect("picture");
            // Delivered as coded — turned only at draw time — and the
            // crop's size, not the source's.
            assert_eq!(shown.rotation, rotation);
            assert_eq!((w, h), (crop.width, crop.height), "{rotation}° frame {n}");
            if rotation == 90 {
                assert_eq!((picture.width, picture.height), (crop.height, crop.width));
            }
            let exported = frame_of(&target, &format!("select=eq(n\\,{n})"), false);
            let measured = psnr(&upright, &exported);
            println!("{rotation}° frame {n}: preview against export {measured:.1} dB");
            assert!(
                measured > EXPORT_THRESHOLD,
                "{rotation}° frame {n}: preview against export {measured} dB"
            );
            // And against FFmpeg's own crop of the source, autorotated.
            let pts = picture.pts;
            let reference = frame_of(
                &source.path,
                &format!(
                    "select=eq(pts\\,{pts}),crop={}:{}:{}:{}",
                    crop.width, crop.height, crop.x, crop.y
                ),
                true,
            );
            let measured = psnr(&upright, &reference);
            assert!(
                measured > REFERENCE_THRESHOLD,
                "{rotation}° frame {n}: preview against FFmpeg's crop {measured} dB"
            );
        }
        player.close();
    }
}

#[test]
fn a_proxy_shows_the_region_the_original_shows() {
    // 1280 × 720 coded, 720 × 1280 upright; its proxy is 304 × 540, upright.
    let source = turned("preview-crop-proxy", (1280, 720), 90);
    let dir = common::scratch("preview-crop-proxy-cache");
    let proxy = Proxies::new(common::orchestrator(), Cache::new(dir, u64::MAX))
        .generate(&source.path, &source.info, &CancelToken::default(), |_| {})
        .expect("proxy");
    assert_eq!(proxy.frame_size(720, 1280), (304, 540));
    let original = Arc::new(media(&source.path));
    let proxied = Arc::new(media(&source.path).with_proxy(Some(proxy)));
    let (from, to) = (source.keyframe(0), source.end());
    let preview = |media: &Arc<SourceMedia>, crop: CropRect| {
        let mut segment = Segment::new(Arc::clone(media), from, to, 0);
        segment.crop = Some(crop);
        player(
            PlaybackPlan::new(vec![segment]).expect("plan"),
            (152, 270),
            AudioChoice::Silent,
        )
    };
    // A crop inside the picture, and one against its far corner, whose
    // edges land on the proxy's own.
    for crop in [rect(100, 200, 360, 640), rect(560, 1120, 160, 160)] {
        let (on_original, on_proxy) = (preview(&original, crop), preview(&proxied, crop));
        assert!(on_proxy.status().proxy, "the picture comes from the proxy");
        for n in [15, 75] {
            let (a, aw, ah, _) = shown_upright(&on_original, n);
            let (b, bw, bh, shown) = shown_upright(&on_proxy, n);
            assert_eq!(shown.rotation, 0, "a proxy is upright");
            assert_eq!((aw, ah), (bw, bh), "{crop:?}");
            let measured = psnr(&a, &b);
            println!("{crop:?} frame {n}: proxy against original {measured:.1} dB");
            assert!(
                measured > PROXY_THRESHOLD,
                "{crop:?} frame {n}: the proxy showed another region ({measured} dB)"
            );
        }
        on_original.close();
        on_proxy.close();
    }
    // The control: a region moved by a sixth of the picture is told apart.
    let (on_original, on_proxy) = (
        preview(&original, rect(100, 200, 360, 640)),
        preview(&proxied, rect(220, 420, 360, 640)),
    );
    let (a, ..) = shown_upright(&on_original, 15);
    let (b, ..) = shown_upright(&on_proxy, 15);
    let moved = psnr(&a, &b);
    println!("a moved region: {moved:.1} dB");
    assert!(
        moved < PROXY_THRESHOLD - 10.0,
        "a moved region measured {moved}"
    );
    on_original.close();
    on_proxy.close();
}

type Captured = Arc<Mutex<Vec<f32>>>;

#[test]
fn dragging_a_crop_while_playing_keeps_playing_without_a_gap_in_the_sound() {
    let source = turned("preview-crop-drag", (640, 360), 0);
    let media = Arc::new(media(&source.path));
    let (from, to) = (source.keyframe(0), source.end());
    let cropped = |crop: CropRect| {
        let mut segment = Segment::new(Arc::clone(&media), from, to, 0);
        segment.crop = Some(crop);
        PlaybackPlan::new(vec![segment]).expect("plan")
    };
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let player = player(
        cropped(rect(0, 0, 320, 180)),
        (0, 0),
        AudioChoice::Capture {
            sample_rate: 48_000,
            samples: Arc::clone(&captured),
        },
    );
    player.command(TransportCommand::Play);
    let deadline = Instant::now() + Duration::from_secs(10);
    while captured
        .lock()
        .expect("lock")
        .iter()
        .all(|s| s.abs() < 1e-6)
    {
        assert!(Instant::now() < deadline, "nothing played");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A drag of a crop handle: a new rectangle every 60 ms, faster than a
    // decoder starts, ending on a smaller one.
    for step in 0..20 {
        player.set_plan(cropped(rect(step * 16, step * 8, 320, 180)));
        std::thread::sleep(Duration::from_millis(60));
    }
    player.set_plan(cropped(rect(320, 180, 160, 90)));
    std::thread::sleep(Duration::from_millis(700));
    let (_, position) = player.position();
    let last = player
        .next_frame(0, Duration::from_secs(2))
        .expect("a frame");
    player.close();
    // Every 10 ms of the tone, from its start to the close, is sounding.
    let heard = captured.lock().expect("lock").clone();
    let left: Vec<f32> = heard.chunks(2).map(|frame| frame[0]).collect();
    let start = left.iter().position(|s| s.abs() > 1e-6).expect("sound");
    let quietest = left[start..]
        .chunks(480)
        .filter(|chunk| chunk.len() == 480)
        .map(|chunk| {
            let mean = chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 480.0;
            10.0 * mean.max(1e-20).log10()
        })
        .fold(f64::INFINITY, f64::min);
    assert!(quietest > -60.0, "the sound dropped out: {quietest} dBFS");
    // The clock ran through it all, and the last crop is what is drawn.
    assert!(position > 1_500_000, "{position}");
    let picture = last.picture.as_ref().expect("a picture");
    assert_eq!((picture.width, picture.height), (160, 90));
}

#[test]
fn a_crop_changed_during_a_seek_lands_on_the_new_crop() {
    let source = turned("preview-crop-seek", (640, 360), 90);
    let media = Arc::new(media(&source.path));
    let (from, to) = (source.keyframe(0), source.end());
    let cropped = |crop: CropRect| {
        let mut segment = Segment::new(Arc::clone(&media), from, to, 0);
        segment.crop = Some(crop);
        PlaybackPlan::new(vec![segment]).expect("plan")
    };
    let player = player(cropped(rect(0, 0, 360, 320)), (0, 0), AudioChoice::Silent);
    let _ = landed(&player);
    // The seek is asked for, and before its frame is up, the crop changes.
    let after = rect(40, 100, 180, 320);
    player.command(TransportCommand::Seek {
        position: at_frame(70),
    });
    player.set_plan(cropped(after));
    let shown = landed(&player);
    assert!(!player.status().resolving);
    let picture = shown.picture.as_ref().expect("a picture");
    assert_eq!(
        (picture.width, picture.height),
        (320, 180),
        "the new crop, coded"
    );
    let upright = common::rotate_counter_clockwise(picture, shown.rotation);
    let reference = frame_of(
        &source.path,
        &format!("select=eq(pts\\,{}),crop=180:320:40:100", picture.pts),
        true,
    );
    let measured = psnr(&upright, &reference);
    assert!(
        measured > REFERENCE_THRESHOLD,
        "the new crop against FFmpeg's {measured} dB"
    );
    player.close();
}

#[test]
fn a_cropped_source_that_goes_offline_is_reported_not_a_panic() {
    let dir = common::scratch("preview-crop-offline");
    let path = dir.join("going away.avi");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input("testsrc2=size=320x180:rate=30:duration=4")
                .option("-c:v", "mjpeg")
                .option("-q:v", "5")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("clip written");
    let plan_for = || {
        let media = Arc::new(media(&path));
        let (from, to) = media.full_range();
        let mut segment = Segment::new(media, from, to, 0);
        segment.crop = Some(rect(80, 40, 160, 90));
        PlaybackPlan::new(vec![segment]).expect("plan")
    };
    let plan = plan_for();
    let playing = player(plan.clone(), (0, 0), AudioChoice::Silent);
    playing.command(TransportCommand::Play);
    let first = playing
        .next_frame(0, Duration::from_secs(5))
        .expect("a frame");
    let picture = first.picture.as_ref().expect("a picture");
    assert_eq!((picture.width, picture.height), (160, 90));
    playing.close();
    std::fs::remove_file(&path).expect("deletable once nothing holds it");
    let orphan = player(plan, (0, 0), AudioChoice::Silent);
    let deadline = Instant::now() + Duration::from_secs(5);
    while orphan.stats().error.is_none() {
        assert!(Instant::now() < deadline, "the error never surfaced");
        let _ = orphan.next_frame(u64::MAX, Duration::from_millis(10));
    }
    let error = orphan.stats().error.expect("an error");
    assert!(error.contains("no longer available"), "{error}");
    orphan.close();
}
