import type { Filmstrip } from "@blinkify/types";
import { describe, expect, it, vi } from "vitest";
import {
  PEAK_CHUNK,
  TimelineMedia,
  type MediaBackend,
} from "./timelineMedia.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

const STRIP: Filmstrip = {
  version: 1,
  interval: 2,
  tileWidth: 160,
  tileHeight: 90,
  columns: 10,
  rows: 10,
  count: 150,
  sheets: ["C:\\cache\\s0.jpg", "C:\\cache\\s1.jpg"],
};

function backend(overrides: Partial<MediaBackend> = {}) {
  const invoke = vi.fn(
    (command: string, args: Record<string, unknown>): Promise<unknown> => {
      if (command === "generate_filmstrip") return Promise.resolve(STRIP);
      if (command === "generate_waveform")
        return Promise.resolve({ state: "ready" });
      if (command === "waveform_peaks") {
        const start = args.startSeconds as number;
        const pairs = new Int16Array(PEAK_CHUNK * 2).map(
          (_, i) => Math.round(start) + (i >> 1),
        );
        return Promise.resolve(pairs.buffer);
      }
      return Promise.reject(new Error(command));
    },
  );
  const loadImage = vi.fn((url: string) =>
    Promise.resolve({ url } as unknown as CanvasImageSource),
  );
  return {
    invoke: invoke as unknown as MediaBackend["invoke"],
    loadImage,
    sheetUrl: (path: string) => `sheet:${path}`,
    calls: invoke,
    ...overrides,
  };
}

describe("the timeline's media", () => {
  it("finds a thumbnail in its sheet once the strip and sheet have loaded", async () => {
    const api = backend();
    const changed = vi.fn();
    const media = new TimelineMedia(api, changed);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    expect(media.tile(1, 0, 50)).toBeNull();
    await flush();
    expect(api.calls).toHaveBeenCalledWith("generate_filmstrip", {
      path: "C:\\a.mp4",
      height: 96,
      pixelsPerSecond: 50,
    });
    // 230 s at 2 s a thumbnail is thumbnail 115: sheet 1, tile 15. The
    // strip is in; that sheet is loading.
    expect(media.tile(1, 230, 50)).toBeNull();
    await flush();
    expect(changed).toHaveBeenCalled();
    const tile = media.tile(1, 230, 50);
    expect(tile).toMatchObject({ sx: 5 * 160, sy: 1 * 90, sw: 160, sh: 90 });
    expect(api.loadImage).toHaveBeenLastCalledWith("sheet:C:\\cache\\s1.jpg");
  });

  it("draws from the nearest density while another zoom's strip loads", async () => {
    const api = backend();
    const media = new TimelineMedia(api, () => undefined);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    media.tile(1, 0, 50);
    await flush();
    media.tile(1, 0, 50);
    await flush();
    api.calls.mockImplementationOnce(() => new Promise(() => undefined));
    expect(media.tile(1, 0, 400)).not.toBeNull();
  });

  it("asks for the waveform, then serves peaks in chunks across a boundary", async () => {
    const api = backend();
    const changed = vi.fn();
    const media = new TimelineMedia(api, changed);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    expect(media.peaks(1, 1, 0, 100, 10)).toBeNull();
    await flush();
    expect(changed).toHaveBeenCalledTimes(1);
    // 100 columns from column 500: the end of chunk 0 and the start of 1.
    expect(media.peaks(1, 1, 50, 100, 10)).toBeNull();
    await flush();
    const peaks = media.peaks(1, 1, 50, 100, 10);
    expect(peaks).not.toBeNull();
    expect(peaks?.[0]).toBe(0 + 500);
    expect(peaks?.[2 * 12]).toBe(Math.round((PEAK_CHUNK * 1) / 10) + 0);
    expect(
      api.calls.mock.calls.filter(([command]) => command === "waveform_peaks"),
    ).toHaveLength(2);
  });

  it("stops waiting for a waveform the engine gave up on, and does not ask again (#152)", async () => {
    const api = backend();
    api.calls.mockImplementation((command: string) =>
      command === "generate_waveform"
        ? Promise.resolve({ state: "pending", fraction: 0 })
        : Promise.reject(new Error(command)),
    );
    const changed = vi.fn();
    const media = new TimelineMedia(api, changed);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    expect(media.peaks(1, 1, 0, 100, 10)).toBeNull();
    await flush();
    expect(media.waveformFailure(1, 1)).toBeNull();

    media.waveformUpdate({
      path: "C:\\a.mp4",
      stream: 1,
      status: { state: "failed", reason: "could not decode the audio" },
    });
    expect(changed).toHaveBeenCalled();
    expect(media.waveformFailure(1, 1)).toBe("could not decode the audio");
    // Every repaint asks again for what it draws; none of them restarts
    // the work.
    expect(media.peaks(1, 1, 0, 100, 10)).toBeNull();
    expect(media.peaks(1, 1, 0, 100, 10)).toBeNull();
    await flush();
    expect(
      api.calls.mock.calls.filter(
        ([command]) => command === "generate_waveform",
      ),
    ).toHaveLength(1);
  });

  it("counts a waveform the engine refused to start as failed, not pending (#152)", async () => {
    const api = backend();
    api.calls.mockImplementation(() =>
      Promise.reject(new Error("the file could not be probed")),
    );
    const changed = vi.fn();
    const media = new TimelineMedia(api, changed);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    expect(media.peaks(1, 1, 0, 100, 10)).toBeNull();
    await flush();
    expect(changed).toHaveBeenCalled();
    expect(media.waveformFailure(1, 1)).toContain("could not be probed");
  });

  it("keeps a failure that arrived before the command's own `pending` answer (#152)", async () => {
    const api = backend();
    let answer: (status: unknown) => void = () => undefined;
    api.calls.mockImplementation(
      () => new Promise((resolve) => (answer = resolve)),
    );
    const media = new TimelineMedia(api, () => undefined);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    media.peaks(1, 1, 0, 100, 10);
    media.waveformUpdate({
      path: "C:\\a.mp4",
      stream: 1,
      status: { state: "failed", reason: "could not decode the audio" },
    });
    answer({ state: "pending", fraction: 0 });
    await flush();
    expect(media.waveformFailure(1, 1)).toBe("could not decode the audio");
  });

  it("does nothing more once disposed", async () => {
    const api = backend();
    const changed = vi.fn();
    const media = new TimelineMedia(api, changed);
    media.setSources(new Map([[1, "C:\\a.mp4"]]));
    media.tile(1, 0, 50);
    media.dispose();
    await flush();
    expect(changed).not.toHaveBeenCalled();
    expect(media.tile(2, 0, 50)).toBeNull();
  });
});
