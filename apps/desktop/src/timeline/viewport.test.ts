import { describe, expect, it } from "vitest";
import {
  clampScale,
  crisp,
  fit,
  frameAt,
  MAX_SCALE,
  maxOrigin,
  minScale,
  rulerStep,
  scrollTo,
  visibleFrames,
  xOf,
  zoomAround,
  ZOOM_STEP,
  type Viewport,
} from "./viewport.js";

const view: Viewport = {
  scale: 2,
  origin: 100,
  scrollTop: 0,
  width: 1000,
  height: 300,
};

describe("the timeline viewport", () => {
  it("maps frames to pixels and back", () => {
    expect(xOf(view, 100)).toBe(0);
    expect(xOf(view, 350)).toBe(500);
    expect(frameAt(view, 500)).toBe(350);
    expect(visibleFrames(view)).toEqual([100, 600]);
  });

  it("keeps the frame under the anchor where it is, at every zoom", () => {
    const length = 100_000;
    for (const anchor of [0, 137, 500, 999]) {
      let current = view;
      const under = frameAt(current, anchor);
      for (let step = 0; step < 30; step++) {
        current = zoomAround(
          current,
          current.scale * ZOOM_STEP,
          anchor,
          length,
        );
        expect(frameAt(current, anchor)).toBeCloseTo(under, 9);
      }
      for (let step = 0; step < 30; step++) {
        const next = zoomAround(
          current,
          current.scale / ZOOM_STEP,
          anchor,
          length,
        );
        // Anchored until the origin reaches frame 0, which it may not pass.
        if (next.origin > 0)
          expect(frameAt(next, anchor)).toBeCloseTo(under, 9);
        current = next;
      }
    }
  });

  it("zooms from the whole project to a single frame, and no further", () => {
    const length = 30 * 60 * 60; // an hour at 30 fps
    const whole = fit(view, length);
    expect(whole.origin).toBe(0);
    expect(xOf(whole, length)).toBeLessThanOrEqual(view.width);
    expect(xOf(whole, length)).toBeGreaterThan(view.width * 0.9);
    let close = whole;
    for (let step = 0; step < 200; step++)
      close = zoomAround(close, close.scale * ZOOM_STEP, 500, length);
    expect(close.scale).toBe(MAX_SCALE);
    // A single frame is wide enough to see and grab.
    expect(xOf(close, 1) - xOf(close, 0)).toBe(MAX_SCALE);
    expect(clampScale(0.000001, length, view.width)).toBe(
      minScale(length, view.width),
    );
  });

  it("never scrolls before frame 0 or far past the end", () => {
    expect(scrollTo(view, -50, 1000).origin).toBe(0);
    expect(scrollTo(view, 1e9, 1000).origin).toBe(maxOrigin(view, 1000));
  });

  it("puts a one-pixel line on a device pixel's centre at every scaling", () => {
    for (const dpr of [1, 1.25, 1.5, 2]) {
      for (const x of [0, 0.3, 17.5, 101.77, 640.02]) {
        const device = crisp(x, dpr) * dpr;
        expect(device - Math.floor(device)).toBeCloseTo(0.5, 9);
        // Never moved by more than a device pixel.
        expect(Math.abs(crisp(x, dpr) - x)).toBeLessThanOrEqual(1 / dpr);
      }
    }
  });

  it("spaces ruler labels by frames close in and by seconds and minutes out", () => {
    expect(rulerStep(MAX_SCALE, 30)).toBe(1);
    expect(rulerStep(10, 30)).toBe(10);
    expect(rulerStep(2, 30)).toBe(60);
    expect(rulerStep(0.01, 30)).toBe(30 * 300);
    for (const scale of [0.001, 0.05, 1, 7, 50]) {
      expect(rulerStep(scale, 25) * scale).toBeGreaterThanOrEqual(80);
    }
  });
});
