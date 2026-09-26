//! The losslessness suite (#45): the evidence for the only thing that
//! distinguishes Blinkify — that what it does not have to change reaches the
//! output unchanged, byte for byte.
//!
//! Every assertion here is on packet payloads, by the hash boundary defined
//! once in `export::verify`, or on a full decode's error stream. "The file
//! exists and plays" is not an assertion anywhere in it.
//!
//! The suite writes the results table in `docs/engineering/losslessness.md`,
//! and fails when the committed table is not what the suite produced: the
//! table is evidence, and evidence is generated, never typed. To regenerate
//! it after a deliberate change, run with `BLINKIFY_UPDATE_LOSSLESSNESS=1`.
//!
//! Only results that are the same on every machine are published. Seams in
//! H.264 and HEVC need a hardware encoder (ADR-0003); they are asserted in
//! `export_smartcut.rs` wherever one exists, and declined where not.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::integer_division
)]

mod common;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blinkify_engine::export::audio::AudioTarget;
use blinkify_engine::export::plan::Media;
use blinkify_engine::export::verify::{
    Divergence, PacketHash, contains, decode_errors, payload_hashes, range_of,
};
use blinkify_engine::orchestrator::{Priority, SidecarCommand};
use blinkify_engine::project::Operation;
use blinkify_engine::tier::ExportTier;
use common::fixture::{Source, speed};

/// Every video file of the corpus, and what it is.
const CORPUS: [(&str, &str); 15] = [
    (
        "h264-high-closed-gop.mp4",
        "H.264 High, closed GOP, B-frames",
    ),
    ("h264-open-gop.mp4", "H.264 High, open GOP"),
    ("h264-high10.mp4", "H.264 High 10, 10-bit"),
    ("hevc-open-gop.mp4", "HEVC Main, CRA and RASL"),
    ("hevc-closed-gop-radl.mp4", "HEVC Main, IDR with RADL"),
    ("hevc-main10.mp4", "HEVC Main 10, SDR"),
    ("hevc-hdr10.mp4", "HEVC Main 10, HDR10"),
    ("portrait-phone.mp4", "H.264, 90° rotation"),
    ("vfr-screen.mp4", "H.264, variable frame rate"),
    ("edit-list.mp4", "H.264, MP4 edit list"),
    ("multi-audio.mkv", "VP9, two audio tracks, chapters"),
    ("vp9.webm", "VP9 profile 0"),
    ("vp9-keyframes.webm", "VP9, keyframe every second"),
    ("av1.mp4", "AV1 Main"),
    ("av1-keyframes.mkv", "AV1, keyframe every second"),
];

/// The results table's rows, as the tests find them.
static ROWS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

fn record(case: &str, file: &str, result: &str) {
    ROWS.lock()
        .expect("rows")
        .push((case.to_owned(), file.to_owned(), result.to_owned()));
}

fn micros(source: &Source, ticks: i64) -> i64 {
    (source.seconds(ticks) * 1_000_000.0).round() as i64
}

fn hashes(path: &Path) -> Vec<PacketHash> {
    payload_hashes(&common::orchestrator(), path, "v:0").expect("packets")
}

fn clean(path: &Path) {
    assert_eq!(
        decode_errors(&common::orchestrator(), path).expect("decodes"),
        Vec::<String>::new(),
        "{}",
        path.display()
    );
}

fn output(name: &str, file: &str) -> PathBuf {
    common::scratch(&format!("lossless-{name}")).join(file)
}

fn container_for(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|e| e.to_str()) {
        Some("webm") => "webm",
        Some("mkv") => "mkv",
        _ => "mp4",
    }
}

/// The random-access keyframes of a source that are shown: where a copy may
/// start and end.
fn cut_points(source: &Source) -> Vec<i64> {
    source
        .video()
        .keyframes
        .iter()
        .filter(|k| k.pts >= 0 && k.random_access && !k.open)
        .map(|k| k.pts)
        .collect()
}

#[test]
fn a_keyframe_aligned_cut_of_every_corpus_file_copies_every_packet() {
    for (name, what) in CORPUS {
        let source = Source::corpus(name);
        let points = cut_points(&source);
        // Between two keyframes where there are several; the whole shown
        // stream where there is one.
        let (from, to) = match points.as_slice() {
            [first, .., last] if points.len() >= 3 => (*first, *last),
            [first, ..] => (*first, source.end()),
            [] => panic!("{name}: no keyframe a copy can start on"),
        };
        let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[])]);
        let video = plan
            .segments
            .iter()
            .find(|s| s.media == Media::Video)
            .expect("video");
        assert_eq!(
            video.tier,
            ExportTier::StreamCopy,
            "{name}: {:?}",
            video.causes
        );
        let target = output(name, &format!("cut.{}", container_for(name)));
        source
            .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let expected = range_of(
            &hashes(&source.path),
            micros(&source, from),
            micros(&source, to),
        )
        .expect("range");
        assert_eq!(contains(&hashes(&target), &expected), Ok(()), "{name}");
        clean(&target);
        record(
            "Keyframe-aligned cut",
            &format!("`{name}` — {what}"),
            &format!("{} packets bit-identical; 0 decode errors", expected.len()),
        );
    }
}

#[test]
fn a_cut_between_keyframes_re_encodes_only_its_windows() {
    // The software encoders exist on every machine, so these results are
    // the same everywhere; H.264 and HEVC are in `export_smartcut.rs`.
    for name in ["vp9-keyframes.webm", "av1-keyframes.mkv"] {
        let source = Source::with_encoders(name);
        let points = cut_points(&source);
        let frame = source.video().time_base.den / 30 / source.video().time_base.num;
        let (from, to) = (points[1] + 7 * frame, points[3] + 5 * frame);
        let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &[])]);
        let video = plan
            .segments
            .iter()
            .find(|s| s.media == Media::Video)
            .expect("video")
            .clone();
        assert!(matches!(video.tier, ExportTier::SmartCut { .. }), "{name}");
        let target = output(&format!("window-{name}"), "cut.mkv");
        source
            .export(&plan, &target, AudioTarget::Opus { kilobits: 128 })
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        // Outside the windows: the source's packets, every one.
        let copy_from = video.windows[0].to;
        let copy_to = video
            .windows
            .last()
            .map_or(to, |w| if w.from > copy_from { w.from } else { to });
        let expected = range_of(
            &hashes(&source.path),
            micros(&source, copy_from),
            micros(&source, copy_to),
        )
        .expect("range");
        assert_eq!(contains(&hashes(&target), &expected), Ok(()), "{name}");
        // Inside them: no more pictures than the windows hold.
        let output_frames = common::packet_times(&target, "v:0").len();
        let window_frames: i64 = video
            .windows
            .iter()
            .map(|w| (w.to - w.from + frame / 2) / frame)
            .sum();
        assert_eq!(
            output_frames,
            expected.len() + usize::try_from(window_frames).expect("small"),
            "{name}"
        );
        clean(&target);
        record(
            "Smart-cut between keyframes",
            &format!("`{name}`"),
            &format!(
                "{} packets outside the windows bit-identical; {} frames re-encoded; 0 decode errors",
                expected.len(),
                window_frames
            ),
        );
    }
}

#[test]
fn ten_trim_and_save_cycles_lose_nothing() {
    // Each generation is the last one's export, trimmed to its own start and
    // end — the edit a user makes again and again.
    let original = Source::corpus("h264-high-closed-gop.mp4");
    let points = cut_points(&original);
    let (from, to) = (points[0], *points.last().expect("keyframes"));
    let expected = range_of(
        &hashes(&original.path),
        micros(&original, from),
        micros(&original, to),
    )
    .expect("range");
    let mut current = original.path.clone();
    let dir = common::scratch("lossless-generations");
    for generation in 1..=10 {
        let source = Source::at(current.clone(), None);
        let points = cut_points(&source);
        let plan = source.plan_clips(vec![source.clip(
            1,
            0,
            points[0],
            if generation == 1 { to } else { source.end() },
            &[],
        )]);
        assert!(plan.summary.lossless, "generation {generation}");
        let next = dir.join(format!("generation-{generation}.mp4"));
        source
            .export(&plan, &next, AudioTarget::default())
            .unwrap_or_else(|error| panic!("generation {generation}: {error}"));
        let got: Vec<[u8; 32]> = hashes(&next).iter().map(|p| p.sha256).collect();
        assert_eq!(got, expected, "generation {generation}");
        current = next;
    }
    clean(&current);
    record(
        "Ten trim-and-save cycles",
        "`h264-high-closed-gop.mp4`",
        &format!(
            "generation 10's {} video packets bit-identical to the source's",
            expected.len()
        ),
    );
}

#[test]
fn changing_the_sound_or_the_speed_leaves_every_picture_packet() {
    let source = Source::corpus("h264-high-closed-gop.mp4");
    let points = cut_points(&source);
    let (from, to) = (points[1], *points.last().expect("keyframes"));
    let expected = range_of(
        &hashes(&source.path),
        micros(&source, from),
        micros(&source, to),
    )
    .expect("range");
    let cases: [(&str, Vec<Operation>); 3] = [
        ("Volume change (+6 dB)", vec![Operation::gain(6.0)]),
        ("Constant speed 2×", vec![speed(2, 1)]),
        ("Constant speed 0.5×", vec![speed(1, 2)]),
    ];
    for (index, (case, operations)) in cases.into_iter().enumerate() {
        let plan = source.plan_clips(vec![source.clip(1, 0, from, to, &operations)]);
        let target = output(&format!("ops-{index}"), "out.mp4");
        source
            .export(&plan, &target, AudioTarget::default())
            .unwrap_or_else(|error| panic!("{case}: {error}"));
        assert_eq!(contains(&hashes(&target), &expected), Ok(()), "{case}");
        clean(&target);
        record(
            case,
            "`h264-high-closed-gop.mp4`",
            &format!(
                "{} video packets bit-identical; sound re-encoded; 0 decode errors",
                expected.len()
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// The suite tests itself: an assertion that cannot fail proves nothing.
// ---------------------------------------------------------------------------

/// Copy a file with FFmpeg under different timestamps: the positive control.
fn rebased(source: &Path, target: &Path) {
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(source)
                .option("-map", "0:v:0")
                .option("-c", "copy")
                .option("-output_ts_offset", "7.5")
                .output_file(target),
            Priority::Foreground,
        )
        .expect("copies");
}

#[test]
fn a_pure_timestamp_rebase_passes_the_boundary() {
    let source = common::corpus("h264-high-closed-gop.mp4");
    let target = output("positive-control", "rebased.mp4");
    rebased(&source, &target);
    let (original, moved) = (hashes(&source), hashes(&target));
    assert_ne!(
        original.iter().map(|p| p.micros).collect::<Vec<_>>(),
        moved.iter().map(|p| p.micros).collect::<Vec<_>>(),
        "the control must actually move the timestamps"
    );
    let all: Vec<[u8; 32]> = original.iter().map(|p| p.sha256).collect();
    assert_eq!(contains(&moved, &all), Ok(()));
}

#[test]
fn a_full_re_encode_fails_the_bit_identity_assertion() {
    let source = common::corpus("vp9.webm");
    let target = output("negative-control", "re-encoded.webm");
    common::orchestrator()
        .run_to_end(
            SidecarCommand::ffmpeg()
                .option("-v", "error")
                .input(&source)
                .option("-map", "0:v:0")
                .option("-c:v", "libvpx-vp9")
                .option("-deadline", "realtime")
                .output_file(&target),
            Priority::Foreground,
        )
        .expect("re-encodes");
    let all: Vec<[u8; 32]> = hashes(&source).iter().map(|p| p.sha256).collect();
    assert!(contains(&hashes(&target), &all).is_err());
}

#[test]
fn a_corrupted_output_fails_the_harness() {
    let source = common::corpus("h264-high-closed-gop.mp4");
    let target = output("corrupted", "copy.mp4");
    rebased(&source, &target);
    // One bit flipped in the middle of the tenth packet's payload.
    let listing = common::orchestrator()
        .run_to_end(
            SidecarCommand::ffprobe()
                .option("-v", "error")
                .option("-select_streams", "v:0")
                .option("-show_entries", "packet=pos,size")
                .option("-of", "csv=p=0")
                .input(&target),
            Priority::Foreground,
        )
        .expect("ffprobe");
    let line = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .nth(10)
        .expect("a packet")
        .to_owned();
    let (size, pos) = line.split_once(',').expect("size,pos");
    let at =
        pos.trim().parse::<usize>().expect("pos") + size.trim().parse::<usize>().expect("size") / 2;
    let mut bytes = std::fs::read(&target).expect("read");
    bytes[at] ^= 0x10;
    std::fs::write(&target, bytes).expect("write");
    let all: Vec<[u8; 32]> = hashes(&source).iter().map(|p| p.sha256).collect();
    assert!(matches!(
        contains(&hashes(&target), &all),
        Err(Divergence::Differs { .. })
    ));
}

// ---------------------------------------------------------------------------
// The published table.
// ---------------------------------------------------------------------------

const TABLE_START: &str =
    "<!-- results: generated by crates/blinkify-engine/tests/losslessness.rs -->";
const TABLE_END: &str = "<!-- end of results -->";

#[test]
fn z_the_published_results_are_the_suites() {
    // Named to sort last in its binary's listing; it waits for the rows
    // the other tests record.
    for _ in 0..600 {
        if ROWS.lock().expect("rows").len() >= CORPUS.len() + 2 + 1 + 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let mut rows = ROWS.lock().expect("rows").clone();
    rows.sort();
    // Laid out as Prettier lays out a Markdown table, so formatting the docs
    // never changes the evidence.
    let header = ("Case".to_owned(), "File".to_owned(), "Result".to_owned());
    let all: Vec<&(String, String, String)> = std::iter::once(&header).chain(&rows).collect();
    let widths = [
        all.iter().map(|r| r.0.chars().count()).max().unwrap_or(0),
        all.iter().map(|r| r.1.chars().count()).max().unwrap_or(0),
        all.iter().map(|r| r.2.chars().count()).max().unwrap_or(0),
    ];
    let line = |cells: [&str; 3]| {
        let padded: Vec<String> = cells
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell}{}", " ".repeat(width - cell.chars().count())))
            .collect();
        format!("| {} |", padded.join(" | "))
    };
    let dashes = widths.map(|width| "-".repeat(width));
    let mut table = String::new();
    let _ = writeln!(table, "{TABLE_START}\n");
    let _ = writeln!(table, "{}", line(["Case", "File", "Result"]));
    let _ = writeln!(table, "{}", line([&dashes[0], &dashes[1], &dashes[2]]));
    for (case, file, result) in &rows {
        let _ = writeln!(table, "{}", line([case, file, result]));
    }
    let _ = write!(table, "\n{TABLE_END}");
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/engineering/losslessness.md");
    let document = std::fs::read_to_string(&path).expect("docs/engineering/losslessness.md");
    let start = document
        .find(TABLE_START)
        .expect("the table's start marker");
    let end = document.find(TABLE_END).expect("the table's end marker") + TABLE_END.len();
    let current = &document[start..end];
    if std::env::var_os("BLINKIFY_UPDATE_LOSSLESSNESS").is_some() {
        let updated = format!("{}{table}{}", &document[..start], &document[end..]);
        std::fs::write(&path, updated).expect("write");
        return;
    }
    assert_eq!(
        current, table,
        "docs/engineering/losslessness.md is not what the suite produced; run it with BLINKIFY_UPDATE_LOSSLESSNESS=1"
    );
}
