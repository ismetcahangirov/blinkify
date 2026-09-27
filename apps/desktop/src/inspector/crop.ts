import type {
  Aspect,
  ClipsCost,
  CropFrame,
  CropRect,
  Edit,
} from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";

/**
 * The crop section's wording and the little it reads off the engine's data
 * (#130). Pure, so every sentence is tested.
 *
 * Nothing here decides a rectangle or a tier. The preset rectangles are the
 * engine's (`view.frames`, rounded to each source's grid in one place), the
 * cost is the plan's (`clip_cost`), and a value the engine refuses is shown
 * with the engine's reason. This file only phrases them.
 */

export type Side = "x" | "y" | "width" | "height";

/** The four fields, in reading order: where the crop starts, then its size. */
export const SIDES: readonly { readonly side: Side; readonly label: string }[] =
  [
    { side: "x", label: "Left" },
    { side: "y", label: "Top" },
    { side: "width", label: "Width" },
    { side: "height", label: "Height" },
  ];

/** What the aspect control shows: a preset, or "free" when the sides match
 * none. */
export type AspectChoice = Aspect | "free";

export const ASPECTS: readonly {
  readonly value: AspectChoice;
  readonly label: string;
}[] = [
  { value: "free", label: "Free" },
  { value: "source", label: "Source" },
  { value: "16:9", label: "16:9 landscape" },
  { value: "9:16", label: "9:16 vertical" },
  { value: "1:1", label: "1:1 square" },
  { value: "4:5", label: "4:5 portrait" },
];

type Frame = DeepReadonly<CropFrame>;
type Rect = DeepReadonly<CropRect>;

/** The rectangle a clip shows: its crop, or the whole of its picture. */
export function shownRect(
  crop: Rect | undefined,
  frame: Frame | undefined,
): Rect | null {
  if (crop) return crop;
  return frame
    ? { x: 0, y: 0, width: frame.width, height: frame.height }
    : null;
}

/**
 * Which preset a rectangle is on its picture: the first whose rectangle has
 * its size, wherever it sits — a 9:16 crop moved to the left is still 9:16.
 * "free" when none has.
 */
export function presetOf(rect: Rect, frame: Frame): AspectChoice {
  return (
    frame.presets.find(
      (preset) =>
        preset.rect.width === rect.width && preset.rect.height === rect.height,
    )?.aspect ?? "free"
  );
}

/** The edit a field sends: that side, for every selected clip, and nothing
 * else — each clip keeps its own other sides. */
export function sideEdit(
  clips: readonly number[],
  side: Side,
  value: number,
): Edit {
  return {
    edit: "set-crop-sides",
    clips: [...clips],
    x: side === "x" ? value : null,
    y: side === "y" ? value : null,
    width: side === "width" ? value : null,
    height: side === "height" ? value : null,
  };
}

/** The side an edit sent by a field changes, to show its refusal there. */
export function sideOf(edit: Edit): Side | null {
  if (edit.edit !== "set-crop-sides") return null;
  return SIDES.find(({ side }) => edit[side] !== null)?.side ?? null;
}

/** `4.2 s`. */
export function seconds(value: number): string {
  return `${value.toFixed(1)} s`;
}

export type Tone = "lossless" | "re-encode" | "neutral";

/**
 * What exporting does to the selected clips' pictures and sound, from the
 * plan: "Cropping re-encodes this clip's pictures (4.2 s). Its sound is still
 * copied." — and, with no crop, the lossless statement.
 */
export function costStatement(
  cost: DeepReadonly<ClipsCost>,
): { readonly text: string; readonly tone: Tone } | null {
  if (cost.clips === 0) return null;
  const one = cost.clips === 1;
  const its = one ? "Its" : "Their";
  const parts: string[] = [];
  let tone: Tone = "lossless";
  if (cost.croppedClips > 0) {
    tone = "re-encode";
    parts.push(
      one
        ? `Cropping re-encodes this clip's pictures (${seconds(cost.croppedSeconds)}).`
        : cost.croppedClips === cost.clips
          ? `Cropping re-encodes the pictures of ${String(cost.clips)} clips (${seconds(cost.croppedSeconds)}).`
          : `Cropping re-encodes the pictures of ${String(cost.croppedClips)} of ${String(cost.clips)} clips (${seconds(cost.croppedSeconds)}).`,
    );
    if (cost.otherSeconds > 0)
      parts.push(
        `${one ? "It" : "Some"} would be re-encoded anyway: ${cost.otherReasons.join(" ")}`,
      );
  } else if (cost.otherSeconds > 0) {
    tone = "re-encode";
    parts.push(
      `Not cropped, but ${one ? "its" : "their"} pictures are re-encoded anyway: ${cost.otherReasons.join(" ")}`,
    );
  } else if (cost.seamedSeconds > 0) {
    tone = "neutral";
    parts.push(
      `Not cropped: ${one ? "its" : "their"} pictures are copied bit for bit, apart from a fraction of a second at a cut between keyframes.`,
    );
  } else {
    parts.push(
      one
        ? "Not cropped: this clip's pictures are copied bit for bit."
        : "Not cropped: their pictures are copied bit for bit.",
    );
  }
  const sound = cost.sound;
  if (sound && cost.croppedClips > 0)
    parts.push(
      sound.reEncodedSeconds === 0
        ? `${its} sound is still copied.`
        : `${its} sound is re-encoded for its own reasons, not because of the crop.`,
    );
  if (cost.declined)
    parts.push("Part of it cannot be exported as it stands: Export says why.");
  return { text: parts.join(" "), tone };
}
