import type { WaveformUpdate } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { describe, expect, it, vi } from "vitest";
import { WAVEFORM_EVENT } from "../timeline/timelineMedia.js";
import {
  onLibraryMedia,
  setLibrarySources,
  waveformReady,
} from "./libraryMedia.js";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: (path: string, protocol: string) => `${protocol}://${path}`,
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("the library's waveforms (#150)", () => {
  it("are ready once the engine's event says so, as the timeline's are", async () => {
    let deliver: ((update: WaveformUpdate) => void) | null = null;
    vi.mocked(listen).mockImplementation((event, handler) => {
      if (event === WAVEFORM_EVENT) {
        deliver = (payload) => handler({ event, id: 1, payload });
      }
      return Promise.resolve(() => undefined);
    });
    vi.mocked(invoke).mockImplementation((command) => {
      // The engine answers at once that it has started, and says `ready`
      // later, by event only.
      if (command === "generate_waveform")
        return Promise.resolve({ state: "pending", fraction: 0 });
      if (command === "waveform_peaks")
        return Promise.resolve(new Int16Array(1024).buffer);
      return Promise.reject(new Error(String(command)));
    });
    const changed = vi.fn();
    onLibraryMedia(changed);
    setLibrarySources(new Map([[1, "C:\\media\\clip.mp4"]]));

    expect(waveformReady(1, 1)).toBe(false);
    await flush();
    expect(waveformReady(1, 1)).toBe(false);

    expect(deliver).not.toBeNull();
    deliver!({
      path: "C:\\media\\clip.mp4",
      stream: 1,
      status: { state: "ready" },
    });
    expect(changed).toHaveBeenCalled();
    // Ready: the card's one column of peaks is asked for, then drawn.
    expect(waveformReady(1, 1)).toBe(false);
    await flush();
    expect(waveformReady(1, 1)).toBe(true);
  });
});
