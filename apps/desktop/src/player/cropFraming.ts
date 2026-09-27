import type { CropFrame, CropRect } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import { placeFrame } from "./orientation.js";
import type { Picture } from "./preview.store.js";

export type { Picture };

/**
 * The arithmetic of framing a crop on the preview (#131). Pure, so every
 * mapping is tested at every rotation and display scale.
 *
 * Three spaces meet here:
 *
 * - **The frame as decoded** — coded orientation, possibly a proxy's size —
 *   which `PreviewCanvas` turns upright and fits with `placeFrame`.
 * - **The canvas box** — where that upright picture lands, in CSS pixels, the
 *   space pointer events arrive in.
 * - **The source's display pixels** — the crop's own space (ADR-0019), the
 *   size `CropFrame` gives.
 *
 * The box is worked out exactly as `PreviewCanvas` draws: from the canvas's
 * device-pixel size, rounded as it rounds it, then back to CSS pixels. A crop
 * that is right at 100 % and a pixel off at 150 % is the bug this avoids.
 *
 * Every rectangle that leaves this file is on the source's chroma grid,
 * inside the picture and at least the minimum size, so the engine has
 * nothing to refuse for a drag.
 */

type Frame = DeepReadonly<CropFrame>;
type Rect = DeepReadonly<CropRect>;

/** Where the upright picture lies on the canvas, in CSS pixels. */
export interface Box {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The canvas box of `picture` in a `cssWidth` × `cssHeight` canvas shown at
 * device pixel ratio `ratio` — what `PreviewCanvas` fills.
 */
export function pictureBox(
  picture: Picture,
  cssWidth: number,
  cssHeight: number,
  ratio: number,
): Box {
  const scale = ratio > 0 ? ratio : 1;
  const width = Math.max(1, Math.round(cssWidth * scale));
  const height = Math.max(1, Math.round(cssHeight * scale));
  const placed = placeFrame(
    picture.width,
    picture.height,
    picture.rotation,
    width,
    height,
  );
  const sideways = Math.abs(Math.round(placed.angle / (Math.PI / 2))) % 2 === 1;
  const shownWidth = sideways ? placed.height : placed.width;
  const shownHeight = sideways ? placed.width : placed.height;
  return {
    left: (placed.centreX - shownWidth / 2) / scale,
    top: (placed.centreY - shownHeight / 2) / scale,
    width: shownWidth / scale,
    height: shownHeight / scale,
  };
}

/** A canvas point, in CSS pixels, as source display pixels (unrounded). */
export function toSource(
  box: Box,
  frame: Frame,
  x: number,
  y: number,
): { x: number; y: number } {
  return {
    x: box.width > 0 ? ((x - box.left) / box.width) * frame.width : 0,
    y: box.height > 0 ? ((y - box.top) / box.height) * frame.height : 0,
  };
}

/** A rectangle of source display pixels, on the canvas in CSS pixels. */
export function toCanvas(box: Box, frame: Frame, rect: Rect): Box {
  const across = frame.width > 0 ? box.width / frame.width : 0;
  const down = frame.height > 0 ? box.height / frame.height : 0;
  return {
    left: box.left + rect.x * across,
    top: box.top + rect.y * down,
    width: rect.width * across,
    height: rect.height * down,
  };
}

/** What a pointer can take hold of: a corner, an edge, or the inside. */
export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w" | "move";

export const HANDLES: readonly Exclude<Handle, "move">[] = [
  "nw",
  "n",
  "ne",
  "e",
  "se",
  "s",
  "sw",
  "w",
];

/** How far from a handle, in CSS pixels, a pointer still takes it. */
export const REACH = 10;

/** Where handle `handle` of `shown` sits on the canvas. */
export function handlePoint(
  shown: Box,
  handle: Exclude<Handle, "move">,
): { x: number; y: number } {
  const x = handle.includes("w")
    ? shown.left
    : handle.includes("e")
      ? shown.left + shown.width
      : shown.left + shown.width / 2;
  const y = handle.includes("n")
    ? shown.top
    : handle.includes("s")
      ? shown.top + shown.height
      : shown.top + shown.height / 2;
  return { x, y };
}

/** The handle under canvas point `x`, `y`, if any: a corner before an edge,
 * an edge before the inside. */
export function handleAt(shown: Box, x: number, y: number): Handle | null {
  let nearest: { handle: Handle; distance: number } | null = null;
  for (const handle of HANDLES) {
    const point = handlePoint(shown, handle);
    const distance = Math.hypot(point.x - x, point.y - y);
    const corner = handle.length === 2;
    // Corners win a tie with the edges beside them.
    const weighed = corner ? distance - 0.5 : distance;
    if (distance <= REACH && (!nearest || weighed < nearest.distance))
      nearest = { handle, distance: weighed };
  }
  if (nearest) return nearest.handle;
  const inside =
    x >= shown.left &&
    x <= shown.left + shown.width &&
    y >= shown.top &&
    y <= shown.top + shown.height;
  return inside ? "move" : null;
}

/** `value` to the nearest multiple of `step`. */
function snap(value: number, step: number): number {
  return Math.round(value / step) * step;
}

function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), high);
}

/** The smallest side on a grid of `step`: the minimum, rounded up to it. */
function least(frame: Frame, step: number): number {
  return Math.ceil(frame.minSize / step) * step;
}

/** The largest multiple of `step` at or below `value`. */
function floorTo(value: number, step: number): number {
  return Math.floor(value / step) * step;
}

/**
 * `start` with handle `handle` moved by `dx`, `dy` source display pixels,
 * on `frame`'s grid, inside it and at least the minimum size. With `aspect`
 * (width over height) a corner or an edge keeps that shape: a corner is
 * anchored at the opposite one, an edge grows its rectangle about the centre
 * of the other axis.
 */
export function dragRect(
  start: Rect,
  handle: Handle,
  dx: number,
  dy: number,
  frame: Frame,
  aspect: number | null,
): CropRect {
  const { across, down } = frame;
  if (handle === "move") {
    return {
      x: clamp(
        snap(start.x + dx, across),
        0,
        floorTo(frame.width - start.width, across),
      ),
      y: clamp(
        snap(start.y + dy, down),
        0,
        floorTo(frame.height - start.height, down),
      ),
      width: start.width,
      height: start.height,
    };
  }
  let left = start.x;
  let top = start.y;
  let right = start.x + start.width;
  let bottom = start.y + start.height;
  if (handle.includes("w")) left += dx;
  if (handle.includes("e")) right += dx;
  if (handle.includes("n")) top += dy;
  if (handle.includes("s")) bottom += dy;

  if (aspect !== null && aspect > 0) {
    return shaped(start, handle, left, top, right, bottom, frame, aspect);
  }

  const minWidth = least(frame, across);
  const minHeight = least(frame, down);
  if (handle.includes("w"))
    left = clamp(snap(left, across), 0, right - minWidth);
  if (handle.includes("e"))
    right = clamp(
      snap(right, across),
      left + minWidth,
      floorTo(frame.width, across),
    );
  if (handle.includes("n")) top = clamp(snap(top, down), 0, bottom - minHeight);
  if (handle.includes("s"))
    bottom = clamp(
      snap(bottom, down),
      top + minHeight,
      floorTo(frame.height, down),
    );
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** A drag of `handle` that keeps `aspect`: see `dragRect`. */
function shaped(
  start: Rect,
  handle: Exclude<Handle, "move">,
  left: number,
  top: number,
  right: number,
  bottom: number,
  frame: Frame,
  aspect: number,
): CropRect {
  const { across, down } = frame;
  const horizontal = handle.includes("w") || handle.includes("e");
  const vertical = handle.includes("n") || handle.includes("s");
  // The size the pointer asks for, in the axis it leads: a corner follows
  // whichever axis it changed more, relative to the rectangle's size.
  let width = Math.max(right - left, 1);
  let height = Math.max(bottom - top, 1);
  if (horizontal && vertical) {
    const widthChange = Math.abs(Math.log(width / Math.max(start.width, 1)));
    const heightChange = Math.abs(Math.log(height / Math.max(start.height, 1)));
    if (widthChange >= heightChange) height = width / aspect;
    else width = height * aspect;
  } else if (horizontal) {
    height = width / aspect;
  } else {
    width = height * aspect;
  }
  // Where it may grow to: from the anchored side or centre, to the frame.
  const anchorX = handle.includes("w")
    ? start.x + start.width
    : handle.includes("e")
      ? start.x
      : start.x + start.width / 2;
  const anchorY = handle.includes("n")
    ? start.y + start.height
    : handle.includes("s")
      ? start.y
      : start.y + start.height / 2;
  const roomX = handle.includes("w")
    ? anchorX
    : handle.includes("e")
      ? frame.width - anchorX
      : 2 * Math.min(anchorX, frame.width - anchorX);
  const roomY = handle.includes("n")
    ? anchorY
    : handle.includes("s")
      ? frame.height - anchorY
      : 2 * Math.min(anchorY, frame.height - anchorY);
  const fit = Math.min(
    1,
    roomX / Math.max(width, 1),
    roomY / Math.max(height, 1),
  );
  width *= fit;
  height *= fit;
  // At least the minimum, in the shape.
  const minWidth = least(frame, across);
  const minHeight = least(frame, down);
  if (width < minWidth || height < minHeight) {
    const grow = Math.max(minWidth / width, minHeight / height);
    width *= grow;
    height *= grow;
  }
  width = clamp(
    floorTo(width, across),
    least(frame, across),
    floorTo(frame.width, across),
  );
  height = clamp(
    floorTo(height, down),
    least(frame, down),
    floorTo(frame.height, down),
  );
  const x = handle.includes("w")
    ? anchorX - width
    : handle.includes("e")
      ? anchorX
      : anchorX - width / 2;
  const y = handle.includes("n")
    ? anchorY - height
    : handle.includes("s")
      ? anchorY
      : anchorY - height / 2;
  return {
    x: clamp(snap(x, across), 0, floorTo(frame.width - width, across)),
    y: clamp(snap(y, down), 0, floorTo(frame.height - height, down)),
    width,
    height,
  };
}

/** `rect` moved by `steps` of the grid each way, kept inside the picture:
 * what an arrow key does. */
export function nudge(
  rect: Rect,
  frame: Frame,
  stepsX: number,
  stepsY: number,
): CropRect {
  return dragRect(
    rect,
    "move",
    stepsX * frame.across,
    stepsY * frame.down,
    frame,
    null,
  );
}

/** The shape a drag keeps: the preset the rectangle is on, or with Shift
 * held, whatever shape it has; otherwise none. */
export function keptAspect(
  rect: Rect,
  frame: Frame,
  shift: boolean,
): number | null {
  if (shift) return rect.height > 0 ? rect.width / rect.height : null;
  // The whole picture is where a crop starts, not a shape it was given —
  // even when a preset (16:9 of a 16:9 source) happens to be all of it.
  if (rect.width === frame.width && rect.height === frame.height) return null;
  const preset = frame.presets.find(
    (candidate) =>
      candidate.aspect !== "source" &&
      candidate.rect.width === rect.width &&
      candidate.rect.height === rect.height,
  );
  return preset && preset.rect.height > 0
    ? preset.rect.width / preset.rect.height
    : null;
}
