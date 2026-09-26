import type { Edit } from "@blinkify/types";
import type { Placement } from "../timeline/draw.js";

/**
 * Noise reduction in the inspector (#47): the strength a clip's denoise step
 * holds, and the edit that changes it.
 *
 * Strength is a blend of the denoised sound with the original, from 0 (the
 * original) to 1 (fully denoised). Bypass is the A/B comparison: the step
 * keeps its strength and the clip plays as recorded, switching on the next
 * sample rather than restarting playback.
 */

export interface DenoiseSettings {
  readonly strength: number;
  readonly bypassed: boolean;
}

export const NO_DENOISE: DenoiseSettings = { strength: 0, bypassed: false };

/** The strength a new noise reduction starts at: most of the effect, with
 * some of the original kept so clean passages do not sound processed. */
export const DEFAULT_STRENGTH = 0.7;

export function denoiseOf(clip: Placement): DenoiseSettings {
  for (const step of clip.audio)
    if (step.op === "denoise")
      return { strength: step.strength, bypassed: step.bypassed };
  return NO_DENOISE;
}

export function sameDenoise(a: DenoiseSettings, b: DenoiseSettings): boolean {
  return a.strength === b.strength && a.bypassed === b.bypassed;
}

/** The edit that gives every clip in `clips` the noise reduction `settings`.
 * At strength 0 the engine removes the step. */
export function setDenoise(
  clips: readonly number[],
  settings: DenoiseSettings,
): Edit {
  return {
    edit: "set-audio",
    clips: [...clips],
    step: { op: "denoise", ...settings },
  };
}

/** Strength as the percentage the slider shows. */
export function formatStrength(strength: number): string {
  return `${String(Math.round(strength * 100))} %`;
}
