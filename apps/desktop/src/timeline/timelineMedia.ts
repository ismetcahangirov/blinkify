import type {
  Filmstrip,
  WaveformStatus,
  WaveformUpdate,
} from "@blinkify/types";
import type { MediaSource, Tile } from "./draw.js";

/**
 * The timeline's pictures and sound (#33): filmstrip sprite sheets (#26) and
 * waveform peaks (#25), loaded from the engine on demand and handed to the
 * painter synchronously.
 *
 * The painter asks for what it is about to draw. What is loaded is returned
 * at once; what is not is requested, `null` is returned — the clip draws
 * without it — and `onChange` fires when it arrives, which marks the content
 * layer dirty. The painter never waits.
 *
 * Density follows the zoom, in the engine's units: the filmstrip is asked for
 * at the zoom's pixels per second and the engine picks the thumbnail interval
 * from its power-of-two ladder; peaks are asked for at the zoom and the
 * engine reads the pyramid level for it. Both are cached here per source and
 * density, and a density still loading draws from the nearest one loaded.
 */

/** Waveform columns fetched per request: a screen is a handful of these. */
export const PEAK_CHUNK = 512;

/** Chunks kept, least recently used dropped first. */
const PEAK_CHUNKS_KEPT = 4096;

/** The height thumbnails are generated at, in device pixels. */
export const THUMBNAIL_HEIGHT = 96;

export interface MediaBackend {
  invoke: <T>(command: string, args: Record<string, unknown>) => Promise<T>;
  loadImage: (url: string) => Promise<CanvasImageSource>;
  /** The URL a sheet file is served at. */
  sheetUrl: (path: string) => string;
}

type Loaded<T> = { state: "loading" } | { state: "ready"; value: T };

/** The power-of-two bucket of a zoom: zooming within one reuses a strip. */
function densityBucket(pixelsPerSecond: number): number {
  return Math.floor(Math.log2(Math.max(pixelsPerSecond, 1e-6)));
}

export class TimelineMedia implements MediaSource {
  private paths = new Map<number, string>();
  private readonly strips = new Map<string, Loaded<Filmstrip>>();
  private readonly sheets = new Map<string, Loaded<CanvasImageSource>>();
  private readonly waveforms = new Map<string, WaveformStatus["state"]>();
  private readonly chunks = new Map<string, Loaded<Int16Array>>();
  private disposed = false;

  constructor(
    private readonly backend: MediaBackend,
    private readonly onChange: () => void,
  ) {}

  /** The project's sources, by id: their paths. */
  setSources(paths: ReadonlyMap<number, string>): void {
    this.paths = new Map(paths);
  }

  dispose(): void {
    this.disposed = true;
  }

  private changed(): void {
    if (!this.disposed) this.onChange();
  }

  tile(source: number, seconds: number, pixelsPerSecond: number): Tile | null {
    const path = this.paths.get(source);
    if (path === undefined) return null;
    const bucket = densityBucket(pixelsPerSecond);
    const key = `${path}|${bucket}`;
    if (!this.strips.has(key)) this.requestStrip(key, path, pixelsPerSecond);
    const strip = this.nearestStrip(path, bucket);
    if (!strip) return null;
    const perSheet = strip.columns * strip.rows;
    if (strip.count === 0 || perSheet === 0) return null;
    const index = Math.min(
      strip.count - 1,
      Math.max(0, Math.round(seconds / strip.interval)),
    );
    const sheetPath = strip.sheets[Math.floor(index / perSheet)];
    if (sheetPath === undefined) return null;
    const image = this.sheet(sheetPath);
    if (!image) return null;
    const within = index % perSheet;
    return {
      image,
      sx: (within % strip.columns) * strip.tileWidth,
      sy: Math.floor(within / strip.columns) * strip.tileHeight,
      sw: strip.tileWidth,
      sh: strip.tileHeight,
    };
  }

  private nearestStrip(path: string, bucket: number): Filmstrip | null {
    let best: Filmstrip | null = null;
    let distance = Infinity;
    for (const [key, loaded] of this.strips) {
      if (loaded.state !== "ready" || !key.startsWith(`${path}|`)) continue;
      const other = Number(key.slice(path.length + 1));
      if (Math.abs(other - bucket) < distance) {
        distance = Math.abs(other - bucket);
        best = loaded.value;
      }
    }
    return best;
  }

  private requestStrip(key: string, path: string, pixelsPerSecond: number) {
    this.strips.set(key, { state: "loading" });
    this.backend
      .invoke<Filmstrip>("generate_filmstrip", {
        path,
        height: THUMBNAIL_HEIGHT,
        pixelsPerSecond,
      })
      .then((strip) => {
        this.strips.set(key, { state: "ready", value: strip });
        this.changed();
      })
      // No pictures (an audio-only file, an unreadable one): the clip draws
      // without thumbnails, and asking again would fail the same way.
      .catch(() => undefined);
  }

  private sheet(path: string): CanvasImageSource | null {
    const known = this.sheets.get(path);
    if (known?.state === "ready") return known.value;
    if (!known) {
      this.sheets.set(path, { state: "loading" });
      this.backend
        .loadImage(this.backend.sheetUrl(path))
        .then((image) => {
          this.sheets.set(path, { state: "ready", value: image });
          this.changed();
        })
        .catch(() => this.sheets.delete(path));
    }
    return null;
  }

  /** A waveform the engine reported on. */
  waveformUpdate(update: WaveformUpdate): void {
    const key = `${update.path}|${update.stream}`;
    const was = this.waveforms.get(key);
    this.waveforms.set(key, update.status.state);
    if (update.status.state === "ready" && was !== "ready") this.changed();
  }

  peaks(
    source: number,
    stream: number,
    seconds: number,
    pixels: number,
    pixelsPerSecond: number,
  ): Int16Array | null {
    const path = this.paths.get(source);
    if (path === undefined || pixels <= 0) return null;
    const waveform = `${path}|${stream}`;
    const state = this.waveforms.get(waveform);
    if (state === undefined) {
      this.waveforms.set(waveform, "pending");
      this.backend
        .invoke<WaveformStatus>("generate_waveform", { path, stream })
        .then((status) => this.waveformUpdate({ path, stream, status }))
        .catch(() => undefined);
      return null;
    }
    if (state !== "ready") return null;

    const out = new Int16Array(pixels * 2);
    const firstColumn = Math.floor(seconds * pixelsPerSecond);
    let any = false;
    for (let i = 0; i < pixels;) {
      const column = firstColumn + i;
      const chunk = Math.floor(column / PEAK_CHUNK);
      const offset = column - chunk * PEAK_CHUNK;
      const take = Math.min(PEAK_CHUNK - offset, pixels - i);
      const data = this.chunk(path, stream, pixelsPerSecond, chunk);
      if (data) {
        out.set(data.subarray(offset * 2, (offset + take) * 2), i * 2);
        any = true;
      }
      i += take;
    }
    return any ? out : null;
  }

  private chunk(
    path: string,
    stream: number,
    pixelsPerSecond: number,
    index: number,
  ): Int16Array | null {
    const key = `${path}|${stream}|${pixelsPerSecond}|${index}`;
    const known = this.chunks.get(key);
    if (known) {
      // Least recently used goes first: re-insert on use.
      this.chunks.delete(key);
      this.chunks.set(key, known);
      return known.state === "ready" ? known.value : null;
    }
    if (index < 0) return null;
    this.chunks.set(key, { state: "loading" });
    while (this.chunks.size > PEAK_CHUNKS_KEPT) {
      const oldest = this.chunks.keys().next().value;
      if (oldest === undefined) break;
      this.chunks.delete(oldest);
    }
    this.backend
      .invoke<ArrayBuffer>("waveform_peaks", {
        path,
        stream,
        pixelsPerSecond,
        startSeconds: (index * PEAK_CHUNK) / pixelsPerSecond,
        pixels: PEAK_CHUNK,
      })
      .then((bytes) => {
        const pairs = new Int16Array(PEAK_CHUNK * 2);
        pairs.set(
          new Int16Array(
            bytes,
            0,
            Math.min(bytes.byteLength >> 1, pairs.length),
          ),
        );
        this.chunks.set(key, { state: "ready", value: pairs });
        this.changed();
      })
      .catch(() => this.chunks.delete(key));
    return null;
  }
}
