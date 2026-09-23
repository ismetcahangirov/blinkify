//! The media probe against the generated corpus and against broken files.

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::float_cmp
)]

mod common;

use std::sync::Arc;

use blinkify_engine::orchestrator::{Limits, Orchestrator};
use blinkify_engine::probe::{
    ChromaSubsampling, FrameRateMode, HdrTransfer, MediaInfo, ProbeError, Prober, StreamKind,
    VideoInfo,
};

fn prober() -> Prober {
    Prober::new(Orchestrator::new(
        common::sidecar(),
        Limits::for_this_machine(),
    ))
}

fn probe(name: &str) -> Arc<MediaInfo> {
    prober()
        .probe(&common::corpus(name))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn first_video(info: &MediaInfo) -> &VideoInfo {
    info.video().next().expect("a video stream").1
}

#[test]
fn h264_with_b_frames_reports_its_codec_profile_and_constant_rate() {
    let info = probe("h264-high-closed-gop.mp4");
    let (stream, video) = info.video().next().expect("video");
    assert_eq!(stream.codec.as_deref(), Some("h264"));
    assert_eq!(stream.profile.as_deref(), Some("High"));
    assert_eq!(stream.level, Some(30));
    assert_eq!((video.width, video.height), (640, 360));
    assert_eq!(video.bit_depth, Some(8));
    assert_eq!(video.chroma_subsampling, Some(ChromaSubsampling::Yuv420));
    assert!(video.has_b_frames);
    assert_eq!(video.frame_rate.mode, FrameRateMode::Constant);
    assert_eq!(video.rotation, 0);
    assert!(video.hdr.is_none(), "SDR must not report HDR");

    let (audio_stream, audio) = info.audio().next().expect("audio");
    assert_eq!(audio_stream.codec.as_deref(), Some("aac"));
    assert_eq!(audio.sample_rate, Some(48_000));
    assert_eq!(audio.channels, Some(2));
    assert_eq!(audio.channel_layout.as_deref(), Some("stereo"));
    assert!(info.container.format_name.contains("mp4"));
    assert!(
        info.container
            .duration_seconds
            .is_some_and(|d| (d - 4.0).abs() < 0.2)
    );
}

#[test]
fn a_portrait_phone_video_reports_rotation_and_display_size() {
    let video = first_video(&probe("portrait-phone.mp4")).clone();
    assert_eq!((video.width, video.height), (1280, 720), "coded size");
    assert_eq!(video.rotation, 90);
    assert_eq!(
        (video.display_width, video.display_height),
        (720, 1280),
        "a portrait video must display portrait"
    );
}

#[test]
fn a_vfr_source_is_reported_as_variable_not_as_its_average() {
    let video = first_video(&probe("vfr-screen.mp4")).clone();
    assert_eq!(video.frame_rate.mode, FrameRateMode::Variable);
    let average = video
        .frame_rate
        .average
        .and_then(blinkify_engine::probe::Rational::value)
        .expect("average");
    assert!(
        average < 29.0 && average > 10.0,
        "the average of 30 and 10 fps sections is between them: {average}"
    );
}

#[test]
fn an_edit_list_is_flagged_and_its_absence_is_not() {
    let edited = probe("edit-list.mp4");
    assert!(
        edited.container.has_edit_list,
        "{:?}",
        edited.container.edit_lists
    );
    assert!(
        edited
            .container
            .edit_lists
            .iter()
            .any(|list| list.entries.iter().any(|e| e.media_time > 0)),
        "the pre-roll is skipped by a non-zero media time: {:?}",
        edited.container.edit_lists
    );

    let matroska = probe("multi-audio.mkv");
    assert!(!matroska.container.has_edit_list);
    assert!(matroska.container.edit_lists.is_empty());
}

#[test]
fn hdr10_metadata_is_reported_when_present() {
    let info = probe("hevc-hdr10.mp4");
    let (stream, video) = info.video().next().expect("video");
    assert_eq!(stream.codec.as_deref(), Some("hevc"));
    assert_eq!(stream.profile.as_deref(), Some("Main 10"));
    assert_eq!(video.bit_depth, Some(10));
    assert_eq!(video.color.primaries.as_deref(), Some("bt2020"));
    assert_eq!(video.color.transfer.as_deref(), Some("smpte2084"));
    assert_eq!(video.color.matrix.as_deref(), Some("bt2020nc"));
    assert_eq!(video.color.range.as_deref(), Some("tv"));

    let hdr = video.hdr.as_ref().expect("HDR10 is reported");
    assert_eq!(hdr.transfer, HdrTransfer::Pq);
    let mastering = hdr.mastering_display.expect("mastering display");
    assert!((mastering.max_luminance - 1000.0).abs() < 0.01);
    assert!((mastering.red[0] - 0.68).abs() < 0.001, "{mastering:?}");
    let light = hdr.content_light.expect("content light level");
    assert_eq!((light.max_content, light.max_frame_average), (1000, 400));
}

#[test]
fn every_codec_in_the_corpus_is_identified() {
    for (file, codec, profile) in [
        ("h264-open-gop.mp4", "h264", Some("High")),
        ("hevc-open-gop.mp4", "hevc", Some("Main")),
        ("hevc-closed-gop-radl.mp4", "hevc", Some("Main")),
        ("vp9.webm", "vp9", Some("Profile 0")),
        ("av1.mp4", "av1", Some("Main")),
    ] {
        let info = probe(file);
        let (stream, _) = info.video().next().expect("video");
        assert_eq!(stream.codec.as_deref(), Some(codec), "{file}");
        assert_eq!(stream.profile.as_deref(), profile, "{file}");
    }
}

#[test]
fn multiple_audio_tracks_and_chapters_are_all_reported() {
    let info = probe("multi-audio.mkv");
    let audio: Vec<_> = info.audio().collect();
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0].0.codec.as_deref(), Some("opus"));
    assert_eq!(audio[0].1.channels, Some(2));
    assert_eq!(audio[1].0.codec.as_deref(), Some("aac"));
    assert_eq!(audio[1].1.channels, Some(6));
    assert_eq!(audio[1].1.channel_layout.as_deref(), Some("5.1"));

    let chapters = &info.container.chapters;
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].title.as_deref(), Some("Opening"));
    assert!((chapters[1].start_seconds - 2.0).abs() < 0.01);
}

#[test]
fn the_probe_result_is_typed_for_the_renderer() {
    // The generated contract carries the facts that matter by name.
    let json = serde_json::to_string(&*probe("portrait-phone.mp4")).expect("serialises");
    for key in [
        "\"displayWidth\":720",
        "\"rotation\":90",
        "\"type\":\"video\"",
        "\"hasEditList\"",
    ] {
        assert!(json.contains(key), "{key} missing from {json}");
    }
    assert!(matches!(
        probe("vp9.webm").streams[0].kind,
        StreamKind::Video(_)
    ));
}

#[test]
fn a_zero_byte_file_is_reported_as_empty() {
    let path = common::scratch("probe-empty").join("empty.mp4");
    std::fs::write(&path, b"").expect("write");
    assert!(matches!(prober().probe(&path), Err(ProbeError::Empty(_))));
}

#[test]
fn a_missing_file_is_reported_as_missing() {
    let path = common::scratch("probe-missing").join("gone.mp4");
    assert!(matches!(
        prober().probe(&path),
        Err(ProbeError::NotFound(_))
    ));
}

#[test]
fn a_non_media_file_names_the_reason() {
    let path = common::scratch("probe-text").join("notes.mp4");
    std::fs::write(&path, "this is a shopping list, not a video\n".repeat(100)).expect("write");
    match prober().probe(&path) {
        Err(ProbeError::Unreadable { reason, .. }) => {
            assert!(reason.contains("not a media format"), "{reason}");
        }
        other => panic!("expected Unreadable, got {other:?}"),
    }
}

#[test]
fn a_truncated_file_names_the_reason() {
    // The corpus MP4s keep `moov` at the end, as cameras do. Cut the file in
    // half and the index is gone — the failure a copy interrupted mid-way
    // leaves behind.
    let source = std::fs::read(common::corpus("h264-high-closed-gop.mp4")).expect("read");
    let path = common::scratch("probe-truncated").join("interrupted copy.mp4");
    std::fs::write(&path, &source[..source.len() >> 1]).expect("write");
    match prober().probe(&path) {
        Err(ProbeError::Unreadable { reason, .. }) => assert!(!reason.is_empty()),
        other => panic!("expected Unreadable, got {other:?}"),
    }
}

#[test]
fn the_cache_answers_for_one_version_and_invalidates_on_change() {
    let dir = common::scratch("probe-cache");
    let path = dir.join("clip.mp4");
    std::fs::copy(common::corpus("h264-high-closed-gop.mp4"), &path).expect("copy");

    let prober = prober();
    let first = prober.probe(&path).expect("probe");
    let again = prober.probe(&path).expect("probe");
    assert!(
        Arc::ptr_eq(&first, &again),
        "an unchanged file is answered from cache"
    );

    // Replace the contents: a different file under the same name.
    std::fs::copy(common::corpus("vp9.webm"), &path).expect("replace");
    let changed = prober.probe(&path).expect("probe");
    assert!(!Arc::ptr_eq(&first, &changed));
    assert_eq!(
        changed.video().next().expect("video").0.codec.as_deref(),
        Some("vp9")
    );
}
