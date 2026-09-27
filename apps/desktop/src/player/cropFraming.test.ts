import type { CropFrame, CropRect } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import {
  dragRect,
  handleAt,
  handlePoint,
  keptAspect,
  nudge,
  pictureBox,
  toCanvas,
  toSource,
  type Picture,
} from "./cropFraming.js";
import { canvasPoint, placeFrame } from "./orientation.js";

const VERTICAL: CropRect = { x: 656, y: 0, width: 608, height: 1080 };

const HD: CropFrame = {
  width: 1920,
  height: 1080,
  across: 2,
  down: 2,
  minSize: 16,
  presets: [
    { aspect: "source", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "16:9", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "9:16", rect: VERTICAL },
    { aspect: "1:1", rect: { x: 420, y: 0, width: 1080, height: 1080 } },
    { aspect: "4:5", rect: { x: 528, y: 0, width: 864, height: 1080 } },
  ],
};

/** A portrait phone clip: coded 1920 × 1080, shown 1080 × 1920. */
const PHONE: CropFrame = {
  width: 1080,
  height: 1920,
  across: 2,
  down: 2,
  minSize: 16,
  presets: [],
};

const RATIOS = [1, 1.25, 1.5, 2];

/**
 * Where source display point `x`, `y` is drawn on the canvas, in CSS pixels:
 * worked out the way `PreviewCanvas` draws — the decoded frame through
 * `placeFrame` and `canvasPoint` at device pixels — and not through the
 * module under test.
 */
function drawnAt(
  picture: Picture,
  frame: CropFrame,
  cssWidth: number,
  cssHeight: number,
  ratio: number,
  x: number,
  y: number,
): { x: number; y: number } {
  const width = Math.max(1, Math.round(cssWidth * ratio));
  const height = Math.max(1, Math.round(cssHeight * ratio));
  const placement = placeFrame(
    picture.width,
    picture.height,
    picture.rotation,
    width,
    height,
  );
  // The display point, back into the decoded frame: undo the
  // counter-clockwise turn the preview applies.
  const u =
    (x / frame.width) *
    (picture.rotation % 180 === 0 ? picture.width : picture.height);
  const v =
    (y / frame.height) *
    (picture.rotation % 180 === 0 ? picture.height : picture.width);
  const turns = (((picture.rotation / 90) % 4) + 4) % 4;
  const [fx, fy] =
    turns === 0
      ? [u, v]
      : turns === 1
        ? [picture.width - v, u]
        : turns === 2
          ? [picture.width - u, picture.height - v]
          : [v, picture.height - u];
  const point = canvasPoint(placement, picture.width, picture.height, fx, fy);
  return { x: point.x / ratio, y: point.y / ratio };
}

describe("the picture's box on the canvas", () => {
  for (const rotation of [0, 90, 180, 270]) {
    for (const ratio of RATIOS) {
      it(`maps source points exactly at ${String(rotation)}° and ${String(ratio * 100)} %`, () => {
        const picture: Picture = { width: 1920, height: 1080, rotation };
        const frame = rotation % 180 === 0 ? HD : PHONE;
        const box = pictureBox(picture, 801, 450, ratio);
        for (const [x, y] of [
          [0, 0],
          [frame.width, frame.height],
          [frame.width, 0],
          [0, frame.height],
          [123, 457],
        ] as const) {
          const drawn = drawnAt(picture, frame, 801, 450, ratio, x, y);
          const back = toSource(box, frame, drawn.x, drawn.y);
          expect(back.x).toBeCloseTo(x, 6);
          expect(back.y).toBeCloseTo(y, 6);
        }
      });
    }
  }

  it("draws a rectangle where its corners map", () => {
    const box = pictureBox(
      { width: 1920, height: 1080, rotation: 0 },
      960,
      540,
      1,
    );
    expect(box).toEqual({ left: 0, top: 0, width: 960, height: 540 });
    expect(toCanvas(box, HD, VERTICAL)).toEqual({
      left: 328,
      top: 0,
      width: 304,
      height: 540,
    });
  });
});

describe("taking hold of the rectangle", () => {
  const shown = { left: 100, top: 50, width: 200, height: 100 };

  it("finds a corner before an edge, an edge before the inside", () => {
    expect(handleAt(shown, 101, 51)).toBe("nw");
    expect(handleAt(shown, 300, 150)).toBe("se");
    expect(handleAt(shown, 200, 52)).toBe("n");
    expect(handleAt(shown, 299, 100)).toBe("e");
    expect(handleAt(shown, 200, 100)).toBe("move");
    expect(handleAt(shown, 20, 20)).toBeNull();
    expect(handlePoint(shown, "sw")).toEqual({ x: 100, y: 150 });
  });
});

describe("a drag", () => {
  it("snaps every side to the chroma grid", () => {
    const next = dragRect(VERTICAL, "se", -101.3, -33.7, HD, null);
    expect(next).toEqual({ x: 656, y: 0, width: 506, height: 1046 });
    for (const value of Object.values(next)) expect(value % 2).toBe(0);
  });

  it("stops at the frame's edge and at the minimum size", () => {
    expect(dragRect(VERTICAL, "e", 5000, 0, HD, null)).toEqual({
      ...VERTICAL,
      width: 1920 - 656,
    });
    expect(dragRect(VERTICAL, "w", -5000, 0, HD, null)).toEqual({
      ...VERTICAL,
      x: 0,
      width: 656 + 608,
    });
    expect(dragRect(VERTICAL, "e", -5000, 0, HD, null).width).toBe(16);
    expect(dragRect(VERTICAL, "move", 5000, 5000, HD, null)).toEqual({
      ...VERTICAL,
      x: 1920 - 608,
    });
  });

  it("stays on the grid on a picture of an odd size", () => {
    const odd: CropFrame = { ...HD, width: 1917, height: 1079 };
    const next = dragRect(
      { x: 0, y: 0, width: 1916, height: 1078 },
      "se",
      50,
      50,
      odd,
      null,
    );
    expect(next).toEqual({ x: 0, y: 0, width: 1916, height: 1078 });
    expect(nudge({ x: 0, y: 0, width: 600, height: 600 }, odd, 1000, 0).x).toBe(
      1316,
    );
  });

  it("keeps a preset's shape, anchored at the opposite corner", () => {
    const aspect = keptAspect(VERTICAL, HD, false);
    expect(aspect).toBeCloseTo(608 / 1080);
    const next = dragRect(VERTICAL, "nw", 304, 0, HD, aspect);
    expect(next.x + next.width).toBe(656 + 608);
    expect(next.y + next.height).toBe(1080);
    expect(next.width / next.height).toBeCloseTo(608 / 1080, 2);
    expect(next.width).toBeLessThan(608);
  });

  it("keeps a free shape only with Shift, and never the whole picture's", () => {
    const free = { x: 100, y: 100, width: 800, height: 400 };
    expect(keptAspect(free, HD, false)).toBeNull();
    expect(keptAspect(free, HD, true)).toBe(2);
    expect(
      keptAspect({ x: 0, y: 0, width: 1920, height: 1080 }, HD, false),
    ).toBeNull();
  });

  it("moves by grid steps with the arrow keys, kept inside", () => {
    expect(nudge(VERTICAL, HD, 1, 0).x).toBe(658);
    expect(nudge(VERTICAL, HD, -10, 0).x).toBe(636);
    expect(nudge(VERTICAL, HD, 0, -1).y).toBe(0);
  });
});

describe("a corner dragged on a portrait phone clip at 150 %", () => {
  it("lands the crop's corner where the pointer is released", () => {
    const picture: Picture = { width: 1920, height: 1080, rotation: 90 };
    const box = pictureBox(picture, 640, 480, 1.5);
    const start: CropRect = { x: 0, y: 0, width: 1080, height: 1920 };
    const grabbed = drawnAt(picture, PHONE, 640, 480, 1.5, 1080, 1920);
    // Released where source point (700, 1200) is drawn.
    const released = drawnAt(picture, PHONE, 640, 480, 1.5, 700, 1200);
    const from = toSource(box, PHONE, grabbed.x, grabbed.y);
    const to = toSource(box, PHONE, released.x, released.y);
    const next = dragRect(
      start,
      "se",
      to.x - from.x,
      to.y - from.y,
      PHONE,
      null,
    );
    expect(next.x + next.width).toBe(700);
    expect(next.y + next.height).toBe(1200);
  });
});
