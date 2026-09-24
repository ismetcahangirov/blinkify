import type { Placement } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import { dragTo, type TrimDrag } from "../timeline/interaction.js";
import type { TrackRow } from "../timeline/draw.js";
import { timingEdit, timingOf } from "./ClipTimingFields.js";

/** 30 frames of a 1/1000 source at 30 fps, from 1 s in, at frame 60. */
const PLACEMENT: Placement = {
  track: 1,
  kind: "video",
  clip: 7,
  source: 1,
  stream: 0,
  timeBase: { num: 1, den: 1000 },
  sourceIn: 1000,
  sourceOut: 2000,
  start: 60,
  length: 30,
  speed: { num: 1, den: 1 },
  audio: [],
  sequenceTimeBase: { num: 1, den: 30 },
  silent: false,
};

describe("typed clip timing", () => {
  it("reads in, out and duration in frames", () => {
    expect(timingOf(PLACEMENT)).toEqual({ in: 30, out: 60, duration: 30 });
  });

  it("sends the edit a handle drag sends", () => {
    const row: TrackRow = {
      id: 1,
      kind: "video",
      top: 0,
      height: 56,
      placements: [PLACEMENT],
    };
    const drag: TrimDrag = {
      kind: "trim",
      placement: PLACEMENT,
      row,
      edge: "start",
      neighbour: null,
    };
    const dragged = dragTo(drag, {
      frames: 4,
      row,
      modifiers: { ctrl: false, shift: false, alt: true },
      rows: [row],
      view: { scale: 4, origin: 0, scrollTop: 0, width: 800, height: 200 },
      playhead: null,
      extents: [],
    }).edit;
    expect(timingEdit(PLACEMENT, "in", 34)).toEqual(dragged);
    expect(timingEdit(PLACEMENT, "duration", 20)).toEqual({
      edit: "trim-edge",
      clip: 7,
      edge: "end",
      frames: -10,
      ripple: false,
    });
    expect(timingEdit(PLACEMENT, "out", 60)).toBeNull();
  });

  it("turns source frames into timeline frames at the clip's speed", () => {
    const fast = { ...PLACEMENT, speed: { num: 2, den: 1 }, length: 15 };
    // Ten more source frames at double speed are five on the timeline.
    expect(timingEdit(fast, "out", 70)).toMatchObject({
      edge: "end",
      frames: 5,
    });
  });
});
