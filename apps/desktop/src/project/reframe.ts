import type { Aspect, ReframeImpact } from "@blinkify/types";
import type { DeepReadonly } from "./project.store.js";
import { duration } from "./SequenceSettingsDialog.js";

/**
 * The reframe dialog's wording (#132). Pure, so every sentence is tested.
 *
 * The decisions — the size, which clips are cropped, which are left — and
 * the cost are the engine's (`preview_reframe`, the plans before and after);
 * this only phrases them, before anything is applied.
 */

/** The shapes a sequence can be reframed to, the commonest first. */
export const REFRAME_ASPECTS: readonly {
  readonly value: Aspect;
  readonly label: string;
}[] = [
  { value: "9:16", label: "9:16 vertical" },
  { value: "1:1", label: "1:1 square" },
  { value: "4:5", label: "4:5 portrait" },
  { value: "16:9", label: "16:9 landscape" },
];

const clips = (count: number) => (count === 1 ? "1 clip" : `${count} clips`);
const is = (count: number) => (count === 1 ? "is" : "are");

/** What reframing would do and cost, in the order a user decides by: the
 * cost, what stays a copy, the size and why, and what is left alone. */
export function reframeStatement(
  impact: DeepReadonly<ReframeImpact>,
  sourceName: (source: number) => string,
): readonly string[] {
  const { reframe } = impact;
  const lines: string[] = [];
  const lost = impact.reEncodedClips.length;
  lines.push(
    lost > 0
      ? `${clips(lost)} (${duration(impact.reEncodedSeconds)}) would no longer be copied and would be re-encoded because of the reframe.`
      : "No clip that is copied now would be re-encoded.",
  );
  if (impact.stillReEncodedClips.length > 0)
    lines.push(
      `${clips(impact.stillReEncodedClips.length)} already re-encoded would stay so.`,
    );
  const kept = impact.copiedClips.length;
  if (kept > 0)
    lines.push(
      `${clips(kept)} (${duration(impact.copiedSeconds)}) would still be copied bit for bit${
        reframe.uncropped.length > 0
          ? `: ${clips(reframe.uncropped.length)} already ${reframe.aspect} ${is(reframe.uncropped.length)} not cropped`
          : ""
      }.`,
    );
  const { width, height } = reframe.settings;
  const name = sourceName(reframe.basis.source);
  lines.push(
    reframe.basis.basis === "native"
      ? `The sequence becomes ${width} × ${height}, the size of ${name}, which is already ${reframe.aspect}, so its clips are copied.`
      : `The sequence becomes ${width} × ${height}, the ${reframe.aspect} crop of ${name}, so it is not scaled up.`,
  );
  const up = reframe.scaledUp.length;
  if (up > 0)
    lines.push(
      `${clips(up)} ${is(up)} smaller than that and would be scaled up to fill it.`,
    );
  if (reframe.kept.length > 0)
    lines.push(
      `${clips(reframe.kept.length)} already cropped to ${reframe.aspect} keep ${reframe.kept.length === 1 ? "its crop" : "their crops"} where ${reframe.kept.length === 1 ? "it is" : "they are"}.`,
    );
  const locked = reframe.locked.length;
  if (locked > 0)
    lines.push(
      `${clips(locked)} on locked tracks ${is(locked)} left as ${locked === 1 ? "it is" : "they are"}, not cropped; the new size still applies to ${locked === 1 ? "it" : "them"}.`,
    );
  if (impact.declined)
    lines.push(
      "Part of the reframed sequence could not be exported as it stands: Export says why.",
    );
  return lines;
}
