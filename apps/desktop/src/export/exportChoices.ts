import type {
  AudioEncoding,
  AudioTarget,
  ExportPlan,
  Rational,
  SizeEstimate,
  SnappedCut,
  StreamClaim,
} from "@blinkify/types";

import { formatTimecode } from "../player/timecode.js";
import { size } from "./exportJobs.js";

/**
 * The export dialog's choices and wording (#50). Pure, so every sentence
 * the dialog shows is tested without a window.
 *
 * Nothing here decides what is copied: the engine's plan and overview do
 * (`CLAUDE.md` section 2). This only chooses a container and a sound target
 * and puts the engine's answer into words.
 */

export type Container = "mp4" | "mov" | "mkv" | "webm";

export const CONTAINERS: readonly { value: Container; label: string }[] = [
  { value: "mp4", label: "MP4" },
  { value: "mov", label: "QuickTime (MOV)" },
  { value: "mkv", label: "Matroska (MKV)" },
  { value: "webm", label: "WebM" },
];

export interface AudioChoice {
  readonly id: string;
  readonly label: string;
  readonly target: AudioTarget;
  readonly lossless: boolean;
}

export const AUDIO_CHOICES: readonly AudioChoice[] = [
  {
    id: "aac-256",
    label: "AAC 256 kb/s",
    target: { codec: "aac", kilobits: 256 },
    lossless: false,
  },
  {
    id: "aac-320",
    label: "AAC 320 kb/s",
    target: { codec: "aac", kilobits: 320 },
    lossless: false,
  },
  {
    id: "opus-192",
    label: "Opus 192 kb/s",
    target: { codec: "opus", kilobits: 192 },
    lossless: false,
  },
  {
    id: "flac",
    label: "FLAC (lossless)",
    target: { codec: "flac" },
    lossless: true,
  },
  {
    id: "pcm",
    label: "PCM (lossless, uncompressed)",
    target: { codec: "pcm" },
    lossless: true,
  },
];

/** Which sound targets a container holds. */
export function audioChoicesFor(container: Container): readonly AudioChoice[] {
  switch (container) {
    case "webm":
      return AUDIO_CHOICES.filter((c) => c.target.codec === "opus");
    case "mp4":
      return AUDIO_CHOICES.filter((c) => c.target.codec !== "pcm");
    case "mov":
    case "mkv":
      return AUDIO_CHOICES;
  }
}

export interface Preset {
  readonly id: string;
  readonly label: string;
  readonly description: string;
  /** `source`: the container the first source is in. */
  readonly container: Container | "source";
  readonly audio: string;
}

export const PRESERVE = "preserve";

export const PRESETS: readonly Preset[] = [
  {
    id: PRESERVE,
    label: "Preserve the source exactly",
    description:
      "The source's own container and packets. Any sound that must be re-encoded is made lossless.",
    container: "source",
    audio: "flac",
  },
  {
    id: "share",
    label: "MP4 for sharing",
    description: "Plays everywhere. Re-encoded sound is AAC 256 kb/s.",
    container: "mp4",
    audio: "aac-256",
  },
  {
    id: "archive",
    label: "Matroska archive",
    description: "Holds any codec. Re-encoded sound is FLAC, lossless.",
    container: "mkv",
    audio: "flac",
  },
  {
    id: "web",
    label: "WebM for the web",
    description: "VP9 or AV1 pictures only. Re-encoded sound is Opus.",
    container: "webm",
    audio: "opus-192",
  },
];

/** The container a path's extension names, if Blinkify writes it. */
export function containerOf(path: string): Container | null {
  const extension = path.split(".").pop()?.toLowerCase() ?? "";
  if (extension === "mp4" || extension === "m4v") return "mp4";
  if (extension === "mov") return "mov";
  if (extension === "mkv") return "mkv";
  if (extension === "webm") return "webm";
  return null;
}

/**
 * The container "preserve the source exactly" writes for a source at
 * `path`: its own — except WebM, which cannot hold lossless sound, where
 * Matroska carries the same packets and can.
 */
export function preservingContainer(path: string | undefined): Container {
  const own = path ? containerOf(path) : null;
  if (own === null) return "mp4";
  return own === "webm" ? "mkv" : own;
}

/** `path` with its extension made `container`'s. */
export function withContainer(path: string, container: Container): string {
  const slash = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  const dot = path.lastIndexOf(".");
  const stem = dot > slash ? path.slice(0, dot) : path;
  return `${stem}.${container}`;
}

/** `0.40 s later`, `1.20 s earlier`, `unchanged`. */
export function shift(seconds: number): string {
  if (Math.abs(seconds) < 0.005) return "unchanged";
  return `${Math.abs(seconds).toFixed(2)} s ${seconds > 0 ? "later" : "earlier"}`;
}

/** What the snap does to one clip, in a line. */
export function cutLine(cut: SnappedCut, frameRate: Rational): string {
  return `Clip at ${timecodeAt(cut.atSeconds, frameRate)}: start ${shift(cut.inShiftSeconds)}, end ${shift(cut.outShiftSeconds)}`;
}

/** A time on the sequence as the player shows it. */
export function timecodeAt(seconds: number, frameRate: Rational): string {
  const rate = frameRate.den > 0 ? frameRate.num / frameRate.den : 30;
  return formatTimecode(Math.round(seconds * rate), frameRate);
}

/** `1:05.4` for a duration. */
export function duration(seconds: number): string {
  const whole = Math.max(0, seconds);
  const minutes = Math.floor(whole / 60);
  const rest = whole - minutes * 60;
  return `${minutes}:${rest.toFixed(1).padStart(4, "0")}`;
}

/** What happens to the pictures, in a line. */
export function picturesLine(claim: StreamClaim): string {
  if (claim.lossless) return "Pictures: copied bit for bit";
  return `Pictures: ${claim.reEncodedSeconds.toFixed(2)} s of ${(claim.copiedSeconds + claim.reEncodedSeconds).toFixed(2)} s re-encoded, the rest copied bit for bit`;
}

/** What happens to the sound, in a line, naming what it is encoded to. */
export function soundLine(
  claim: StreamClaim,
  encoding: AudioEncoding | undefined,
): string {
  if (claim.lossless) return "Sound: copied bit for bit";
  const made = encoding
    ? ` as ${encodingName(encoding)}${encoding.matchesCopied ? ", to match the copied sound" : ""}`
    : "";
  return `Sound: ${claim.reEncodedSeconds.toFixed(2)} s re-encoded${made}, the rest copied`;
}

/** `FLAC, lossless`, `AAC 256 kb/s`. */
export function encodingName(encoding: AudioEncoding): string {
  const codec =
    encoding.codec === "aac"
      ? "AAC"
      : encoding.codec === "opus"
        ? "Opus"
        : encoding.codec === "flac"
          ? "FLAC"
          : encoding.codec.startsWith("pcm")
            ? "PCM"
            : encoding.codec;
  if (encoding.lossless) return `${codec}, lossless`;
  return encoding.kilobits === null
    ? codec
    : `${codec} ${encoding.kilobits} kb/s`;
}

/** The size, saying whether it is an estimate. */
export function sizeLine(estimate: SizeEstimate): string {
  return estimate.precise
    ? `About ${size(estimate.bytes)}`
    : `Roughly ${size(estimate.bytes)} (an estimate: part of it is re-encoded, or its rates are not recorded)`;
}

/**
 * The application bar's summary of the plan: `Lossless`, `2 seams`,
 * `3 segments re-encoded`, or what stops the export.
 */
export function planSummary(plan: ExportPlan): {
  readonly text: string;
  readonly state: "lossless" | "seams" | "re-encoded" | "declined";
} {
  const declined = plan.segments.filter((s) => s.decline !== undefined).length;
  if (declined > 0)
    return {
      text:
        declined === 1
          ? "1 cut cannot export"
          : `${declined} cuts cannot export`,
      state: "declined",
    };
  if (plan.summary.lossless) return { text: "Lossless", state: "lossless" };
  const seams = plan.segments.filter((s) => s.tier.tier === "smart-cut").length;
  const whole = plan.segments.filter(
    (s) => s.tier.tier === "full-re-encode",
  ).length;
  const parts: string[] = [];
  if (seams > 0) parts.push(seams === 1 ? "1 seam" : `${seams} seams`);
  if (whole > 0)
    parts.push(
      whole === 1 ? "1 segment re-encoded" : `${whole} segments re-encoded`,
    );
  return {
    text: parts.join(" · "),
    state: whole > 0 ? "re-encoded" : "seams",
  };
}
