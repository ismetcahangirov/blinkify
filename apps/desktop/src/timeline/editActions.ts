import type { CutPoint, Edit, Timeline } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import type { Placement } from "./draw.js";

/**
 * The timeline's edit actions (#35), worked out from what the editor shows:
 * the selection, the playhead, the keyframe indicator. Pure — each returns
 * the one edit to send and what to tell the user — so the rules are tested
 * without a window. The engine does the cutting; these only choose where.
 */

export interface Action {
  readonly edit: Edit | null;
  /** Said after the edit, or instead of it when there is nothing to do. */
  readonly notice: string | null;
}

/** How long a freeze frame lasts, as CapCut makes it: three seconds. */
export const FREEZE_SECONDS = 3;

/** Past this, reversing is worth a warning at the point of use. */
export const LONG_REVERSE_SECONDS = 60;

type View = DeepReadonly<Timeline> | null | undefined;

function placements(timeline: View): Placement[] {
  return (timeline?.tracks ?? []).flatMap((track) => [...track.placements]);
}

const covers = (p: Placement, frame: number) =>
  p.start <= frame && frame < p.start + p.length;

function seconds(frames: number, timeline: View): number {
  const rate = timeline?.frameRate;
  return rate && rate.num > 0 ? (frames * rate.den) / rate.num : 0;
}

function clock(total: number): string {
  const whole = Math.round(total);
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/**
 * Split at the playhead: the selected clips under it, or with none selected
 * every clip under it. With snap-to-keyframe on and the playhead off a
 * keyframe, the cut moves to the nearest keyframe inside the clip — and the
 * notice says where it landed, because moving a user's cut silently would be
 * a correctness change disguised as a convenience.
 */
export function splitAction(
  timeline: View,
  selection: readonly number[],
  playhead: number | null,
  cut: CutPoint | null,
  snapToKeyframe: boolean,
): Action {
  if (playhead === null)
    return { edit: null, notice: "Open the project's preview to split." };
  let at = playhead;
  let notice: string | null = null;
  if (snapToKeyframe && cut && cut.position === playhead && !cut.lossless) {
    const candidates = [cut.previous, cut.next].filter(
      (frame): frame is number => frame !== null && frame !== playhead,
    );
    const nearest = candidates.sort(
      (a, b) => Math.abs(a - playhead) - Math.abs(b - playhead),
    )[0];
    if (nearest !== undefined) {
      at = nearest;
      const gap = Math.abs(nearest - playhead);
      notice = `Cut moved to the keyframe ${gap} frame${gap === 1 ? "" : "s"} ${nearest < playhead ? "earlier" : "later"}.`;
    }
  }
  const under = placements(timeline).filter(
    (p) => covers(p, at) && p.start !== at,
  );
  const chosen = under
    .filter((p) => selection.includes(p.clip))
    .map((p) => p.clip);
  if (under.length === 0)
    return { edit: null, notice: "Nothing to split here." };
  return {
    edit: {
      edit: "split",
      clips: selection.length > 0 && chosen.length > 0 ? chosen : [],
      at,
    },
    notice,
  };
}

/** Freeze the frame at the playhead of the selected video clip under it, or
 * of the main track's clip there. */
export function freezeAction(
  timeline: View,
  selection: readonly number[],
  playhead: number | null,
): Action {
  if (playhead === null)
    return {
      edit: null,
      notice: "Open the project's preview to freeze a frame.",
    };
  const video = (timeline?.tracks ?? []).filter((t) => t.kind === "video");
  const under = video.flatMap((t) =>
    t.placements.filter((p) => covers(p, playhead) && !p.motion),
  );
  const clip =
    under.find((p) => selection.includes(p.clip)) ?? under[0] ?? null;
  if (!clip)
    return { edit: null, notice: "No video clip plays at the playhead." };
  const rate = timeline?.frameRate;
  const frames =
    rate && rate.den > 0
      ? Math.max(1, Math.round((FREEZE_SECONDS * rate.num) / rate.den))
      : 90;
  return {
    edit: { edit: "freeze-frame", clip: clip.clip, at: playhead, frames },
    notice: "The frozen frame is re-encoded at export.",
  };
}

/** Reverse the selected video clips — or play them forwards again when all
 * of them already run backwards. Warned at the point of use, not at export. */
export function reverseAction(
  timeline: View,
  selection: readonly number[],
): Action {
  const chosen = placements(timeline).filter(
    (p) =>
      selection.includes(p.clip) && p.kind === "video" && p.motion !== "hold",
  );
  if (chosen.length === 0)
    return { edit: null, notice: "Select a video clip to reverse." };
  const reverse = !chosen.every((p) => p.motion === "reverse");
  const total = seconds(
    chosen.reduce((sum, p) => sum + p.length, 0),
    timeline,
  );
  const notice = reverse
    ? total > LONG_REVERSE_SECONDS
      ? `Reversing ${clock(total)} of video re-encodes all of it at export, which takes a while.`
      : "A reversed clip is re-encoded at export."
    : null;
  return {
    edit: {
      edit: "set-reverse",
      clips: chosen.map((p) => p.clip),
      reverse,
    },
    notice,
  };
}

export function duplicateAction(selection: readonly number[]): Action {
  return selection.length === 0
    ? { edit: null, notice: "Select a clip to duplicate." }
    : { edit: { edit: "duplicate", clips: [...selection] }, notice: null };
}

/** What the keyframe indicator says about a cut at the playhead. */
export function cutStatement(cut: CutPoint | null): {
  state: "lossless" | "re-encode" | "none";
  text: string;
} {
  if (!cut) return { state: "none", text: "" };
  if (cut.lossless)
    return { state: "lossless", text: "Keyframe: a cut here is lossless" };
  if (cut.openGop)
    return {
      state: "re-encode",
      text: "Open-GOP keyframe: a cut here re-encodes the pictures around it",
    };
  const ahead =
    cut.next === null ? null : `next keyframe in ${cut.next - cut.position} f`;
  return {
    state: "re-encode",
    text: `A cut here re-encodes up to the next keyframe${ahead ? ` (${ahead})` : ""}`,
  };
}
