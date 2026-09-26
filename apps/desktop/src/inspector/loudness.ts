import type { Edit, Loudness, LoudnessReport } from "@blinkify/types";
import type { Placement } from "../timeline/draw.js";
import { formatDb } from "./gain.js";

/**
 * Loudness normalisation in the inspector (#48): the settings a clip's
 * normalisation holds, the platforms' targets, and the engine's two-pass
 * report put into words — in LUFS, the unit a platform's requirement is
 * stated in, never as a percentage.
 */

export interface NormaliseSettings {
  readonly targetLufs: number;
  readonly ceilingDbtp: number;
  readonly bypassed: boolean;
}

export interface Preset {
  readonly id: string;
  readonly label: string;
  readonly targetLufs: number;
  readonly ceilingDbtp: number;
}

/** Where the common destinations play programmes back. */
export const PRESETS: readonly Preset[] = [
  {
    id: "streaming",
    label: "Streaming — YouTube, Spotify (−14 LUFS)",
    targetLufs: -14,
    ceilingDbtp: -1,
  },
  {
    id: "podcast",
    label: "Podcast — Apple (−16 LUFS)",
    targetLufs: -16,
    ceilingDbtp: -1,
  },
  {
    id: "broadcast",
    label: "Broadcast — EBU R128 (−23 LUFS)",
    targetLufs: -23,
    ceilingDbtp: -1,
  },
  {
    id: "atsc",
    label: "Broadcast — ATSC A/85 (−24 LUFS)",
    targetLufs: -24,
    ceilingDbtp: -2,
  },
];

/** A new normalisation starts at the streaming target. */
export const DEFAULT_NORMALISE: NormaliseSettings = {
  targetLufs: -14,
  ceilingDbtp: -1,
  bypassed: false,
};

export const TARGET_MIN = -40;
export const TARGET_MAX = -5;

/** The preset `settings` match, or `custom`. */
export function presetOf(settings: {
  readonly targetLufs: number;
  readonly ceilingDbtp: number;
}): string {
  return (
    PRESETS.find(
      (preset) =>
        preset.targetLufs === settings.targetLufs &&
        preset.ceilingDbtp === settings.ceilingDbtp,
    )?.id ?? "custom"
  );
}

/** The clip's own normalisation, or `null`. */
export function normaliseOf(clip: Placement): NormaliseSettings | null {
  for (const step of clip.audio)
    if (step.op === "normalise")
      return {
        targetLufs: step.targetLufs,
        ceilingDbtp: step.ceilingDbtp,
        bypassed: step.bypassed,
      };
  return null;
}

export function sameNormalise(
  a: NormaliseSettings | null,
  b: NormaliseSettings | null,
): boolean {
  if (a === null || b === null) return a === b;
  return (
    a.targetLufs === b.targetLufs &&
    a.ceilingDbtp === b.ceilingDbtp &&
    a.bypassed === b.bypassed
  );
}

export function setNormalise(
  clips: readonly number[],
  settings: NormaliseSettings,
): Edit {
  return {
    edit: "set-audio",
    clips: [...clips],
    step: { op: "normalise", ...settings },
  };
}

export function setSequenceLoudness(
  settings: {
    readonly targetLufs: number;
    readonly ceilingDbtp: number;
  } | null,
): Edit {
  return {
    edit: "set-sequence-loudness",
    loudness:
      settings === null
        ? null
        : {
            targetLufs: settings.targetLufs,
            ceilingDbtp: settings.ceilingDbtp,
          },
  };
}

/** A measurement in words: loudness, range and true peak. */
export function describeLoudness(loudness: Loudness): string {
  if (loudness.integratedLufs === null)
    return "too quiet to measure (below the −70 LUFS gate)";
  const parts = [`${formatDb(loudness.integratedLufs)} LUFS`];
  if (loudness.rangeLu !== null)
    parts.push(`range ${loudness.rangeLu.toFixed(1)} LU`);
  if (loudness.truePeakDbtp !== null)
    parts.push(`peak ${formatDb(loudness.truePeakDbtp)} dBTP`);
  return parts.join(", ");
}

/** What the two passes did, in one sentence. */
export function reportStatement(report: LoudnessReport): string {
  if (report.before.integratedLufs === null)
    return "Too quiet to measure: normalisation leaves it where it is.";
  return `One gain of ${formatDb(report.gainDb)} dB brings it to ${formatDb(report.targetLufs)} LUFS, with peaks held under ${formatDb(report.ceilingDbtp)} dBTP. The level does not ride the sound.`;
}
