import type { Placement as EvaluatedPlacement } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import type { TrackRow } from "./draw.js";
import {
  allClips,
  clickSelection,
  dragTo,
  edgeReach,
  hitTest,
  neighbourAt,
  snap,
  SNAP_PIXELS,
  snapTargets,
  type DragInput,
  type Hit,
  type MoveDrag,
  type TrimDrag,
} from "./interaction.js";
import type { Viewport } from "./viewport.js";

const NONE = { ctrl: false, shift: false, alt: false };

/** A clip of a 1/1000 source at 30 fps, `length` frames at `start`. */
function clip(
  id: number,
  start: number,
  length: number,
  kind: "video" | "audio" = "video",
  sourceIn = 1000,
): EvaluatedPlacement {
  return {
    track: 1,
    kind,
    clip: id,
    source: 1,
    stream: 0,
    timeBase: { num: 1, den: 1000 },
    sourceIn,
    sourceOut: sourceIn + Math.round((length * 1000) / 30),
    start,
    length,
    speed: { num: 1, den: 1 },
    audio: [],
    sequenceTimeBase: { num: 1, den: 30 },
  };
}

const V1: TrackRow = {
  id: 1,
  kind: "video",
  top: 0,
  height: 56,
  placements: [clip(1, 0, 30), clip(2, 30, 30), clip(3, 90, 30)],
};
const V2: TrackRow = {
  id: 2,
  kind: "video",
  top: 56,
  height: 56,
  placements: [],
};
const A1: TrackRow = {
  id: 3,
  kind: "audio",
  top: 112,
  height: 40,
  placements: [clip(4, 0, 60, "audio")],
};
const ROWS = [V1, V2, A1];

const view = (scale: number): Viewport => ({
  scale,
  origin: 0,
  scrollTop: 0,
  width: 2000,
  height: 300,
});

const EXTENTS = [
  {
    source: 1,
    stream: 0,
    timeBase: { num: 1, den: 1000 },
    start: 0,
    end: 10_000,
  },
];

function input(overrides: Partial<DragInput> = {}): DragInput {
  return {
    frames: 0,
    row: V1,
    modifiers: NONE,
    rows: ROWS,
    view: view(4),
    playhead: null,
    extents: EXTENTS,
    ...overrides,
  };
}

const at = (row: TrackRow, index: number): EvaluatedPlacement => {
  const placement = row.placements[index];
  if (!placement) throw new Error("no such clip");
  return placement as EvaluatedPlacement;
};

describe("clip hit testing and selection", () => {
  it("tells a clip's body from its edges", () => {
    const v = view(4);
    expect(hitTest(ROWS, v, 60, 10)?.zone).toBe("body");
    expect(hitTest(ROWS, v, 1, 10)?.zone).toBe("start");
    expect(hitTest(ROWS, v, 118, 10)?.zone).toBe("end");
    expect(hitTest(ROWS, v, 125, 10)?.placement.clip).toBe(2);
    expect(hitTest(ROWS, v, 300, 10)).toBeNull();
    expect(hitTest(ROWS, v, 10, 130)?.placement.clip).toBe(4);
  });

  it("selects one, adds with Ctrl, extends with Shift and clears on empty", () => {
    const hit = (index: number): Hit => ({
      row: V1,
      placement: at(V1, index),
      zone: "body",
    });
    expect(clickSelection([4], hit(0), NONE)).toEqual([1]);
    expect(clickSelection([1], hit(2), { ...NONE, ctrl: true })).toEqual([
      1, 3,
    ]);
    expect(clickSelection([1, 3], hit(2), { ...NONE, ctrl: true })).toEqual([
      1,
    ]);
    expect(clickSelection([1], hit(2), { ...NONE, shift: true })).toEqual([
      1, 2, 3,
    ]);
    expect(clickSelection([1, 2], null, NONE)).toEqual([]);
    expect(allClips(ROWS)).toEqual([1, 2, 3, 4]);
  });
});

describe("snapping", () => {
  it("engages within the same number of pixels at every zoom", () => {
    const targets = [300];
    for (const scale of [0.01, 0.3, 1, 4, 20, 96]) {
      const within = (SNAP_PIXELS - 0.5) / scale;
      const beyond = (SNAP_PIXELS + 0.5) / scale;
      expect(snap([300 - within], targets, scale).target).toBe(300);
      expect(snap([300 + within], targets, scale).shift).toBeCloseTo(-within);
      expect(snap([300 - beyond], targets, scale).target).toBeNull();
    }
  });

  it("snaps to clip edges, the playhead and the sequence start", () => {
    expect(snapTargets(ROWS, new Set([2]), 45)).toEqual([
      0, 30, 45, 60, 90, 120,
    ]);
  });

  it("is suspended while Alt is held", () => {
    const drag: MoveDrag = {
      kind: "move",
      clips: [{ placement: at(V1, 2), row: V1 }],
      from: V1,
    };
    // 90 → 60 is 30 frames; one frame short snaps onto clip 2's end.
    expect(dragTo(drag, input({ frames: -29 })).ghosts[0]?.start).toBe(60);
    expect(
      dragTo(drag, input({ frames: -29, modifiers: { ...NONE, alt: true } }))
        .ghosts[0]?.start,
    ).toBe(61);
  });
});

describe("moving clips", () => {
  const drag = (index: number): MoveDrag => ({
    kind: "move",
    clips: [{ placement: at(V1, index), row: V1 }],
    from: V1,
  });

  it("commits one move on release, wherever the pointer went on the way", () => {
    const result = dragTo(drag(2), input({ frames: 40.4 }));
    expect(result.edit).toEqual({
      edit: "move-clips",
      moves: [{ clip: 3, track: 1, start: 130 }],
    });
    expect(dragTo(drag(2), input({ frames: 0.2 })).edit).toBeNull();
  });

  it("moves to another track of the same kind", () => {
    const result = dragTo(drag(0), input({ frames: 200, row: V2 }));
    expect(result.invalid).toBeNull();
    expect(result.edit).toEqual({
      edit: "move-clips",
      moves: [{ clip: 1, track: 2, start: 200 }],
    });
  });

  it("refuses an incompatible track and an overlapping drop", () => {
    expect(dragTo(drag(0), input({ frames: 200, row: A1 })).invalid).toMatch(
      /cannot go on a audio track|video clip cannot/,
    );
    const overlapping = dragTo(
      drag(2),
      input({ frames: -45, modifiers: { ...NONE, alt: true } }),
    );
    expect(overlapping.invalid).toBe("Another clip is in the way.");
    expect(overlapping.edit).toBeNull();
  });

  it("never moves a clip before frame 0", () => {
    expect(dragTo(drag(1), input({ frames: -500 })).ghosts[0]?.start).toBe(0);
  });
});

describe("trimming", () => {
  const trimDrag = (index: number, edge: "start" | "end"): TrimDrag => ({
    kind: "trim",
    placement: at(V1, index),
    row: V1,
    edge,
    neighbour: neighbourAt(V1, at(V1, index), edge),
  });

  it("clamps at the source and shows it", () => {
    // Clip 3 starts 1000 ticks into a source that begins at 0: 30 frames.
    expect(edgeReach(at(V1, 2), "start", EXTENTS)[0]).toBe(-30);
    const result = dragTo(
      trimDrag(2, "start"),
      input({ frames: -80, modifiers: { ...NONE, alt: true } }),
    );
    expect(result.clamped).toBe(true);
    expect(result.ghosts[0]?.start).toBe(60);
    // Past the source's end: the source has 10000 ticks, the clip ends at 2000.
    expect(edgeReach(at(V1, 2), "end", EXTENTS)[1]).toBe(240);
  });

  it("stops at the neighbour unless it ripples", () => {
    const blocked = dragTo(trimDrag(0, "end"), input({ frames: 10 }));
    expect(blocked.clamped).toBe(true);
    expect(blocked.edit).toBeNull();
    const rippled = dragTo(
      trimDrag(0, "end"),
      input({ frames: 10, modifiers: { ...NONE, shift: true, alt: true } }),
    );
    expect(rippled.edit).toEqual({
      edit: "trim-edge",
      clip: 1,
      edge: "end",
      frames: 10,
      ripple: true,
    });
  });

  it("rolls a shared edge with Ctrl", () => {
    const result = dragTo(
      trimDrag(0, "end"),
      input({ frames: 6, modifiers: { ...NONE, ctrl: true, alt: true } }),
    );
    expect(result.edit).toEqual({ edit: "roll", left: 1, right: 2, frames: 6 });
    expect(result.ghosts.map((g) => [g.clip, g.start, g.length])).toEqual([
      [1, 0, 36],
      [2, 36, 24],
    ]);
    // Clip 3 has no neighbour at its start: Ctrl trims it as usual.
    expect(
      dragTo(
        trimDrag(2, "start"),
        input({ frames: 5, modifiers: { ...NONE, ctrl: true, alt: true } }),
      ).edit,
    ).toMatchObject({ edit: "trim-edge", clip: 3, edge: "start", frames: 5 });
  });

  it("never trims a clip below one frame", () => {
    const result = dragTo(
      trimDrag(2, "end"),
      input({ frames: -100, modifiers: { ...NONE, alt: true } }),
    );
    expect(result.ghosts[0]?.length).toBe(1);
    expect(result.clamped).toBe(true);
  });
});
