import type { Edit, GainAdvice } from "@blinkify/types";
import type { Placement } from "../timeline/draw.js";

/**
 * Clip gain in the inspector (#46): the settings a clip's gain step holds,
 * the edit that changes them, and the engine's advice put into words.
 *
 * Nothing here decides what the limiter does. How far it turns the peaks
 * down is the engine's figure (`gain_advice`), worked out from the clip's
 * measured true peak; this only phrases it.
 */

/** The slider's range, in dB. The field accepts the same. */
export const GAIN_MIN = -24;
export const GAIN_MAX = 24;

/** The limiter's default ceiling and the lowest the engine accepts, dBTP. */
export const DEFAULT_CEILING = -1;
export const CEILING_MIN = -20;

export interface GainSettings {
  readonly db: number;
  readonly ceilingDbtp: number;
  readonly bypassed: boolean;
}

export const NO_GAIN: GainSettings = {
  db: 0,
  ceilingDbtp: DEFAULT_CEILING,
  bypassed: false,
};

/** The clip's gain step, or no gain at the default ceiling. */
export function gainOf(clip: Placement): GainSettings {
  for (const step of clip.audio)
    if (step.op === "gain")
      return {
        db: step.db,
        ceilingDbtp: step.ceilingDbtp,
        bypassed: step.bypassed,
      };
  return NO_GAIN;
}

export function sameGain(a: GainSettings, b: GainSettings): boolean {
  return (
    a.db === b.db &&
    a.ceilingDbtp === b.ceilingDbtp &&
    a.bypassed === b.bypassed
  );
}

/** The edit that gives every clip in `clips` the gain `settings`. At 0 dB the
 * engine removes the step, so the sound is copied again. */
export function setGain(
  clips: readonly number[],
  settings: GainSettings,
): Edit {
  return {
    edit: "set-audio",
    clips: [...clips],
    step: { op: "gain", ...settings },
  };
}

/** Decibels to one place, with a sign and a typographic minus. */
export function formatDb(value: number): string {
  const text = Math.abs(value).toFixed(1);
  if (text === "0.0") return "0.0";
  return value < 0 ? `−${text}` : `+${text}`;
}

/**
 * What the limiter does at the clip's gain, as the engine measured it.
 * Engaging is stated with its amount; a limiter that is idle says so too, so
 * silence from the panel is never mistaken for "not checked".
 */
export function limiterStatement(advice: GainAdvice): string {
  const ceiling = formatDb(advice.ceilingDbtp);
  if (advice.before.truePeakDbtp === null)
    return "The clip is silent: there is nothing for the limiter to do.";
  if (advice.limitingDb > 0)
    return `The limiter is turning the loudest peaks down by up to ${advice.limitingDb.toFixed(1)} dB to hold them at ${ceiling} dBTP, rather than letting them clip.`;
  return `The peaks stay under ${ceiling} dBTP: the limiter is not engaging.`;
}

/** The suggestion, with what it would cost, or why there is none. */
export function suggestionText(advice: GainAdvice): string {
  if (advice.suggestedDb === null)
    return "The clip is too quiet to measure, so there is no suggestion.";
  const target = formatDb(advice.targetLufs);
  const cost =
    advice.suggestedLimitingDb > 0
      ? `, with the limiter turning peaks down by up to ${advice.suggestedLimitingDb.toFixed(1)} dB`
      : "";
  return `${formatDb(advice.suggestedDb)} dB brings it to ${target} LUFS${cost}.`;
}

/** The clip's level before its gain, for the facts list. */
export function measuredLevel(advice: GainAdvice): string {
  const { integratedLufs, truePeakDbtp } = advice.before;
  const loudness =
    integratedLufs === null ? "—" : `${formatDb(integratedLufs)} LUFS`;
  const peak = truePeakDbtp === null ? "—" : `${formatDb(truePeakDbtp)} dBTP`;
  return `${loudness}, peak ${peak}`;
}
