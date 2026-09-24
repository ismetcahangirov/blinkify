//! Importing into the media library (#53), against real files: what the
//! shell's import does, step by step — probe, summarise, fingerprint, add —
//! and what it must never do: write a copy of the media anywhere.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::integer_division
)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use blinkify_engine::orchestrator::{Limits, Orchestrator};
use blinkify_engine::probe::Prober;
use blinkify_engine::project::asset::AssetInfo;
use blinkify_engine::project::edit::{Document, Edit, EditContext};
use blinkify_engine::project::{Project, SourceRef, Track, TrackKind};
use blinkify_engine::proxy::MediaAsset;

fn prober() -> Prober {
    Prober::new(Orchestrator::new(
        common::sidecar(),
        Limits::for_this_machine(),
    ))
}

/// What the import does with one file: its summary and reference, or why it
/// is refused.
fn import(prober: &Prober, path: &Path) -> Result<(AssetInfo, SourceRef), String> {
    let info = prober.probe(path).map_err(|error| error.to_string())?;
    let asset = AssetInfo::of(&info)
        .ok_or_else(|| "it has no pictures or sound Blinkify can use".to_owned())?;
    let reference = SourceRef::of(&MediaAsset::new(path.to_path_buf()).export_source())
        .map_err(|error| error.to_string())?;
    Ok((asset, reference))
}

fn files(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("dir")
            .map(|e| e.expect("entry"))
        {
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

#[test]
fn an_import_references_the_file_and_writes_no_copy() {
    let dir = common::scratch("library-import");
    // A folder and a name no ASCII-only code would survive.
    let folder = dir.join("Çəkilişlər — 2026");
    std::fs::create_dir_all(&folder).expect("dir");
    let path = folder.join("Bakı sahili.mp4");
    std::fs::copy(common::corpus("portrait-phone.mp4"), &path).expect("copy");
    let before = files(&dir);

    let (asset, reference) = import(&prober(), &path).expect("imports");
    let video = asset.video.as_ref().expect("pictures");
    assert!(
        video.display_height > video.display_width,
        "portrait, as shot"
    );
    assert!(asset.duration_seconds.is_some_and(|d| d > 0.0));
    assert!(asset.matching.is_some(), "a sequence can match it");
    assert_eq!(reference.path(), path.as_path());

    let mut project = Project::matching_first_clip("Library");
    project.sequence.tracks = vec![
        Track::new(1, TrackKind::Video, Vec::new()),
        Track::new(2, TrackKind::Audio, Vec::new()),
    ];
    let mut document = Document::new(project).expect("valid");
    let ids = document.add_sources(&[reference], &EditContext::default());
    document.describe_asset(ids[0], Some(asset.clone()));
    assert_eq!(document.assets().len(), 1);

    // Dragged onto the video track: one clip, one history entry.
    let time_base = prober().probe(&path).expect("probe").streams
        [usize::try_from(video.stream).expect("index")]
    .time_base
    .expect("time base");
    document
        .apply(
            &Edit::AddClip {
                track: 1,
                source: ids[0],
                stream: video.stream,
                time_base,
                start: 0,
                from: 0,
                to: time_base.den / time_base.num,
            },
            &EditContext::default(),
        )
        .expect("drop");
    assert_eq!(document.history().entries, vec!["Import file", "Add clip"]);
    assert_eq!(files(&dir), before, "importing or placing wrote a file");
}

#[test]
fn a_file_blinkify_cannot_use_is_refused_with_a_reason() {
    let dir = common::scratch("library-refuse");
    let prober = prober();

    let empty = dir.join("empty.mp4");
    std::fs::write(&empty, b"").expect("write");
    assert!(import(&prober, &empty).is_err(), "a zero-byte file");

    let text = dir.join("notes.mp4");
    std::fs::write(&text, b"not a video at all").expect("write");
    assert!(import(&prober, &text).is_err(), "a text file named .mp4");

    // Removed between the drop and the probe.
    let gone = dir.join("gone.mp4");
    std::fs::copy(common::corpus("vfr-screen.mp4"), &gone).expect("copy");
    std::fs::remove_file(&gone).expect("remove");
    assert!(
        import(&prober, &gone).is_err(),
        "a file that is no longer there"
    );
}

#[test]
fn removing_a_source_takes_its_clips_and_undo_brings_all_back() {
    let prober = prober();
    let path = common::corpus("vfr-screen.mp4");
    let (asset, reference) = import(&prober, &path).expect("imports");
    let video = asset.video.expect("pictures");
    let time_base = prober.probe(&path).expect("probe").streams
        [usize::try_from(video.stream).expect("index")]
    .time_base
    .expect("time base");
    let mut project = Project::matching_first_clip("Remove");
    project.sequence.tracks = vec![Track::new(1, TrackKind::Video, Vec::new())];
    let mut document = Document::new(project).expect("valid");
    let ids = document.add_sources(&[reference], &EditContext::default());
    for start in [0, 100] {
        document
            .apply(
                &Edit::AddClip {
                    track: 1,
                    source: ids[0],
                    stream: video.stream,
                    time_base,
                    start,
                    from: 0,
                    to: time_base.den / time_base.num,
                },
                &EditContext::default(),
            )
            .expect("add");
    }
    let before = document.project().to_json().expect("json");
    document
        .apply(
            &Edit::RemoveSource { source: ids[0] },
            &EditContext::default(),
        )
        .expect("remove");
    assert!(document.project().sources.is_empty());
    assert_eq!(document.project().clips().count(), 0);
    document.undo();
    assert_eq!(document.project().to_json().expect("json"), before);
}
