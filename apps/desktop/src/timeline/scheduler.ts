/**
 * Redraw scheduling for the timeline canvas (#33).
 *
 * The timeline is two canvases stacked: **content** — ruler, tracks, clips,
 * waveforms, thumbnails — and **overlay** — the playhead and what an
 * interaction draws over the clips. A state change does not draw: it marks a
 * layer dirty, and the dirty layers are drawn once, together, on the next
 * animation frame. A hundred changes between two frames cost one draw.
 *
 * The playhead moves every frame during playback and dirties only the
 * overlay, so playing never redraws a clip.
 */

export type Layer = "content" | "overlay";

export interface FrameSource {
  request: (callback: () => void) => number;
  cancel: (handle: number) => void;
}

const browserFrames: FrameSource = {
  request: (callback) => requestAnimationFrame(callback),
  cancel: (handle) => cancelAnimationFrame(handle),
};

export class RedrawScheduler {
  private readonly dirty = new Set<Layer>();
  private handle: number | null = null;
  /** How many times each layer has been drawn — what the tests assert. */
  readonly draws: Record<Layer, number> = { content: 0, overlay: 0 };

  constructor(
    private readonly draw: (layers: ReadonlySet<Layer>) => void,
    private readonly frames: FrameSource = browserFrames,
  ) {}

  /** Mark `layer` for the next frame. */
  invalidate(layer: Layer): void {
    this.dirty.add(layer);
    if (this.handle === null) {
      this.handle = this.frames.request(() => this.flush());
    }
  }

  /** Draw what is dirty now. Called by the frame; tests may call it. */
  flush(): void {
    this.handle = null;
    if (this.dirty.size === 0) return;
    const layers = new Set(this.dirty);
    this.dirty.clear();
    for (const layer of layers) this.draws[layer] += 1;
    this.draw(layers);
  }

  dispose(): void {
    if (this.handle !== null) this.frames.cancel(this.handle);
    this.handle = null;
    this.dirty.clear();
  }
}
