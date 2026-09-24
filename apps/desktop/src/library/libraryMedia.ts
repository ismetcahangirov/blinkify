import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { Tile } from "../timeline/draw.js";
import { TimelineMedia } from "../timeline/timelineMedia.js";

/**
 * The library's thumbnails and background preparation (#53).
 *
 * The same sprite-sheet machinery as the timeline's filmstrip (#26) — the
 * library asks the engine for a filmstrip exactly as the timeline does, and
 * the engine's content-keyed cache means the timeline finds it ready. There
 * is no second thumbnail path.
 *
 * Asking is also what starts the work: an imported asset's card asks for its
 * thumbnail and its waveform, and the engine generates both at background
 * priority (#22) while the card says it is preparing.
 */

/** A low density: one picture is all a card shows. */
const CARD_PIXELS_PER_SECOND = 1;

const listeners = new Set<() => void>();

function loadImage(url: string): Promise<CanvasImageSource> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`could not load ${url}`));
    image.src = url;
  });
}

let media: TimelineMedia | null = null;

function shared(): TimelineMedia {
  media ??= new TimelineMedia(
    {
      invoke: (command, args) => invoke(command, args),
      loadImage,
      sheetUrl: (path) =>
        `${convertFileSrc("", "frame")}sheet/${encodeURIComponent(path)}`,
    },
    () => listeners.forEach((listener) => listener()),
  );
  return media;
}

/** Call `listener` whenever a thumbnail or waveform arrives. */
export function onLibraryMedia(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function setLibrarySources(paths: ReadonlyMap<number, string>): void {
  shared().setSources(paths);
}

/** The card picture of `source`, a second in — or `null` while preparing. */
export function cardTile(source: number, seconds: number): Tile | null {
  return shared().tile(source, seconds, CARD_PIXELS_PER_SECOND);
}

/** Whether `source`'s waveform is ready; asking starts it. */
export function waveformReady(source: number, stream: number): boolean {
  return shared().peaks(source, stream, 0, 1, CARD_PIXELS_PER_SECOND) !== null;
}
