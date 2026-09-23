//! The shared evaluator against the real sidecar (#30): preview and export
//! agree on every frame of generated graphs, a proxy changes nothing about
//! the content, a graph change shows at once and writes no file, an
//! unchanged clip keeps its decoder, and speed and gain are heard and seen.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::cast_possible_wrap,
    clippy::range_plus_one,
    clippy::integer_division
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use blinkify_engine::cache::Cache;
use blinkify_engine::export::ExportContent;
use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{CancelToken, Limits, Orchestrator, Priority, SidecarCommand};
use blinkify_engine::playback::{
    AudioChoice, DefaultDevice, PlaybackPlan, Player, PlayerOptions, ShownFrame, SourceMedia,
    TransportCommand,
};
use blinkify_engine::probe::{Prober, Rational};
use blinkify_engine::project::evaluate::evaluate;
use blinkify_engine::project::{
    Clip, ClipId, Operation, Project, SequenceSettings, SourceId, Track, TrackKind,
};
use blinkify_engine::proxy::{MediaAsset, Proxies};
use blinkify_engine::time::{self, MICROSECONDS, Rounding};

const RATE: u32 = 48_000;

type Captured = Arc<Mutex<Vec<f32>>>;

fn orchestrator() -> Orchestrator {
    Orchestrator::new(common::sidecar(), Limits::for_this_machine())
}

fn media(orchestrator: &Orchestrator, path: &Path, cache: Option<Cache>) -> SourceMedia {
    let info = Prober::new(orchestrator.clone())
        .probe(path)
        .expect("probe");
    let index =
        Arc::new(KeyframeIndex::open(path, &info, orchestrator.clone(), cache).expect("index"));
    SourceMedia::new(path, info, index).expect("playable")
}

/// Pictures and a 1 kHz tone at full scale / 4, `seconds` long: every frame
/// a keyframe (MJPEG), the sound lossless (FLAC).
fn clip_file(orchestrator: &Orchestrator, dir: &Path, seconds: u32) -> PathBuf {
    let path = dir.join("source.mkv");
    orchestrator
        .run_to_end(
            SidecarCommand::ffmpeg()
                .lavfi_input(&format!(
                    "testsrc2=size=160x90:rate=30:duration={seconds}"
                ))
                .lavfi_input(&format!(
                    "sine=frequency=1000:sample_rate=48000,volume=4:precision=double,aformat=channel_layouts=stereo,atrim=duration={seconds}"
                ))
                .option("-c:v", "mjpeg")
                .option("-c:a", "flac")
                .output_file(&path),
            Priority::Foreground,
        )
        .expect("source");
    path
}

fn player(orchestrator: &Orchestrator, plan: PlaybackPlan) -> (Player, Captured) {
    let samples: Captured = Arc::new(Mutex::new(Vec::new()));
    let player = Player::new(
        orchestrator.clone(),
        plan,
        &PlayerOptions {
            audio: AudioChoice::Capture {
                sample_rate: RATE,
                samples: Arc::clone(&samples),
            },
            max_width: 160,
            max_height: 90,
            default_device: DefaultDevice::System,
        },
    );
    (player, samples)
}

/// The newest frame once the player has stopped resolving and nothing newer
/// arrives for a moment.
fn landed(player: &Player) -> Arc<ShownFrame> {
    let deadline = Instant::now() + Duration::from_secs(20);
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

fn project_with(source: &Path, tracks: Vec<Track>, rate: Rational) -> Project {
    let mut project = Project::new(
        "evaluate",
        SequenceSettings {
            frame_rate: rate,
            ..SequenceSettings::default()
        },
    );
    project
        .add_source(&MediaAsset::new(source.to_path_buf()).export_source())
        .expect("source");
    project.sequence.tracks = tracks;
    project
}

fn plan_of(project: &Project, sources: &BTreeMap<SourceId, Arc<SourceMedia>>) -> PlaybackPlan {
    PlaybackPlan::from_timeline(&evaluate(project).expect("evaluate"), sources).expect("plan")
}

/// xorshift64*, seeded, so a failing graph reproduces.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: u64) -> i64 {
        (self.next() % bound) as i64
    }
}

#[test]
fn preview_and_export_agree_on_every_frame_of_generated_graphs() {
    let orchestrator = orchestrator();
    let dir = common::scratch("evaluate-agree");
    let video_path = common::corpus("vfr-screen.mp4");
    let sound_path = clip_file(&orchestrator, &dir, 6);
    let video = Arc::new(media(&orchestrator, &video_path, None));
    let sound = Arc::new(media(&orchestrator, &sound_path, None));
    let info = Prober::new(orchestrator.clone())
        .probe(&video_path)
        .expect("probe");
    let proxy = Proxies::new(
        orchestrator.clone(),
        Cache::new(dir.join("proxies"), u64::MAX),
    )
    .generate(&video_path, &info, &CancelToken::default(), |_| {})
    .expect("proxy");
    let proxied = Arc::new(media(&orchestrator, &video_path, None).with_proxy(Some(proxy)));

    let video_stream = video.video.as_ref().expect("video").index;
    let video_tb = video.time_base();
    let (video_in, video_out) = video.full_range();
    let audio = sound.audio.expect("audio");
    let (sound_in, sound_out) = sound.full_range();
    let audio_range = (
        time::rescale(sound_in, sound.time_base(), audio.time_base, Rounding::Up).expect("in"),
        time::rescale(
            sound_out,
            sound.time_base(),
            audio.time_base,
            Rounding::Down,
        )
        .expect("out"),
    );

    let mut random = Random(0x5eed_0030);
    let rates = [(30, 1), (30_000, 1001), (25, 1), (60, 1), (24, 1)];
    let speeds = [(1, 1), (1, 2), (3, 2), (2, 1), (3, 1)];
    let mut positions_checked = 0;
    for case in 0..25 {
        let (num, den) = rates[random.below(rates.len() as u64) as usize];
        let rate = Rational { num, den };
        let mut project = project_with(&video_path, Vec::new(), rate);
        let sound_id = project
            .add_source(&MediaAsset::new(sound_path.clone()).export_source())
            .expect("source");
        let mut clip_id: ClipId = 0;
        let mut track = |kind: TrackKind,
                         id: u32,
                         source: SourceId,
                         stream: u32,
                         tb: Rational,
                         (first, last): (i64, i64),
                         snap: &dyn Fn(i64) -> i64,
                         random: &mut Random| {
            let mut clips = Vec::new();
            let mut at = random.below(20);
            for _ in 0..1 + random.below(4) {
                clip_id += 1;
                let span = last - first;
                // Half the cuts on a real frame boundary, as an editor makes
                // them: where a rounding error shows as the previous frame.
                let mut from = first + random.below((span / 2) as u64);
                let mut to = from + 1 + random.below((last - from) as u64);
                if random.below(2) == 0 {
                    from = snap(from).max(first);
                    to = snap(to).max(from + 1);
                }
                let (n, d) = speeds[random.below(speeds.len() as u64) as usize];
                let mut operations = vec![Operation::Trim { from, to }];
                if (n, d) != (1, 1) {
                    operations.push(Operation::Speed {
                        ratio: Rational { num: n, den: d },
                    });
                }
                if random.below(2) == 0 {
                    operations.push(Operation::Gain {
                        db: -(random.below(120) as f64) / 10.0,
                    });
                }
                clips.push(Clip::new(clip_id, source, stream, tb, at, operations));
                // The next clip after this one, with a gap or none: its
                // length in frames, rounded up as the evaluator does.
                let played = Rational {
                    num: tb.num * d,
                    den: tb.den * n,
                };
                let sequence = Rational { num: den, den: num };
                let length =
                    time::rescale(to - from, played, sequence, Rounding::Up).expect("length");
                at += length + random.below(3);
            }
            Track { id, kind, clips }
        };
        let pictures = track(
            TrackKind::Video,
            1,
            1,
            video_stream,
            video_tb,
            (video_in, video_out),
            &|tick| {
                video
                    .index
                    .frame_at_or_before(video_stream, tick)
                    .expect("table")
                    .unwrap_or(tick)
            },
            &mut random,
        );
        let music = track(
            TrackKind::Audio,
            2,
            sound_id,
            audio.index,
            audio.time_base,
            audio_range,
            &|tick| tick,
            &mut random,
        );
        project.sequence.tracks = vec![pictures, music];

        let export = ExportContent::of(&project).expect("export");
        let timeline = &export.timeline;
        let original = BTreeMap::from([(1, Arc::clone(&video)), (sound_id, Arc::clone(&sound))]);
        let with_proxy =
            BTreeMap::from([(1, Arc::clone(&proxied)), (sound_id, Arc::clone(&sound))]);
        let plan = PlaybackPlan::from_timeline(timeline, &original).expect("plan");
        let plan_proxy = PlaybackPlan::from_timeline(timeline, &with_proxy).expect("plan");

        // The proxy changes the pictures' source of pixels, never the content.
        let content = |plan: &PlaybackPlan| -> Vec<(Option<ClipId>, i64, i64, i64, Rational)> {
            plan.sound_tracks()
                .iter()
                .flat_map(|(_, segments)| segments.iter())
                .map(|s| (s.clip, s.source_in, s.source_out, s.timeline_start, s.speed))
                .collect()
        };
        assert_eq!(content(&plan), content(&plan_proxy), "case {case}");
        assert!(
            plan_proxy
                .segments()
                .iter()
                .all(|s| s.source.proxy.is_some())
        );

        for position in 0..timeline.length() {
            let t = time::rescale(position, timeline.time_base, MICROSECONDS, Rounding::Up)
                .expect("µs");
            let exported = export.at(position);
            let placed = |track: u32| exported.clips.iter().find(|c| c.track == track);

            // Pictures: the same clip, the same range, speed and chain, and
            // the same source frame on screen.
            match (placed(1), plan.segment_at(t)) {
                (None, None) => {}
                (Some(applied), Some((_, segment))) => {
                    assert_eq!(segment.clip, Some(applied.clip), "case {case} @{position}");
                    let placement = timeline
                        .placements()
                        .find(|p| p.clip == applied.clip)
                        .expect("placement");
                    assert_eq!(
                        (segment.source_in, segment.source_out, segment.speed),
                        (placement.source_in, placement.source_out, placement.speed),
                        "case {case} @{position}"
                    );
                    assert_eq!(segment.audio, placement.audio, "case {case} @{position}");
                    let frame = |tick: i64| {
                        video
                            .index
                            .frame_at_or_before(video_stream, tick)
                            .expect("table")
                    };
                    assert_eq!(
                        frame(segment.source_at(t)),
                        frame(applied.source_tick),
                        "case {case} @{position}: preview tick {} export tick {}",
                        segment.source_at(t),
                        applied.source_tick
                    );
                }
                (applied, segment) => panic!(
                    "case {case} @{position}: export {:?}, preview {:?}",
                    applied.map(|a| a.clip),
                    segment.map(|(_, s)| s.clip)
                ),
            }

            // Sound: the same clip on the audio track.
            let tracks = plan.sound_tracks();
            let (_, segments) = tracks.iter().find(|(id, _)| *id == 2).expect("audio track");
            let heard =
                blinkify_engine::playback::plan::segment_at(segments, t).map(|(_, s)| s.clip);
            assert_eq!(
                heard,
                placed(2).map(|a| Some(a.clip)),
                "case {case} @{position}"
            );
            positions_checked += 1;
        }
    }
    assert!(positions_checked > 1000, "{positions_checked}");
}

/// Every file under `dir`, with its size and modification time.
fn files(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("dir") {
            let entry = entry.expect("entry");
            let metadata = entry.metadata().expect("metadata");
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                found.insert(
                    entry.path(),
                    (metadata.len(), metadata.modified().expect("mtime")),
                );
            }
        }
    }
    found
}

fn two_clips(path: &Path, second: Vec<Operation>, first_gain: Option<f64>) -> Project {
    let tb = Rational { num: 1, den: 1000 };
    let mut first = vec![Operation::Trim { from: 0, to: 2000 }];
    if let Some(db) = first_gain {
        first.push(Operation::Gain { db });
    }
    project_with(
        path,
        vec![Track {
            id: 1,
            kind: TrackKind::Video,
            clips: vec![
                Clip::new(1, 1, 0, tb, 0, first),
                Clip::new(2, 1, 0, tb, 60, second),
            ],
        }],
        Rational { num: 30, den: 1 },
    )
}

#[test]
fn a_graph_change_shows_at_once_and_writes_no_file() {
    let orchestrator = orchestrator();
    let dir = common::scratch("evaluate-live");
    let path = clip_file(&orchestrator, &dir, 8);
    let cache = Cache::new(dir.join("cache"), u64::MAX);
    let source = Arc::new(media(&orchestrator, &path, Some(cache)));
    assert_eq!(source.time_base(), Rational { num: 1, den: 1000 });
    let sources = BTreeMap::from([(1, Arc::clone(&source))]);

    let trim = |from: i64| {
        vec![Operation::Trim {
            from,
            to: from + 2000,
        }]
    };
    let project = two_clips(&path, trim(3000), None);
    let (player, _) = player(&orchestrator, plan_of(&project, &sources));
    // Half a second into the second clip: 3.5 s of the source.
    player.command(TransportCommand::Seek {
        position: 2_500_000,
    });
    let before = landed(&player).picture.as_ref().expect("picture").pts;
    assert!((3466..=3500).contains(&before), "{before}");

    let written = files(&dir);
    // Trim the second clip to start 2 s later: the same place on the
    // timeline now shows 5.5 s of the source.
    player.set_plan(plan_of(&two_clips(&path, trim(5000), None), &sources));
    let after = landed(&player).picture.as_ref().expect("picture").pts;
    assert!((5466..=5500).contains(&after), "{before} → {after}");

    // Reorder, change speed, change gain: none of it writes anything.
    let mut reordered = two_clips(&path, trim(5000), Some(-6.0));
    reordered.sequence.tracks[0].clips[0].start = 60;
    reordered.sequence.tracks[0].clips[1].start = 0;
    player.set_plan(plan_of(&reordered, &sources));
    let _ = landed(&player);
    let mut faster = trim(5000);
    faster.push(Operation::Speed {
        ratio: Rational { num: 2, den: 1 },
    });
    player.set_plan(plan_of(&two_clips(&path, faster, Some(-3.0)), &sources));
    let _ = landed(&player);
    player.command(TransportCommand::Play);
    std::thread::sleep(Duration::from_millis(500));
    player.set_plan(plan_of(&two_clips(&path, trim(4000), None), &sources));
    std::thread::sleep(Duration::from_millis(500));
    player.close();
    assert_eq!(files(&dir), written, "a graph change wrote a file");
}

#[test]
fn an_unchanged_clip_keeps_its_decoder() {
    let orchestrator = orchestrator();
    let dir = common::scratch("evaluate-reuse");
    let path = clip_file(&orchestrator, &dir, 8);
    let source = Arc::new(media(&orchestrator, &path, None));
    let sources = BTreeMap::from([(1, Arc::clone(&source))]);
    let second = |from: i64| {
        vec![Operation::Trim {
            from,
            to: from + 2000,
        }]
    };
    let (player, _) = player(
        &orchestrator,
        plan_of(&two_clips(&path, second(3000), None), &sources),
    );
    player.command(TransportCommand::Seek {
        position: 1_000_000,
    });
    let shown = landed(&player);
    let pids = player.decoder_pids();
    assert!(!pids.is_empty());

    // The clip under the playhead is untouched; the other one changes.
    player.set_plan(plan_of(
        &two_clips(&path, second(5000), Some(-3.0)),
        &sources,
    ));
    let after = player.decoder_pids();
    assert!(
        !after.is_empty() && after.iter().all(|pid| pids.contains(pid)),
        "the decoder was restarted: {pids:?} → {after:?}"
    );
    let still = landed(&player);
    assert_eq!(
        still.picture.as_ref().map(|p| p.pts),
        shown.picture.as_ref().map(|p| p.pts)
    );

    // The clip under the playhead changes: now it is decoded again.
    let mut moved = two_clips(&path, second(5000), None);
    moved.sequence.tracks[0].clips[0] = Clip::new(
        1,
        1,
        0,
        Rational { num: 1, den: 1000 },
        0,
        vec![Operation::Trim {
            from: 1000,
            to: 3000,
        }],
    );
    player.set_plan(plan_of(&moved, &sources));
    let now = landed(&player);
    assert_eq!(now.picture.as_ref().expect("picture").pts, 2000);
    assert!(
        player.decoder_pids().iter().any(|pid| !pids.contains(pid)),
        "a changed clip must be decoded again"
    );
    player.close();
}

/// Wait until the clock passes `t`.
fn play_until(player: &Player, t: i64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while player.position().1 < t {
        assert!(Instant::now() < deadline, "playback stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_clips_speed_is_seen_and_its_gain_is_heard() {
    let orchestrator = orchestrator();
    let dir = common::scratch("evaluate-speed-gain");
    let path = clip_file(&orchestrator, &dir, 8);
    let source = Arc::new(media(&orchestrator, &path, None));
    let sources = BTreeMap::from([(1, Arc::clone(&source))]);
    let tb = Rational { num: 1, den: 1000 };
    let one = |operations: Vec<Operation>| {
        project_with(
            &path,
            vec![Track {
                id: 1,
                kind: TrackKind::Video,
                clips: vec![Clip::new(1, 1, 0, tb, 0, operations)],
            }],
            Rational { num: 30, den: 1 },
        )
    };
    let trim = Operation::Trim { from: 0, to: 6000 };

    // Double speed: a second of timeline shows two of the source.
    let (fast, _) = player(
        &orchestrator,
        plan_of(
            &one(vec![
                trim,
                Operation::Speed {
                    ratio: Rational { num: 2, den: 1 },
                },
            ]),
            &sources,
        ),
    );
    assert_eq!(fast.plan().duration(), 3_000_000);
    fast.command(TransportCommand::Play);
    play_until(&fast, 1_000_000);
    fast.command(TransportCommand::Pause);
    let shown = landed(&fast);
    let pts = shown.picture.as_ref().expect("picture").pts;
    assert!(
        (pts - 2 * shown.position / 1000).abs() <= 70,
        "{pts} at {}",
        shown.position
    );
    fast.close();

    // −6.02 dB: half the amplitude of the same clip without it.
    let peak = |operations: Vec<Operation>| {
        let (player, captured) = player(&orchestrator, plan_of(&one(operations), &sources));
        player.command(TransportCommand::Play);
        play_until(&player, 2_000_000);
        player.close();
        let heard = captured.lock().expect("captured").clone();
        heard.iter().fold(0.0_f32, |max, s| max.max(s.abs()))
    };
    let unity = peak(vec![trim]);
    let halved = peak(vec![trim, Operation::Gain { db: -6.020_6 }]);
    assert!(unity > 0.2, "{unity}");
    assert!(
        (halved - unity / 2.0).abs() < 0.002,
        "{halved} against {unity}"
    );
}
