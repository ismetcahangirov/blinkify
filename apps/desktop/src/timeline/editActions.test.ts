import type { CutPoint, Placement, Timeline } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import {
  cutStatement,
  duplicateAction,
  freezeAction,
  reverseAction,
  splitAction,
} from "./editActions.js";

function clip(
  id: number,
  start: number,
  length: number,
  extra: Partial<Placement> = {},
): Placement {
  return {
    track: 1,
    kind: "video",
    clip: id,
    source: 1,
    stream: 0,
    timeBase: { num: 1, den: 90_000 },
    sourceIn: 0,
    sourceOut: length * 3000,
    start,
    length,
    speed: { num: 1, den: 1 },
    audio: [],
    sequenceTimeBase: { num: 1, den: 30 },
    ...extra,
  };
}

const TIMELINE: Timeline = {
  timeBase: { num: 1, den: 30 },
  frameRate: { num: 30, den: 1 },
  tracks: [
    { id: 1, kind: "video", placements: [clip(1, 0, 60), clip(2, 60, 3000)] },
    {
      id: 2,
      kind: "audio",
      placements: [clip(3, 0, 120, { kind: "audio", track: 2 })],
    },
  ],
};

const cut = (overrides: Partial<CutPoint> = {}): CutPoint => ({
  clip: 1,
  position: 40,
  lossless: false,
  openGop: false,
  previous: 30,
  next: 45,
  ...overrides,
});

describe("split", () => {
  it("cuts every clip under the playhead when none is selected", () => {
    expect(splitAction(TIMELINE, [], 40, null, false)).toEqual({
      edit: { edit: "split", clips: [], at: 40 },
      notice: null,
    });
  });

  it("cuts only the selected clips under the playhead", () => {
    expect(splitAction(TIMELINE, [3], 40, null, false).edit).toEqual({
      edit: "split",
      clips: [3],
      at: 40,
    });
  });

  it("leaves the cut alone unless snapping to keyframes is on", () => {
    expect(splitAction(TIMELINE, [], 40, cut(), false).edit).toMatchObject({
      at: 40,
    });
    const snapped = splitAction(TIMELINE, [], 40, cut(), true);
    expect(snapped.edit).toMatchObject({ at: 45 });
    expect(snapped.notice).toBe("Cut moved to the keyframe 5 frames later.");
    // Already on a keyframe: nothing moves and nothing is said.
    expect(
      splitAction(TIMELINE, [], 40, cut({ lossless: true }), true),
    ).toEqual({ edit: { edit: "split", clips: [], at: 40 }, notice: null });
  });

  it("says there is nothing to split at an edge or in a gap", () => {
    expect(splitAction(TIMELINE, [], 5000, null, false)).toEqual({
      edit: null,
      notice: "Nothing to split here.",
    });
    expect(splitAction(TIMELINE, [], null, null, false).edit).toBeNull();
  });
});

describe("freeze frame and reverse", () => {
  it("freezes three seconds of the video clip under the playhead", () => {
    const action = freezeAction(TIMELINE, [], 40);
    expect(action.edit).toEqual({
      edit: "freeze-frame",
      clip: 1,
      at: 40,
      frames: 90,
    });
    expect(action.notice).toMatch(/re-encoded at export/);
  });

  it("warns at the point of use that a long reverse re-encodes all of it", () => {
    const long = reverseAction(TIMELINE, [2]);
    expect(long.edit).toEqual({
      edit: "set-reverse",
      clips: [2],
      reverse: true,
    });
    expect(long.notice).toBe(
      "Reversing 1:40 of video re-encodes all of it at export, which takes a while.",
    );
    expect(reverseAction(TIMELINE, [1]).notice).toBe(
      "A reversed clip is re-encoded at export.",
    );
    // Sound is not reversed; a reversed clip is played forwards again.
    expect(reverseAction(TIMELINE, [3]).edit).toBeNull();
    const reversed: Timeline = {
      ...TIMELINE,
      tracks: [
        {
          id: 1,
          kind: "video",
          placements: [
            clip(1, 0, 60, { motion: "reverse", forced: "reverse" }),
          ],
        },
      ],
    };
    expect(reverseAction(reversed, [1]).edit).toMatchObject({ reverse: false });
  });

  it("duplicates the selection, or says to select something", () => {
    expect(duplicateAction([1, 2]).edit).toEqual({
      edit: "duplicate",
      clips: [1, 2],
    });
    expect(duplicateAction([]).edit).toBeNull();
  });
});

describe("the keyframe indicator", () => {
  it("says a cut on a keyframe is lossless and elsewhere what it costs", () => {
    expect(cutStatement(cut({ lossless: true }))).toEqual({
      state: "lossless",
      text: "Keyframe: a cut here is lossless",
    });
    expect(cutStatement(cut())).toEqual({
      state: "re-encode",
      text: "A cut here re-encodes up to the next keyframe (next keyframe in 5 f)",
    });
    expect(cutStatement(cut({ openGop: true })).text).toMatch(/Open-GOP/);
    expect(cutStatement(null).state).toBe("none");
  });
});
