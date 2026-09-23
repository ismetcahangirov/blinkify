//! The project file (#32), against real files on disk.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::cast_possible_truncation
)]

mod common;

use std::path::{Path, PathBuf};

use blinkify_engine::keyframes::KeyframeIndex;
use blinkify_engine::orchestrator::{Limits, Orchestrator};
use blinkify_engine::probe::{Prober, Rational};
use blinkify_engine::project::{
    Clip, Operation, Project, ProjectError, RelinkError, SCHEMA_VERSION, SequenceSettings,
    SourceStatus, Track, TrackKind,
};
use blinkify_engine::proxy::MediaAsset;

fn fixture(version: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/projects/v{version}.blinkify"))
}

/// A copy of a corpus file in the test's scratch directory, so the test can
/// move it and change it. The corpus itself is never touched.
fn copy(name: &str, dir: &Path) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(common::corpus(name), &path).expect("copy");
    path
}

/// A project with one clip of `path` on a video track.
fn project_of(path: &Path) -> Project {
    let mut project = Project::new("Test", SequenceSettings::default());
    let source = project
        .add_source(&MediaAsset::new(path.to_path_buf()).export_source())
        .expect("source");
    project.sequence.tracks.push(Track {
        id: 1,
        kind: TrackKind::Video,
        clips: vec![Clip {
            id: 7,
            source,
            stream: 0,
            time_base: Rational {
                num: 1,
                den: 90_000,
            },
            start: 0,
            operations: vec![Operation::Trim {
                from: 0,
                to: 90_000,
            }],
        }],
    });
    project
}

#[test]
fn every_schema_version_ever_written_opens_in_this_build() {
    for version in 1..=SCHEMA_VERSION {
        let path = fixture(version);
        assert!(
            path.is_file(),
            "schema {version} has no fixture: commit a file written by that version to {}",
            path.display()
        );
        let project =
            Project::load(&path).unwrap_or_else(|error| panic!("schema {version}: {error}"));
        assert_eq!(project.schema_version, SCHEMA_VERSION);
        assert_eq!(project.clips().count(), 2, "schema {version}");
    }
    // The fixture of the current version is exactly what this build writes.
    let current = std::fs::read_to_string(fixture(SCHEMA_VERSION)).expect("read");
    let project = Project::from_json(&current).expect("load");
    assert_eq!(project.to_json().expect("json"), current);
}

#[test]
fn a_project_saved_and_loaded_saves_byte_identical() {
    let dir = common::scratch("project-round-trip");
    let project = project_of(&copy("vfr-screen.mp4", &dir));
    let first = dir.join("a.blinkify");
    let second = dir.join("b.blinkify");
    project.save(&first).expect("save");
    Project::load(&first)
        .expect("load")
        .save(&second)
        .expect("save again");
    assert_eq!(
        std::fs::read(&first).expect("read"),
        std::fs::read(&second).expect("read")
    );
    // No sibling left behind by the save.
    let names: Vec<_> = std::fs::read_dir(&dir)
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
}

#[test]
fn a_moved_source_marks_its_clips_and_can_be_relinked() {
    let dir = common::scratch("project-moved");
    let original = copy("vfr-screen.mp4", &dir);
    let project_path = dir.join("moved.blinkify");
    project_of(&original).save(&project_path).expect("save");

    let moved = dir.join("elsewhere").join("renamed.mp4");
    std::fs::create_dir_all(moved.parent().expect("parent")).expect("dir");
    std::fs::rename(&original, &moved).expect("move");

    // The project opens: a missing source is not a failure to open.
    let mut project = Project::load(&project_path).expect("opens");
    let status = project.check_sources();
    assert_eq!(status.get(&1), Some(&SourceStatus::Missing));
    assert_eq!(project.clips_of(1), vec![7]);

    // A different file is refused.
    let other = copy("portrait-phone.mp4", &dir);
    assert_eq!(
        project.relink(1, &MediaAsset::new(other).export_source()),
        Err(RelinkError::DifferentContent)
    );
    project
        .relink(1, &MediaAsset::new(moved.clone()).export_source())
        .expect("relink");
    assert_eq!(
        project.check_sources().get(&1),
        Some(&SourceStatus::Present)
    );
    assert_eq!(project.sources.get(&1).expect("source").path(), moved);
}

#[test]
fn a_source_replaced_at_the_same_path_is_a_changed_source() {
    let dir = common::scratch("project-changed");
    let path = copy("vfr-screen.mp4", &dir);
    let project = project_of(&path);

    // Same path, same size, one byte different in the first mebibyte.
    let mut bytes = std::fs::read(&path).expect("read");
    bytes[1000] ^= 0xff;
    std::fs::write(&path, &bytes).expect("write");
    match project.check_sources().get(&1) {
        Some(SourceStatus::Changed { found }) => assert_eq!(found.size, bytes.len() as u64),
        other => panic!("expected a changed source, got {other:?}"),
    }
    assert_eq!(
        project.sources[&1].relink(&MediaAsset::new(path).export_source()),
        Err(RelinkError::DifferentContent)
    );
}

#[test]
fn touching_a_source_without_changing_it_keeps_it_present() {
    let dir = common::scratch("project-touched");
    let path = copy("vfr-screen.mp4", &dir);
    let project = project_of(&path);
    let bytes = std::fs::read(&path).expect("read");
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&path, bytes).expect("rewrite");
    assert_eq!(
        project.check_sources().get(&1),
        Some(&SourceStatus::Present)
    );
}

#[test]
fn a_damaged_or_future_project_is_refused_with_a_reason() {
    let dir = common::scratch("project-damaged");
    let text = std::fs::read_to_string(fixture(SCHEMA_VERSION)).expect("read");

    let truncated = dir.join("truncated.blinkify");
    std::fs::write(&truncated, &text[..text.len().div_euclid(2)]).expect("write");
    assert!(matches!(
        Project::load(&truncated),
        Err(ProjectError::Corrupt(_))
    ));

    let empty = dir.join("empty.blinkify");
    std::fs::write(&empty, "").expect("write");
    assert!(matches!(
        Project::load(&empty),
        Err(ProjectError::Corrupt(_))
    ));

    let wrong_shape = text.replace("\"op\": \"trim\"", "\"op\": \"teleport\"");
    assert!(matches!(
        Project::from_json(&wrong_shape),
        Err(ProjectError::Corrupt(_))
    ));

    let future = text.replacen(
        &format!("\"schemaVersion\": {SCHEMA_VERSION}"),
        &format!("\"schemaVersion\": {}", SCHEMA_VERSION + 1),
        1,
    );
    let error = Project::from_json(&future).expect_err("refused");
    assert_eq!(
        error,
        ProjectError::FutureVersion {
            found: SCHEMA_VERSION + 1,
            supported: SCHEMA_VERSION
        }
    );
    assert!(error.to_string().contains("newer version of Blinkify"));

    assert!(matches!(
        Project::load(&dir.join("not-there.blinkify")),
        Err(ProjectError::Io(_))
    ));
}

#[test]
fn clip_timing_lands_on_the_real_frames_of_a_vfr_source() {
    let path = common::corpus("vfr-screen.mp4");
    let orchestrator = Orchestrator::new(common::sidecar(), Limits::for_this_machine());
    let info = Prober::new(orchestrator.clone())
        .probe(&path)
        .expect("probe");
    let index = KeyframeIndex::open(&path, &info, orchestrator, None).expect("index");
    let stream = index.streams().next().expect("video");
    let time_base = index.time_base(stream).expect("time base");
    assert_eq!(
        time_base,
        Rational {
            num: 1,
            den: 90_000
        }
    );

    // From the keyframe at 2.5 s to the one at 3.2 s: across the file's
    // 30 → 10 fps change, where every frame is 9000 ticks long.
    let clip = Clip {
        id: 1,
        source: 1,
        stream,
        time_base,
        start: 100,
        operations: vec![Operation::Trim {
            from: 225_000,
            to: 288_000,
        }],
    };
    for (rate, frames) in [
        (Rational { num: 30, den: 1 }, 21),
        (
            Rational {
                num: 30_000,
                den: 1001,
            },
            21,
        ),
        (Rational { num: 25, den: 1 }, 18),
        (Rational { num: 60, den: 1 }, 42),
    ] {
        let sequence = SequenceSettings {
            frame_rate: rate,
            ..SequenceSettings::default()
        }
        .time_base();
        assert_eq!(clip.length(sequence), Some(frames), "{rate:?}");

        // Every timeline frame shows a source frame inside the trim, in
        // order, and the first shows exactly the first.
        let mut shown = Vec::new();
        for at in 100..100 + frames {
            let tick = clip.source_at(at, sequence).expect("covered");
            let frame = index
                .frame_at_or_before(stream, tick)
                .expect("table")
                .expect("a frame");
            assert!(
                (225_000..288_000).contains(&frame),
                "{rate:?} {at}: {frame}"
            );
            shown.push(frame);
        }
        assert_eq!(shown.first(), Some(&225_000), "{rate:?}");
        assert!(shown.windows(2).all(|pair| pair[0] <= pair[1]), "{rate:?}");
        assert_eq!(clip.source_at(100 + frames, sequence), None, "{rate:?}");
    }
}
