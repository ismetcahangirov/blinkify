import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "./preview.store.js";
import { droppedOn } from "./useFileDrop.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const opened = (session: number) => ({
  session,
  info: {
    frame: { width: 640, height: 360 },
    rotation: 0,
    displayWidth: 640,
    displayHeight: 360,
    timeBase: { num: 1, den: 15360 },
    durationSeconds: 4,
    stream: 0,
  },
});

beforeEach(() => {
  invoked.mockReset();
  usePreviewStore.setState({
    status: "empty",
    preview: null,
    path: null,
    error: null,
    stats: null,
  });
});

describe("the preview store", () => {
  it("asks the engine for a preview sized to the surface", async () => {
    invoked.mockResolvedValueOnce(opened(1));
    await usePreviewStore.getState().open("C:\\clips\\a.mp4", 959.6, 540.2);
    expect(invoked).toHaveBeenCalledWith("open_preview", {
      path: "C:\\clips\\a.mp4",
      maxWidth: 960,
      maxHeight: 540,
    });
    expect(usePreviewStore.getState()).toMatchObject({
      status: "playing",
      preview: { session: 1 },
    });
  });

  it("closes the open session before opening another", async () => {
    invoked.mockResolvedValueOnce(opened(1));
    await usePreviewStore.getState().open("a.mp4", 100, 100);
    invoked.mockResolvedValueOnce(undefined).mockResolvedValueOnce(opened(2));
    await usePreviewStore.getState().open("b.mp4", 100, 100);
    expect(invoked.mock.calls.map(([command]) => command)).toEqual([
      "open_preview",
      "close_preview",
      "open_preview",
    ]);
    expect(invoked).toHaveBeenCalledWith("close_preview", { session: 1 });
    expect(usePreviewStore.getState().preview?.session).toBe(2);
  });

  it("closes a session that lost a race to a newer open", async () => {
    let finishSlow!: (value: unknown) => void;
    invoked.mockImplementation((command, args) => {
      if (command === "open_preview") {
        const path = (args as { path: string }).path;
        if (path === "slow.mp4") {
          return new Promise((resolve) => {
            finishSlow = resolve;
          });
        }
        return Promise.resolve(opened(2));
      }
      return Promise.resolve(undefined);
    });
    const slow = usePreviewStore.getState().open("slow.mp4", 100, 100);
    await Promise.resolve();
    await usePreviewStore.getState().open("fast.mp4", 100, 100);
    finishSlow(opened(1));
    await slow;
    expect(usePreviewStore.getState().preview?.session).toBe(2);
    expect(invoked).toHaveBeenCalledWith("close_preview", { session: 1 });
  });

  it("reports an engine refusal as a failure with its reason", async () => {
    invoked.mockRejectedValueOnce("C:\\a.txt has no video to preview");
    await usePreviewStore.getState().open("C:\\a.txt", 100, 100);
    expect(usePreviewStore.getState()).toMatchObject({
      status: "failed",
      preview: null,
      error: "C:\\a.txt has no video to preview",
    });
  });

  it("closing tells the engine, which ends the decoder", async () => {
    invoked.mockResolvedValueOnce(opened(4));
    await usePreviewStore.getState().open("a.mp4", 100, 100);
    invoked.mockResolvedValueOnce(undefined);
    await usePreviewStore.getState().close();
    expect(invoked).toHaveBeenLastCalledWith("close_preview", { session: 4 });
    expect(usePreviewStore.getState().status).toBe("empty");
  });
});

describe("droppedOn", () => {
  const rect = { left: 100, top: 50, right: 300, bottom: 150 };

  it("compares physical drop positions against CSS-pixel rectangles", () => {
    expect(droppedOn({ x: 400, y: 200 }, rect, 2)).toBe(true);
    expect(droppedOn({ x: 400, y: 200 }, rect, 1)).toBe(false);
    expect(droppedOn({ x: 100, y: 50 }, rect, 1)).toBe(true);
    expect(droppedOn({ x: 300, y: 50 }, rect, 1)).toBe(false);
  });
});
