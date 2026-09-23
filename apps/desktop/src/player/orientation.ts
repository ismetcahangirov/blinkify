/**
 * Where a decoded frame goes on the preview canvas.
 *
 * Frames arrive as coded — a portrait phone video arrives landscape — and are
 * turned upright here, at draw time, from the rotation the probe read out of
 * the display matrix (#27). The rule is the one FFmpeg's own autorotation
 * applies: rotate **counter-clockwise** by `rotation` degrees. The engine test
 * `a_portrait_video_arrives_as_coded_and_turns_upright_by_its_rotation`
 * proves that rule against FFmpeg pixel for pixel; the tests beside this file
 * prove the canvas transform below implements it.
 *
 * A rotation on the GPU costs nothing. A sideways portrait video is the most
 * visible bug a preview can have.
 */

export interface Placement {
  /** The canvas point the frame's centre is drawn at. */
  centreX: number;
  centreY: number;
  /** The canvas rotation to apply, in radians. Canvas angles run clockwise
   * because the y axis points down, so a counter-clockwise turn is negative. */
  angle: number;
  /** The unrotated frame's size on the canvas. */
  width: number;
  height: number;
}

/**
 * Fit a `frameWidth` by `frameHeight` frame, turned counter-clockwise by
 * `rotation` degrees, centred in a `canvasWidth` by `canvasHeight` canvas,
 * as large as it goes without cropping.
 */
export function placeFrame(
  frameWidth: number,
  frameHeight: number,
  rotation: number,
  canvasWidth: number,
  canvasHeight: number,
): Placement {
  const quarterTurns = (((Math.round(rotation / 90) % 4) + 4) % 4) as
    0 | 1 | 2 | 3;
  const sideways = quarterTurns % 2 === 1;
  const shownWidth = sideways ? frameHeight : frameWidth;
  const shownHeight = sideways ? frameWidth : frameHeight;
  const scale =
    shownWidth > 0 && shownHeight > 0
      ? Math.min(canvasWidth / shownWidth, canvasHeight / shownHeight)
      : 0;
  return {
    centreX: canvasWidth / 2,
    centreY: canvasHeight / 2,
    angle: -(quarterTurns * Math.PI) / 2,
    width: frameWidth * scale,
    height: frameHeight * scale,
  };
}

/**
 * Where a point of the frame — in frame pixels, origin top-left — lands on
 * the canvas under `placement`. What `drawFrame` does, as arithmetic.
 */
export function canvasPoint(
  placement: Placement,
  frameWidth: number,
  frameHeight: number,
  x: number,
  y: number,
): { x: number; y: number } {
  const scaleX = frameWidth > 0 ? placement.width / frameWidth : 0;
  const scaleY = frameHeight > 0 ? placement.height / frameHeight : 0;
  const localX = (x - frameWidth / 2) * scaleX;
  const localY = (y - frameHeight / 2) * scaleY;
  const cos = Math.cos(placement.angle);
  const sin = Math.sin(placement.angle);
  return {
    x: placement.centreX + localX * cos - localY * sin,
    y: placement.centreY + localX * sin + localY * cos,
  };
}

/**
 * Draw `source` — the frame at its own size — onto `context` under
 * `placement`, clearing whatever was there.
 */
export function drawFrame(
  context: CanvasRenderingContext2D,
  source: CanvasImageSource,
  placement: Placement,
): void {
  const { canvas } = context;
  context.setTransform(1, 0, 0, 1, 0, 0);
  context.clearRect(0, 0, canvas.width, canvas.height);
  context.translate(placement.centreX, placement.centreY);
  context.rotate(placement.angle);
  context.drawImage(
    source,
    -placement.width / 2,
    -placement.height / 2,
    placement.width,
    placement.height,
  );
  context.setTransform(1, 0, 0, 1, 0, 0);
}
