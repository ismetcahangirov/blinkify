import { describe, expect, it } from "vitest";

import { canvasPoint, placeFrame } from "./orientation.js";

/** Round away float noise so corners compare exactly. */
const round = ({ x, y }: { x: number; y: number }) => ({
  x: Math.round(x * 1000) / 1000 + 0,
  y: Math.round(y * 1000) / 1000 + 0,
});

describe("placeFrame", () => {
  it("fits an unrotated frame and letterboxes it", () => {
    const placement = placeFrame(1920, 1080, 0, 960, 960);
    expect(placement).toMatchObject({
      centreX: 480,
      centreY: 480,
      width: 960,
      height: 540,
    });
    expect(placement.angle).toBe(-0);
  });

  it("fits a portrait phone frame by its displayed, rotated size", () => {
    // Coded 1280x720, displayed 720x1280, into a 1000x1000 canvas: the
    // displayed height is the constraint.
    const placement = placeFrame(1280, 720, 90, 1000, 1000);
    expect(placement.width).toBeCloseTo(1000);
    expect(placement.height).toBeCloseTo(562.5);
    expect(placement.angle).toBeCloseTo(-Math.PI / 2);
  });

  it("treats any quarter-turn multiple, including negative, as its turn", () => {
    expect(placeFrame(4, 2, 450, 10, 10).angle).toBeCloseTo(-Math.PI / 2);
    expect(placeFrame(4, 2, -90, 10, 10).angle).toBeCloseTo(-(3 * Math.PI) / 2);
  });
});

describe("the frame turns counter-clockwise by its rotation", () => {
  // A 4x2 frame in a 2x4 or 4x2 canvas at scale 1, so every corner lands on a
  // canvas corner and the direction of the turn is unambiguous.
  const corners = (
    rotation: number,
    canvasWidth: number,
    canvasHeight: number,
  ) => {
    const placement = placeFrame(4, 2, rotation, canvasWidth, canvasHeight);
    const at = (x: number, y: number) =>
      round(canvasPoint(placement, 4, 2, x, y));
    return { topLeft: at(0, 0), topRight: at(4, 0), bottomLeft: at(0, 2) };
  };

  it("0: unchanged", () => {
    expect(corners(0, 4, 2)).toEqual({
      topLeft: { x: 0, y: 0 },
      topRight: { x: 4, y: 0 },
      bottomLeft: { x: 0, y: 2 },
    });
  });

  it("90: the coded top-right corner becomes the displayed top-left", () => {
    expect(corners(90, 2, 4)).toEqual({
      topRight: { x: 0, y: 0 },
      topLeft: { x: 0, y: 4 },
      bottomLeft: { x: 2, y: 4 },
    });
  });

  it("180: upside down", () => {
    expect(corners(180, 4, 2)).toEqual({
      topLeft: { x: 4, y: 2 },
      topRight: { x: 0, y: 2 },
      bottomLeft: { x: 4, y: 0 },
    });
  });

  it("270: the coded bottom-left corner becomes the displayed top-left", () => {
    expect(corners(270, 2, 4)).toEqual({
      bottomLeft: { x: 0, y: 0 },
      topLeft: { x: 2, y: 0 },
      topRight: { x: 2, y: 4 },
    });
  });
});
