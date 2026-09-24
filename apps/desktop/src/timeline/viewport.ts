/**
 * The timeline's view of time: which frame is at which pixel (#33).
 *
 * Pure arithmetic, so zoom anchoring can be asserted numerically. Positions
 * are sequence frames — the unit of the evaluated timeline — and pixels are
 * CSS pixels; device pixels are the painter's concern (`crisp`).
 */

export interface Viewport {
  /** CSS pixels per sequence frame. */
  readonly scale: number;
  /** The frame at the left edge of the clip area; may be fractional. */
  readonly origin: number;
  /** How far the track stack is scrolled down, in CSS pixels. */
  readonly scrollTop: number;
  /** The clip area, in CSS pixels. */
  readonly width: number;
  readonly height: number;
}

/**
 * The closest zoom: one frame this wide. Wide enough to see and grab a
 * single frame, which is what "zoom to a single frame" is for.
 */
export const MAX_SCALE = 96;

/** The farthest zoom never shows less than this many frames of room. */
const MIN_VISIBLE_FRAMES = 30;

/** How much of the width a fitted project fills: the rest is room to drop. */
const FIT_FILL = 0.92;

/** The farthest zoom for a project `length` frames long in `width` pixels. */
export function minScale(length: number, width: number): number {
  const frames = Math.max(length, MIN_VISIBLE_FRAMES);
  return Math.min(MAX_SCALE, (Math.max(width, 1) * FIT_FILL) / frames);
}

export function clampScale(
  scale: number,
  length: number,
  width: number,
): number {
  return Math.min(MAX_SCALE, Math.max(minScale(length, width), scale));
}

/** The frame under pixel `x`. */
export function frameAt(view: Viewport, x: number): number {
  return view.origin + x / view.scale;
}

/** The pixel of frame `frame`. */
export function xOf(view: Viewport, frame: number): number {
  return (frame - view.origin) * view.scale;
}

/** The first frame visible, and the first not visible — the range to draw. */
export function visibleFrames(view: Viewport): [number, number] {
  return [
    Math.floor(view.origin),
    Math.ceil(view.origin + view.width / view.scale),
  ];
}

/**
 * The furthest the view scrolls: the end of the project and one screen of
 * room after it, to drop onto.
 */
export function maxOrigin(view: Viewport, length: number): number {
  return Math.max(0, length + (view.width * 0.5) / view.scale - 1);
}

export function scrollTo(
  view: Viewport,
  origin: number,
  length: number,
): Viewport {
  return {
    ...view,
    origin: Math.min(maxOrigin(view, length), Math.max(0, origin)),
  };
}

/**
 * Zoom to `scale`, keeping the frame under pixel `anchorX` where it is — so
 * what the user is looking at stays under the pointer or the playhead.
 * Clamped to the zoom range; the origin never goes before frame 0.
 */
export function zoomAround(
  view: Viewport,
  scale: number,
  anchorX: number,
  length: number,
): Viewport {
  const next = clampScale(scale, length, view.width);
  const anchored = frameAt(view, anchorX);
  return {
    ...view,
    scale: next,
    origin: Math.max(0, anchored - anchorX / next),
  };
}

/** Each wheel notch or zoom key multiplies the scale by this. */
export const ZOOM_STEP = 1.25;

/** The whole project in view. */
export function fit(view: Viewport, length: number): Viewport {
  return { ...view, scale: minScale(length, view.width), origin: 0 };
}

/**
 * A 1-CSS-pixel line at `x`, placed on a device pixel's centre so it is one
 * device pixel wide and sharp at any display scaling — 100 %, 125 %, 150 %,
 * 200 %. A line at a device pixel's edge is smeared over two.
 */
export function crisp(x: number, dpr: number): number {
  return (Math.floor(x * dpr) + 0.5) / dpr;
}

/** A rectangle edge on a device-pixel boundary, so fills do not blur. */
export function snapToDevice(x: number, dpr: number): number {
  return Math.round(x * dpr) / dpr;
}

/**
 * Ruler ticks: the step between labelled ticks, in frames, chosen so labels
 * are at least `minGap` pixels apart. Steps are whole frames at close zoom
 * and whole seconds, then minutes, further out.
 */
export function rulerStep(scale: number, fps: number, minGap = 80): number {
  const second = Math.max(1, Math.round(fps));
  const steps = [
    1,
    2,
    5,
    10,
    second,
    second * 2,
    second * 5,
    second * 10,
    second * 30,
    second * 60,
    second * 120,
    second * 300,
    second * 600,
    second * 1800,
    second * 3600,
  ].filter((step) => step <= second || step % second === 0);
  return steps.find((step) => step * scale >= minGap) ?? second * 3600;
}
