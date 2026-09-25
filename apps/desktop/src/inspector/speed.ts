import type { Rational, SpeedVerdict } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";

/**
 * The speed control's numbers (#56): how a slider position and a typed
 * factor become the one ratio the engine is sent, and how the engine's
 * verdict is put into words.
 *
 * Nothing here decides a tier. The verdict — copied or re-encoded, and why —
 * is the engine's (`project::speed::verdict`), sent with every view; this
 * file only phrases it.
 */

type Verdict = DeepReadonly<SpeedVerdict>;

/** A tenth of normal to a hundred times normal: the engine's `SPEED_RANGE`. */
export const MIN_SPEED = 0.1;
export const MAX_SPEED = 100;

/**
 * The finest speed step: a hundredth. Both the slider and the typed factor
 * land on this grid, so the two can only ever send the same ratio for the
 * same factor.
 */
const STEPS_PER_UNIT = 100;

/**
 * The slider moves in powers of two, not in factors. A linear track from
 * 0.1× to 100× would give everything below 2× — where nearly all edits are —
 * a few pixels. On a log scale 0.5×, 1× and 2× are evenly spaced, as they
 * are in CapCut.
 */
export const SLIDER_MIN = Math.log2(MIN_SPEED);
export const SLIDER_MAX = Math.log2(MAX_SPEED);
export const SLIDER_STEP = 0.01;

const clamp = (factor: number) =>
  Math.min(MAX_SPEED, Math.max(MIN_SPEED, factor));

function gcd(a: number, b: number): number {
  return b === 0 ? Math.abs(a) : gcd(b, a % b);
}

/** The ratio sent for `factor`: on the hundredths grid, in lowest terms. */
export function speedRatio(factor: number): Rational {
  const num = Math.round(clamp(factor) * STEPS_PER_UNIT);
  const divisor = gcd(num, STEPS_PER_UNIT);
  return { num: num / divisor, den: STEPS_PER_UNIT / divisor };
}

/** The factor a slider position stands for. */
export function sliderToFactor(position: number): number {
  return 2 ** position;
}

/** Where the slider sits for `factor`. */
export function factorToSlider(factor: number): number {
  return Math.log2(clamp(factor));
}

export const factorOf = (ratio: Rational): number => ratio.num / ratio.den;

export const sameRatio = (a: Rational, b: Rational): boolean =>
  a.num * b.den === b.num * a.den;

export const NORMAL_SPEED: Rational = { num: 1, den: 1 };

/** `2×`, `0.25×`, `1.37×` — at most two decimals, none trailing. */
export function formatFactor(factor: number): string {
  return `${Number.parseFloat(factor.toFixed(2))}×`;
}

/**
 * A rate exactly as the engine holds it — `60000/1001` — and, for reading,
 * its decimal. The exact form is what a bug report needs; the decimal is
 * what a person recognises.
 */
export function formatRate(rate: Rational): string {
  const exact = rate.den === 1 ? `${rate.num}` : `${rate.num}/${rate.den}`;
  const decimal = Number.parseFloat((rate.num / rate.den).toFixed(3));
  return rate.den === 1 ? `${exact} fps` : `${exact} fps (${decimal})`;
}

/** What the verdicts of the selected clips say, in one sentence and a tone. */
export interface SpeedStatement {
  readonly tone: "lossless" | "re-encode" | "mixed";
  readonly text: string;
}

export function speedStatement(
  verdicts: readonly Verdict[],
): SpeedStatement | null {
  if (verdicts.length === 0) return null;
  const copied = verdicts.filter((v) => v.tier.tier === "stream-copy").length;
  if (copied === verdicts.length)
    return {
      tone: "lossless",
      text: "Lossless: the video is copied, and only its timestamps change.",
    };
  if (copied === 0) {
    const outside = verdicts.flatMap((v) =>
      v.problems.filter((p) => p.problem === "frame-rate-outside-container"),
    );
    const [first] = outside;
    return {
      tone: "re-encode",
      text: first
        ? `Re-encoded: ${formatRate(first.rate)} is outside what a video file can carry (1–240 fps), so the pictures are re-timed to the sequence rate.`
        : "Re-encoded at this speed.",
    };
  }
  return {
    tone: "mixed",
    text: `Mixed: ${copied} of ${verdicts.length} clips are copied at this speed; the rest are re-encoded.`,
  };
}

/** The warnings to raise now, rather than at export: one per kind. */
export function speedWarnings(verdicts: readonly Verdict[]): string[] {
  const warnings = new Set<string>();
  for (const verdict of verdicts)
    for (const problem of verdict.problems)
      if (problem.problem === "frame-rate-outside-container")
        warnings.add(
          `${formatRate(problem.rate)} is not a frame rate a video file can carry.`,
        );
      else
        warnings.add(
          `${formatRate(problem.rate)} will stutter: Blinkify copies every recorded frame and does not invent new ones.`,
        );
  return [...warnings];
}
