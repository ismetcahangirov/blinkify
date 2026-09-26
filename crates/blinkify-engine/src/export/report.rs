//! The export report (#52): what an export actually did, segment by segment,
//! measured on the file it wrote.
//!
//! This is the artefact that proves the product did what it promised, and
//! what a user sends back when they think it did not. So it is not the plan
//! read back: every packet of the output is hashed by the one hash boundary
//! of #45 ([`verify`](super::verify)) and looked for among its source's
//! packets. A segment the plan called a copy whose packets are not the
//! source's is reported as re-encoded, and as not what was planned.
//!
//! - **Reasons are the dialog's.** [`explain`] phrases each cause for the
//!   export dialog (#50) and here, so the two cannot disagree about the same
//!   export.
//! - **Each re-encode says what could have been done instead**, where there
//!   is something: snap a cut to its keyframe, choose lossless sound, match
//!   the sequence to the clip.
//! - **The text form has no paths unless asked.** A user pasting it into an
//!   issue should not publish their folders by accident.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::audio::AudioEncoding;
use super::execute::{ExportInput, ExportOutcome};
use super::overview::{explain, explain_decline};
use super::plan::{Cause, ExportPlan, Media, Segment};
use super::verify::{PacketHash, VerifyError, payload_hashes, payload_hashes_between};
use crate::orchestrator::Orchestrator;
use crate::probe::{MediaInfo, Prober, Rational};
use crate::project::SourceId;
use crate::tier::ExportTier;

/// A file the export read or wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileFacts {
    /// The file's name, without its folder.
    pub name: String,
    #[ts(type = "string")]
    pub path: PathBuf,
    pub container: String,
    /// Each stream's codec, in file order.
    pub codecs: Vec<String>,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub duration_seconds: Option<f64>,
}

impl FileFacts {
    /// `path`, as its probe describes it.
    #[must_use]
    pub fn of(path: &Path, info: &MediaInfo) -> Self {
        Self {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_path_buf(),
            container: info
                .container
                .format_long_name
                .clone()
                .unwrap_or_else(|| info.container.format_name.clone()),
            codecs: info
                .streams
                .iter()
                .filter_map(|stream| stream.codec.clone())
                .collect(),
            size_bytes: info.container.size_bytes,
            duration_seconds: info.container.duration_seconds,
        }
    }
}

/// What happened to a segment's packets, as measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum Execution {
    /// Every packet is its source's.
    Copied,
    /// Some are, some were encoded: a smart-cut.
    PartlyReEncoded,
    /// None is.
    ReEncoded,
    /// The output has no packet here.
    Empty,
}

/// One segment of the output, as executed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReportSegment {
    pub media: Media,
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// What the plan decided.
    pub planned: ExportTier,
    /// What the output shows.
    pub execution: Execution,
    /// The execution is what the plan decided.
    pub as_planned: bool,
    pub packets: u32,
    /// Packets bit-identical to the source's, by the hash boundary of #45.
    pub identical: u32,
    pub reasons: Vec<String>,
    /// What could have been done so it was copied, where anything could.
    pub suggestions: Vec<String>,
    /// For encoded packets: what made them, and how it was asked.
    pub encoder: Option<String>,
}

/// One stream's totals, as executed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StreamTotals {
    pub copied_seconds: f64,
    pub re_encoded_seconds: f64,
    pub packets: u32,
    pub identical: u32,
}

/// What an export did, measured on what it wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportReport {
    /// Milliseconds since the Unix epoch.
    #[ts(type = "number")]
    pub created: u64,
    pub name: String,
    pub sources: Vec<FileFacts>,
    pub output: FileFacts,
    pub video: Option<StreamTotals>,
    pub audio: Option<StreamTotals>,
    /// Of every packet of the output, the share bit-identical to its
    /// source's, from 0 to 100.
    pub identical_percent: f64,
    /// Every packet of the output is its source's.
    pub lossless: bool,
    /// The export in one sentence.
    pub summary: String,
    pub segments: Vec<ReportSegment>,
    /// Every suggestion, once.
    pub suggestions: Vec<String>,
    /// How the re-encoded sound was made, where any was.
    pub audio_encoding: Option<AudioEncoding>,
    /// Every sidecar command the export ran. They name files by path, so
    /// the text form carries them only with paths.
    pub commands: Vec<String>,
}

/// Everything a report is made from.
#[derive(Debug, Clone, Copy)]
pub struct ReportRequest<'a> {
    pub name: &'a str,
    pub plan: &'a ExportPlan,
    pub outcome: &'a ExportOutcome,
    pub inputs: &'a BTreeMap<SourceId, ExportInput>,
    /// The sound target was lossless: no suggestion to choose one.
    pub lossless_audio_target: bool,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
}

/// Measure what `request.outcome` wrote against its sources, and report it.
///
/// # Errors
///
/// A file could not be read.
pub fn build(
    orchestrator: &Orchestrator,
    request: &ReportRequest<'_>,
) -> Result<ExportReport, VerifyError> {
    let plan = request.plan;
    let output_info = Prober::new(orchestrator.clone())
        .probe(&request.outcome.path)
        .map_err(|error| VerifyError::Probe(error.to_string()))?;
    let mut segments = Vec::new();
    let mut totals: BTreeMap<u8, StreamTotals> = BTreeMap::new();
    for (media, selector) in [(Media::Video, "v:0"), (Media::Audio, "a:0")] {
        let planned: Vec<&Segment> = plan.segments.iter().filter(|s| s.media == media).collect();
        if planned.is_empty() {
            continue;
        }
        let output = payload_hashes(orchestrator, &request.outcome.path, selector)?;
        let mut total = StreamTotals {
            copied_seconds: 0.0,
            re_encoded_seconds: 0.0,
            packets: 0,
            identical: 0,
        };
        for segment in planned {
            let known = source_hashes(orchestrator, request.inputs, segment)?;
            let reported = measure(segment, plan.time_base, &output, &known, request);
            let length = reported.end_seconds - reported.start_seconds;
            let copied = if reported.packets == 0 {
                0.0
            } else {
                length * f64::from(reported.identical) / f64::from(reported.packets)
            };
            total.copied_seconds += copied;
            total.re_encoded_seconds += length - copied;
            total.packets += reported.packets;
            total.identical += reported.identical;
            segments.push(reported);
        }
        totals.insert(u8::from(media == Media::Audio), total);
    }
    let packets: u32 = totals.values().map(|t| t.packets).sum();
    let identical: u32 = totals.values().map(|t| t.identical).sum();
    let identical_percent = if packets == 0 {
        0.0
    } else {
        f64::from(identical) * 100.0 / f64::from(packets)
    };
    let lossless = packets > 0 && identical == packets;
    let mut suggestions: Vec<String> = Vec::new();
    for suggestion in segments.iter().flat_map(|s| &s.suggestions) {
        if !suggestions.contains(suggestion) {
            suggestions.push(suggestion.clone());
        }
    }
    let video = totals.get(&0).copied();
    let audio = totals.get(&1).copied();
    Ok(ExportReport {
        created: request.created,
        name: request.name.to_owned(),
        sources: request
            .inputs
            .values()
            .map(|input| FileFacts::of(input.source.path(), &input.info))
            .collect(),
        output: FileFacts::of(&request.outcome.path, &output_info),
        video,
        audio,
        identical_percent,
        lossless,
        summary: summary(lossless, identical_percent, video, audio),
        segments,
        suggestions,
        audio_encoding: request.outcome.audio.clone(),
        commands: request.outcome.commands.clone(),
    })
}

/// The hashes of every packet the segment's sources hold around its range.
fn source_hashes(
    orchestrator: &Orchestrator,
    inputs: &BTreeMap<SourceId, ExportInput>,
    segment: &Segment,
) -> Result<HashSet<[u8; 32]>, VerifyError> {
    let mut known = HashSet::new();
    for source in &segment.sources {
        let Some(input) = inputs.get(&source.source) else {
            continue;
        };
        let range = (
            crate::time::seconds(source.source_in, source.time_base),
            crate::time::seconds(source.source_out, source.time_base),
        );
        let start = input
            .info
            .streams
            .iter()
            .find(|s| s.index == source.stream)
            .and_then(|s| s.start_seconds)
            .unwrap_or(0.0);
        let hashes = payload_hashes_between(
            orchestrator,
            input.source.path(),
            &source.stream.to_string(),
            Some((range.0 - start, range.1 - start)),
        )?;
        known.extend(hashes.into_iter().map(|p| p.sha256));
    }
    Ok(known)
}

fn seconds(ticks: i64, time_base: Rational) -> f64 {
    crate::time::seconds(ticks, time_base)
}

/// One segment, measured: the output's packets shown in its range, and how
/// many of them are its sources'.
fn measure(
    segment: &Segment,
    time_base: Rational,
    output: &[PacketHash],
    known: &HashSet<[u8; 32]>,
    request: &ReportRequest<'_>,
) -> ReportSegment {
    let start_seconds = seconds(segment.start, time_base);
    let end_seconds = seconds(segment.end(), time_base);
    #[allow(clippy::cast_possible_truncation)]
    let (from, to) = (
        (start_seconds * 1e6).round() as i64,
        (end_seconds * 1e6).round() as i64,
    );
    let inside: Vec<&PacketHash> = output
        .iter()
        .filter(|p| p.micros >= from - 1 && p.micros < to - 1)
        .collect();
    let packets = u32::try_from(inside.len()).unwrap_or(u32::MAX);
    let identical = u32::try_from(inside.iter().filter(|p| known.contains(&p.sha256)).count())
        .unwrap_or(u32::MAX);
    let execution = match (packets, identical) {
        (0, _) => Execution::Empty,
        (all, same) if all == same => Execution::Copied,
        (_, 0) => Execution::ReEncoded,
        _ => Execution::PartlyReEncoded,
    };
    // A seam can be narrower than its window — every picture of it
    // independent of the other side — and then nothing needed encoding.
    let as_planned = matches!(
        (segment.tier, execution),
        (ExportTier::StreamCopy, Execution::Copied)
            | (
                ExportTier::SmartCut { .. },
                Execution::PartlyReEncoded | Execution::Copied
            )
            | (ExportTier::FullReEncode { .. }, Execution::ReEncoded)
            | (_, Execution::Empty)
    );
    let mut reasons: Vec<String> = segment
        .causes
        .iter()
        .map(|cause| explain(cause, segment.sources.first()))
        .collect();
    if let Some(decline) = &segment.decline {
        reasons.push(explain_decline(decline, segment.alternative.is_some()));
    }
    if !as_planned {
        reasons.push(format!(
            "The plan decided {}, but the output shows {}: this is what was executed.",
            tier_words(segment.tier),
            execution_words(execution)
        ));
    }
    let mut suggestions: Vec<String> = Vec::new();
    for cause in &segment.causes {
        if let Some(suggestion) = suggest(cause, request.lossless_audio_target)
            && !suggestions.contains(&suggestion)
        {
            suggestions.push(suggestion);
        }
    }
    let encoder = match (execution, segment.media) {
        (Execution::Copied | Execution::Empty, _) => None,
        (_, Media::Video) => segment.encoder.as_ref().map(|choice| {
            format!(
                "{} ({} profile, {}{})",
                choice.encoder,
                choice.profile,
                choice.pixel_format,
                choice
                    .level
                    .map_or_else(String::new, |level| format!(", level {level}"))
            )
        }),
        (_, Media::Audio) => request.outcome.audio.as_ref().map(|encoding| {
            let rate = encoding
                .kilobits
                .map_or_else(|| "lossless".to_owned(), |k| format!("{k} kb/s"));
            format!(
                "{} ({rate}, {} Hz, {} channels)",
                encoding.encoder, encoding.sample_rate, encoding.channels
            )
        }),
    };
    ReportSegment {
        media: segment.media,
        start_seconds,
        end_seconds,
        planned: segment.tier,
        execution,
        as_planned,
        packets,
        identical,
        reasons,
        suggestions,
        encoder,
    }
}

/// What the user could have done so `cause` did not re-encode, where there
/// is anything.
#[must_use]
pub fn suggest(cause: &Cause, lossless_audio_target: bool) -> Option<String> {
    let lossless_sound = (!lossless_audio_target).then(|| {
        "Choose FLAC or PCM as the sound to re-encode to, and the adjusted sound is \
         re-encoded without loss."
            .to_owned()
    });
    match cause {
        Cause::InPointNotKeyframe { .. }
        | Cause::OutPointNotKeyframe { .. }
        | Cause::OpenGopAtOutPoint { .. } => Some(
            "Snap the cut to the nearest keyframe (Export, then Snap to keyframes) and it is \
             copied whole."
                .to_owned(),
        ),
        Cause::KeyframesUnknown => Some(
            "Wait for the source's keyframes to finish indexing before exporting: the cut may \
             then be proved a copy."
                .to_owned(),
        ),
        Cause::SpeedOutsideContainer { .. } => Some(
            "Choose a speed at which the clip plays between 1 and 240 frames a second, and its \
             packets are copied and retimed."
                .to_owned(),
        ),
        Cause::SequenceMismatch { .. } => Some(
            "Set the sequence to the clip's resolution, frame rate and pixel shape in Sequence \
             settings, and it is copied."
                .to_owned(),
        ),
        Cause::IncompatibleEncoding { .. } => Some(
            "Export this clip on its own: clips recorded with different settings cannot share \
             one copied stream."
                .to_owned(),
        ),
        Cause::Gap => Some(
            "Close the gap (ripple delete it) so there is no black or silence to encode."
                .to_owned(),
        ),
        Cause::AudioChain | Cause::AudioSpeed { .. } | Cause::AudioMix { .. } => lossless_sound,
        // A held or reversed picture has no source packets to copy.
        Cause::Operation { .. } => None,
    }
}

fn tier_words(tier: ExportTier) -> &'static str {
    match tier {
        ExportTier::StreamCopy => "a copy",
        ExportTier::SmartCut { .. } => "a smart-cut",
        ExportTier::FullReEncode { .. } => "a re-encode",
    }
}

fn execution_words(execution: Execution) -> &'static str {
    match execution {
        Execution::Copied => "every packet copied",
        Execution::PartlyReEncoded => "some packets re-encoded",
        Execution::ReEncoded => "every packet re-encoded",
        Execution::Empty => "no packets",
    }
}

fn summary(
    lossless: bool,
    identical_percent: f64,
    video: Option<StreamTotals>,
    audio: Option<StreamTotals>,
) -> String {
    if lossless {
        return "Fully lossless: every packet of the output is bit-identical to its source's."
            .to_owned();
    }
    let part = |name: &str, totals: Option<StreamTotals>| {
        totals
            .filter(|t| t.re_encoded_seconds > 0.0)
            .map(|t| format!("{:.2} s of the {name} re-encoded", t.re_encoded_seconds))
    };
    let parts: Vec<String> = [part("pictures", video), part("sound", audio)]
        .into_iter()
        .flatten()
        .collect();
    format!(
        "Not fully lossless: {identical_percent:.1} % of the output's packets are \
         bit-identical to their source's; {}.",
        if parts.is_empty() {
            "the rest differ".to_owned()
        } else {
            parts.join(", ")
        }
    )
}

impl ExportReport {
    /// The report as plain text, for pasting into an issue. Folders are left
    /// out — only file names — unless `with_paths`; the commands, which name
    /// files by path, come only with them.
    #[must_use]
    pub fn to_text(&self, with_paths: bool) -> String {
        let file = |facts: &FileFacts| {
            let name = if with_paths {
                facts.path.display().to_string()
            } else {
                facts.name.clone()
            };
            format!(
                "{name} — {}, {}, {}, {}",
                facts.container,
                if facts.codecs.is_empty() {
                    "no streams".to_owned()
                } else {
                    facts.codecs.join(" + ")
                },
                bytes(facts.size_bytes),
                facts
                    .duration_seconds
                    .map_or_else(|| "unknown length".to_owned(), clock)
            )
        };
        let mut text = String::new();
        let _ = writeln!(text, "Blinkify export report: {}", self.name);
        let _ = writeln!(text, "Exported {}", utc(self.created));
        let _ = writeln!(text);
        let _ = writeln!(text, "{}", self.summary);
        let _ = writeln!(text);
        let _ = writeln!(text, "Output: {}", file(&self.output));
        for source in &self.sources {
            let _ = writeln!(text, "Source: {}", file(source));
        }
        let _ = writeln!(text);
        for (name, totals) in [("Pictures", self.video), ("Sound", self.audio)] {
            if let Some(t) = totals {
                let _ = writeln!(
                    text,
                    "{name}: {:.2} s copied, {:.2} s re-encoded; {} of {} packets bit-identical",
                    t.copied_seconds, t.re_encoded_seconds, t.identical, t.packets
                );
            }
        }
        let _ = writeln!(
            text,
            "Bit-identical: {:.1} % of the output's packets",
            self.identical_percent
        );
        let _ = writeln!(text);
        let _ = writeln!(text, "Segments");
        for segment in &self.segments {
            let _ = writeln!(
                text,
                "  {}–{}  {}  {}{}  ({} of {} packets identical)",
                clock(segment.start_seconds),
                clock(segment.end_seconds),
                match segment.media {
                    Media::Video => "pictures",
                    Media::Audio => "sound",
                },
                execution_words(segment.execution),
                if segment.as_planned {
                    String::new()
                } else {
                    format!(" — planned as {}", tier_words(segment.planned))
                },
                segment.identical,
                segment.packets
            );
            if let Some(encoder) = &segment.encoder {
                let _ = writeln!(text, "      Encoded with {encoder}");
            }
            for reason in &segment.reasons {
                let _ = writeln!(text, "      Why: {reason}");
            }
            for suggestion in &segment.suggestions {
                let _ = writeln!(text, "      Instead: {suggestion}");
            }
        }
        if let Some(encoding) = &self.audio_encoding {
            let _ = writeln!(text);
            let _ = writeln!(
                text,
                "Re-encoded sound: {} with {}{}",
                encoding.codec,
                encoding.encoder,
                if encoding.matches_copied {
                    ", to match the copied sound"
                } else {
                    ""
                }
            );
        }
        if with_paths && !self.commands.is_empty() {
            let _ = writeln!(text);
            let _ = writeln!(text, "Commands");
            for command in &self.commands {
                let _ = writeln!(text, "  {command}");
            }
        }
        text
    }
}

/// `1:05.400`.
fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let millis = (seconds * 1000.0).round() as u64;
    let (minutes, rest) = (millis.div_euclid(60_000), millis % 60_000);
    let hours = minutes.div_euclid(60);
    if hours > 0 {
        format!("{hours}:{:02}:{:06.3}", minutes % 60, rest_seconds(rest))
    } else {
        format!("{minutes}:{:06.3}", rest_seconds(rest))
    }
}

fn rest_seconds(millis: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let seconds = millis as f64 / 1000.0;
    seconds
}

fn bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let value = bytes as f64;
    match value {
        v if v >= 1e9 => format!("{:.2} GB", v / 1e9),
        v if v >= 1e6 => format!("{:.1} MB", v / 1e6),
        v if v >= 1e3 => format!("{:.0} KB", v / 1e3),
        v => format!("{v:.0} bytes"),
    }
}

/// `2026-09-26 21:40 UTC` for milliseconds since the Unix epoch, with no
/// calendar dependency: days to a civil date (Howard Hinnant's algorithm).
fn utc(millis: u64) -> String {
    let seconds = millis.div_euclid(1000);
    let (days, of_day) = (seconds.div_euclid(86_400), seconds % 86_400);
    let days = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era.div_euclid(1460) + day_of_era.div_euclid(36_524)
        - day_of_era.div_euclid(146_096))
    .div_euclid(365);
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era.div_euclid(4) - year_of_era.div_euclid(100));
    let month_index = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_index + 2).div_euclid(5) + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        of_day.div_euclid(3600),
        (of_day % 3600).div_euclid(60)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_reads_as_a_utc_date() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        // 2026-09-26 21:40 UTC.
        assert_eq!(utc(1_790_458_800_000), "2026-09-26 21:40 UTC");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn times_and_sizes_read_plainly() {
        assert_eq!(clock(65.4), "1:05.400");
        assert_eq!(clock(3725.0), "1:02:05.000");
        assert_eq!(bytes(12_300_000), "12.3 MB");
        assert_eq!(bytes(512), "512 bytes");
    }

    #[test]
    fn every_cut_cause_suggests_the_snap_and_a_hold_suggests_nothing() {
        let cut = Cause::InPointNotKeyframe {
            keyframe_before: Some(0),
            keyframe_after: Some(30),
        };
        assert!(suggest(&cut, true).is_some_and(|s| s.contains("Snap the cut")));
        assert!(
            suggest(
                &Cause::Operation {
                    reason: crate::tier::ReEncodeReason::FreezeFrame
                },
                false
            )
            .is_none()
        );
        // Lossless sound is suggested only where the target was not.
        assert!(suggest(&Cause::AudioChain, false).is_some());
        assert!(suggest(&Cause::AudioChain, true).is_none());
    }
}
