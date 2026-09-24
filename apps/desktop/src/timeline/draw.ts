import type {
  Placement as EvaluatedPlacement,
  Rational,
} from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import {
  crisp,
  rulerStep,
  snapToDevice,
  visibleFrames,
  xOf,
  type Viewport,
} from "./viewport.js";

/**
 * Painting the timeline (#33). Pure functions of a `Scene` onto a 2D
 * context, so what is drawn can be asserted without a browser.
 *
 * Everything here is virtualised: only the frames in view and the tracks in
 * view are visited. Clips on a track are in timeline order (the evaluator
 * sorts them), so the first visible one is found by binary search and the
 * walk stops at the first one past the right edge — the cost of a frame is
 * the clips on screen, not the clips in the project.
 */

/** A placement as the timeline receives it: the evaluator's, read-only. */
export type Placement = DeepReadonly<EvaluatedPlacement>;

/** The subset of `CanvasRenderingContext2D` the timeline paints with. */
export interface Painter {
  fillStyle: string | CanvasGradient | CanvasPattern;
  strokeStyle: string | CanvasGradient | CanvasPattern;
  lineWidth: number;
  font: string;
  textBaseline: CanvasTextBaseline;
  globalAlpha: number;
  setTransform(
    a: number,
    b: number,
    c: number,
    d: number,
    e: number,
    f: number,
  ): void;
  clearRect(x: number, y: number, w: number, h: number): void;
  fillRect(x: number, y: number, w: number, h: number): void;
  strokeRect(x: number, y: number, w: number, h: number): void;
  beginPath(): void;
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  closePath(): void;
  fill(): void;
  stroke(): void;
  rect(x: number, y: number, w: number, h: number): void;
  clip(): void;
  save(): void;
  restore(): void;
  fillText(text: string, x: number, y: number): void;
  measureText(text: string): { width: number };
  drawImage(
    image: CanvasImageSource,
    sx: number,
    sy: number,
    sw: number,
    sh: number,
    dx: number,
    dy: number,
    dw: number,
    dh: number,
  ): void;
}

/** The colours the timeline paints with, read once from the design tokens. */
export interface Theme {
  background: string;
  track: string;
  border: string;
  clipVideo: string;
  clipVideoBorder: string;
  clipAudio: string;
  clipAudioBorder: string;
  clipLabel: string;
  selected: string;
  playhead: string;
  rulerText: string;
  warning: string;
  font: string;
}

/** A track as laid out: where its row is, and what is on it. */
export interface TrackRow {
  readonly id: number;
  readonly kind: "video" | "audio";
  /** From the top of the track stack, before scrolling, in CSS pixels. */
  readonly top: number;
  readonly height: number;
  readonly placements: readonly Placement[];
}

/** One thumbnail, as a region of a loaded sprite sheet. */
export interface Tile {
  image: CanvasImageSource;
  sx: number;
  sy: number;
  sw: number;
  sh: number;
}

/**
 * Where a clip's pictures and sound come from. Both answer synchronously
 * from what is already loaded, and return `null` — a placeholder is drawn —
 * for what is not; loading it is theirs, and ends in a redraw.
 */
export interface MediaSource {
  /** The thumbnail of `source` nearest `seconds`, at `pixelsPerSecond`. */
  tile(source: number, seconds: number, pixelsPerSecond: number): Tile | null;
  /**
   * Peaks for `pixels` columns of `source`'s `stream` from `seconds`, at
   * `pixelsPerSecond`: `(min, max)` pairs, full scale 32767.
   */
  peaks(
    source: number,
    stream: number,
    seconds: number,
    pixels: number,
    pixelsPerSecond: number,
  ): Int16Array | null;
}

export interface Scene {
  readonly view: Viewport;
  readonly dpr: number;
  readonly rulerHeight: number;
  readonly rows: readonly TrackRow[];
  readonly frameRate: Rational;
  readonly selection: ReadonlySet<number>;
  /** Clips the engine says cannot be stream-copied (#57): marked. */
  readonly ineligible: ReadonlySet<number>;
  /** A label per source id: its file name. */
  readonly labels: ReadonlyMap<number, string>;
  readonly theme: Theme;
  readonly media: MediaSource | null;
  readonly text: TextCache;
}

/**
 * Text is the expensive part of a canvas frame: measuring, and cutting a
 * label to fit. Both are cached here, keyed by the text and the width it
 * must fit, and the cache is dropped when the zoom changes — the only time
 * the fitting widths change for most labels.
 */
export class TextCache {
  private readonly fitted = new Map<string, string>();
  private scale = Number.NaN;
  measured = 0;

  /** Forget everything if the zoom changed. */
  forZoom(scale: number): this {
    if (scale !== this.scale) {
      this.fitted.clear();
      this.scale = scale;
    }
    return this;
  }

  /** `text`, cut with an ellipsis to fit `width` — or empty if nothing fits. */
  fit(painter: Painter, text: string, width: number): string {
    const bucket = Math.floor(width / 8);
    const key = `${bucket}\u0000${text}`;
    const known = this.fitted.get(key);
    if (known !== undefined) return known;
    const room = bucket * 8;
    const measure = (value: string) => {
      this.measured += 1;
      return painter.measureText(value).width;
    };
    let result: string;
    if (measure(text) <= room) result = text;
    else {
      let low = 0;
      let high = text.length;
      while (low < high) {
        const middle = Math.ceil((low + high) / 2);
        if (measure(`${text.slice(0, middle)}…`) <= room) low = middle;
        else high = middle - 1;
      }
      result = low > 0 ? `${text.slice(0, low)}…` : "";
    }
    this.fitted.set(key, result);
    return result;
  }
}

/** The index of the first placement that ends after `frame`. */
export function firstVisible(
  placements: readonly Placement[],
  frame: number,
): number {
  let low = 0;
  let high = placements.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    const placement = placements[middle];
    if (placement && placement.start + placement.length <= frame)
      low = middle + 1;
    else high = middle;
  }
  return low;
}

/** The placements of a row that intersect `[first, last)` frames. */
export function placementsIn(
  placements: readonly Placement[],
  first: number,
  last: number,
): Placement[] {
  const found: Placement[] = [];
  for (let i = firstVisible(placements, first); i < placements.length; i++) {
    const placement = placements[i];
    if (!placement || placement.start >= last) break;
    found.push(placement);
  }
  return found;
}

/** `frame` as `m:ss` or `h:mm:ss`, with `:ff` frames when `frames`. */
export function rulerLabel(
  frame: number,
  fps: number,
  frames: boolean,
): string {
  const perSecond = Math.max(1, Math.round(fps));
  const seconds = Math.floor(frame / perSecond);
  const rest = frame - seconds * perSecond;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  const clock = h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
  return frames ? `${clock}:${pad(rest)}` : clock;
}

function fps(rate: Rational): number {
  return rate.den > 0 ? rate.num / rate.den : 30;
}

/** Draw the content layer: ruler, track rows, clips. */
export function drawContent(painter: Painter, scene: Scene): void {
  const { view, dpr, theme } = scene;
  painter.setTransform(dpr, 0, 0, dpr, 0, 0);
  painter.clearRect(0, 0, view.width, view.height);
  painter.fillStyle = theme.background;
  painter.fillRect(0, 0, view.width, view.height);
  scene.text.forZoom(view.scale);
  drawRows(painter, scene);
  drawRuler(painter, scene);
}

function drawRuler(painter: Painter, scene: Scene): void {
  const { view, dpr, theme, rulerHeight } = scene;
  const rate = fps(scene.frameRate);
  painter.fillStyle = theme.track;
  painter.fillRect(0, 0, view.width, rulerHeight);
  const step = rulerStep(view.scale, rate);
  const minor = Math.max(1, step / 5);
  const [first, last] = visibleFrames(view);
  const showFrames = step < Math.max(1, Math.round(rate));
  painter.strokeStyle = theme.border;
  painter.lineWidth = 1 / dpr;
  painter.beginPath();
  const bottom = crisp(rulerHeight - 1, dpr);
  painter.moveTo(0, bottom);
  painter.lineTo(view.width, bottom);
  for (
    let frame = Math.floor(first / minor) * minor;
    frame <= last;
    frame += minor
  ) {
    const x = crisp(xOf(view, frame), dpr);
    const major = Math.round(frame) % step === 0;
    painter.moveTo(x, rulerHeight - (major ? 10 : 5));
    painter.lineTo(x, rulerHeight - 1);
  }
  painter.stroke();
  painter.fillStyle = theme.rulerText;
  painter.font = theme.font;
  painter.textBaseline = "top";
  for (
    let frame = Math.floor(first / step) * step;
    frame <= last;
    frame += step
  ) {
    painter.fillText(
      rulerLabel(frame, rate, showFrames),
      xOf(view, frame) + 3,
      3,
    );
  }
}

function drawRows(painter: Painter, scene: Scene): void {
  const { view, dpr, theme, rulerHeight } = scene;
  const [first, last] = visibleFrames(view);
  for (const row of scene.rows) {
    const top = rulerHeight + row.top - view.scrollTop;
    if (top + row.height <= rulerHeight || top >= view.height) continue;
    painter.save();
    painter.beginPath();
    painter.rect(0, rulerHeight, view.width, view.height - rulerHeight);
    painter.clip();
    painter.fillStyle = theme.track;
    painter.fillRect(0, top + 1, view.width, row.height - 2);
    for (const placement of placementsIn(row.placements, first, last)) {
      drawClip(painter, scene, row, placement, top);
    }
    painter.strokeStyle = theme.border;
    painter.lineWidth = 1 / dpr;
    painter.beginPath();
    const line = crisp(top + row.height - 1, dpr);
    painter.moveTo(0, line);
    painter.lineTo(view.width, line);
    painter.stroke();
    painter.restore();
  }
}

function drawClip(
  painter: Painter,
  scene: Scene,
  row: TrackRow,
  placement: Placement,
  rowTop: number,
): void {
  const { view, dpr, theme } = scene;
  // Clamped to a little beyond the view: a clip hours long at close zoom is
  // millions of pixels wide, and canvas coordinates that large lose
  // precision.
  const left = Math.max(-2, snapToDevice(xOf(view, placement.start), dpr));
  const right = Math.min(
    view.width + 2,
    snapToDevice(xOf(view, placement.start + placement.length), dpr),
  );
  const width = Math.max(1 / dpr, right - left);
  const top = rowTop + 2;
  const height = row.height - 4;
  const video = row.kind === "video";
  painter.fillStyle = video ? theme.clipVideo : theme.clipAudio;
  painter.fillRect(left, top, width, height);

  if (width > 4) {
    painter.save();
    painter.beginPath();
    painter.rect(left, top, width, height);
    painter.clip();
    if (video)
      drawThumbnails(painter, scene, placement, left, top, width, height);
    else drawWaveform(painter, scene, placement, left, top, width, height);
    const name = scene.labels.get(placement.source) ?? "";
    const label = scene.text.fit(
      painter,
      placement.motion === "hold"
        ? `Freeze · ${name}`
        : placement.motion === "reverse"
          ? `Reverse · ${name}`
          : name,
      width - 8,
    );
    if (label) {
      painter.fillStyle = theme.clipLabel;
      painter.font = theme.font;
      painter.textBaseline = "top";
      painter.fillText(label, left + 4, top + 3);
    }
    painter.restore();
  }

  const selected = scene.selection.has(placement.clip);
  const ineligible = scene.ineligible.has(placement.clip);
  painter.strokeStyle = selected
    ? theme.selected
    : video
      ? theme.clipVideoBorder
      : theme.clipAudioBorder;
  painter.lineWidth = (selected ? 2 : 1) / dpr;
  const inset = painter.lineWidth / 2;
  painter.strokeRect(
    left + inset,
    top + inset,
    width - 2 * inset,
    height - 2 * inset,
  );
  if (placement.forced) {
    // Re-encoded at export whatever else holds (#35): a band along the top.
    painter.fillStyle = theme.warning;
    painter.fillRect(left, top, width, 3);
  }
  if (ineligible) {
    // The copy-ineligible mark (#57): a band along the bottom edge, so it
    // reads at every zoom without covering the pictures.
    painter.fillStyle = theme.warning;
    painter.fillRect(left, top + height - 3, width, 3);
  }
}

/** Seconds of source time per sequence frame of a placement. */
function sourceSecondsPerFrame(placement: Placement): number {
  const sequence = placement.sequenceTimeBase;
  const speed = placement.speed;
  return (sequence.num / sequence.den) * (speed.num / speed.den);
}

function sourceSeconds(placement: Placement, ticks: number): number {
  return (ticks * placement.timeBase.num) / placement.timeBase.den;
}

function drawThumbnails(
  painter: Painter,
  scene: Scene,
  placement: Placement,
  left: number,
  top: number,
  width: number,
  height: number,
): void {
  const { view } = scene;
  if (!scene.media) return;
  const perFrame = sourceSecondsPerFrame(placement);
  const pixelsPerSecond = view.scale / perFrame;
  const clipX = xOf(view, placement.start);
  const firstTile = scene.media.tile(
    placement.source,
    sourceSeconds(placement, placement.sourceIn),
    pixelsPerSecond,
  );
  if (!firstTile) return;
  const tileWidth = (firstTile.sw / firstTile.sh) * height;
  const from = Math.max(left, clipX);
  const start = clipX + Math.floor((from - clipX) / tileWidth) * tileWidth;
  for (let x = start; x < left + width; x += tileWidth) {
    const seconds =
      sourceSeconds(placement, placement.sourceIn) +
      ((x - clipX) / view.scale) * perFrame;
    const tile = scene.media.tile(placement.source, seconds, pixelsPerSecond);
    if (!tile) continue;
    painter.drawImage(
      tile.image,
      tile.sx,
      tile.sy,
      tile.sw,
      tile.sh,
      x,
      top,
      tileWidth,
      height,
    );
  }
}

function drawWaveform(
  painter: Painter,
  scene: Scene,
  placement: Placement,
  left: number,
  top: number,
  width: number,
  height: number,
): void {
  const { view, dpr, theme } = scene;
  if (!scene.media) return;
  const perFrame = sourceSecondsPerFrame(placement);
  const pixelsPerSecond = view.scale / perFrame;
  const clipX = xOf(view, placement.start);
  const from = Math.max(left, clipX);
  const seconds =
    sourceSeconds(placement, placement.sourceIn) +
    ((from - clipX) / view.scale) * perFrame;
  const pixels = Math.ceil(left + width - from);
  const peaks = scene.media.peaks(
    placement.source,
    placement.stream,
    seconds,
    pixels,
    pixelsPerSecond,
  );
  if (!peaks) return;
  const middle = top + height / 2;
  const half = height / 2 - 2;
  painter.strokeStyle = theme.clipAudioBorder;
  painter.lineWidth = 1 / dpr;
  painter.beginPath();
  for (let i = 0; i < pixels && 2 * i + 1 < peaks.length; i++) {
    const min = (peaks[2 * i] ?? 0) / 32767;
    const max = (peaks[2 * i + 1] ?? 0) / 32767;
    const x = crisp(from + i, dpr);
    painter.moveTo(x, middle - max * half);
    painter.lineTo(x, middle - min * half + 1 / dpr);
  }
  painter.stroke();
}

/** A drag in progress, as the overlay shows it (#34). */
export interface DragPreview {
  readonly ghosts: readonly {
    clip: number;
    row: number;
    start: number;
    length: number;
  }[];
  readonly snapped: number | null;
  readonly clamped: boolean;
  readonly invalid: string | null;
}

/**
 * Draw the overlay layer: the playhead and, while a clip is dragged, where it
 * would land — its ghost, the line it snapped to, and a warning edge where a
 * bound stopped it. The clips themselves are not redrawn during a drag.
 */
export function drawOverlay(
  painter: Painter,
  scene: Pick<Scene, "view" | "dpr" | "theme" | "rulerHeight"> & {
    readonly rows?: readonly TrackRow[];
  },
  playhead: number | null,
  drag: DragPreview | null = null,
): void {
  const { view, dpr, theme, rulerHeight } = scene;
  painter.setTransform(dpr, 0, 0, dpr, 0, 0);
  painter.clearRect(0, 0, view.width, view.height);
  if (drag) drawDrag(painter, scene, drag);
  if (playhead === null) return;
  const x = crisp(xOf(view, playhead), dpr);
  if (x < -6 || x > view.width + 6) return;
  painter.strokeStyle = theme.playhead;
  painter.lineWidth = 1 / dpr;
  painter.beginPath();
  painter.moveTo(x, 0);
  painter.lineTo(x, view.height);
  painter.stroke();
  painter.fillStyle = theme.playhead;
  painter.beginPath();
  painter.moveTo(x - 5, 0);
  painter.lineTo(x + 5, 0);
  painter.lineTo(x + 5, rulerHeight - 10);
  painter.lineTo(x, rulerHeight - 5);
  painter.lineTo(x - 5, rulerHeight - 10);
  painter.closePath();
  painter.fill();
}

function drawDrag(
  painter: Painter,
  scene: Pick<Scene, "view" | "dpr" | "theme" | "rulerHeight"> & {
    readonly rows?: readonly TrackRow[];
  },
  drag: DragPreview,
): void {
  const { view, dpr, theme, rulerHeight } = scene;
  const colour = drag.invalid ? theme.warning : theme.selected;
  for (const ghost of drag.ghosts) {
    const row = scene.rows?.find((r) => r.id === ghost.row);
    if (!row) continue;
    const top = rulerHeight + row.top - view.scrollTop + 2;
    const left = snapToDevice(xOf(view, ghost.start), dpr);
    const right = snapToDevice(xOf(view, ghost.start + ghost.length), dpr);
    painter.globalAlpha = 0.25;
    painter.fillStyle = colour;
    painter.fillRect(
      left,
      top,
      Math.max(1 / dpr, right - left),
      row.height - 4,
    );
    painter.globalAlpha = 1;
    painter.strokeStyle = colour;
    painter.lineWidth = 2 / dpr;
    painter.strokeRect(
      left + 1 / dpr,
      top + 1 / dpr,
      right - left - 2 / dpr,
      row.height - 6,
    );
    if (drag.clamped) {
      // The bound that stopped the drag: a solid bar on both ends, so it
      // reads whichever edge hit it.
      painter.fillStyle = theme.warning;
      painter.fillRect(left, top, 3, row.height - 4);
      painter.fillRect(right - 3, top, 3, row.height - 4);
    }
  }
  if (drag.snapped !== null) {
    const x = crisp(xOf(view, drag.snapped), dpr);
    painter.strokeStyle = theme.selected;
    painter.lineWidth = 1 / dpr;
    painter.beginPath();
    painter.moveTo(x, rulerHeight);
    painter.lineTo(x, view.height);
    painter.stroke();
  }
}
