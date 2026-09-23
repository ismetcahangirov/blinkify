import type { PlaybackStatus } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "./preview.store.js";
import { droppedOn } from "./useFileDrop.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const PAUSED: PlaybackStatus = {
  state: "paused",
  position: 0,
  duration: 4_000_000,
  frameRate: { num: 30, den: 1 },
  timecode: "00:00:00:00",
  durationTimecode: "00:00:04:00",
  speed: "normal",
  loopRange: null,
  audio: { kind: "device", name: "Speakers" },
  resolving: false,
  proxy: false,
};

const opened = (session: number) => ({ session, status: PAUSED });

beforeEach(() => {
  invoked.mockReset();
  usePreviewStore.setState({
    status: "empty",
    session: null,
    playback: null,
    frameNumber: null,
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
      status: "open",
      session: 1,
      playback: PAUSED,
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
    expect(usePreviewStore.getState().session).toBe(2);
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
    expect(usePreviewStore.getState().session).toBe(2);
    expect(invoked).toHaveBeenCalledWith("close_preview", { session: 1 });
  });

  it("reports an engine refusal as a failure with its reason", async () => {
    invoked.mockRejectedValueOnce("C:\\a.txt has no audio or video to play");
    await usePreviewStore.getState().open("C:\\a.txt", 100, 100);
    expect(usePreviewStore.getState()).toMatchObject({
      status: "failed",
      session: null,
      error: "C:\\a.txt has no audio or video to play",
    });
  });

  it("closing tells the engine, which ends the decoders", async () => {
    invoked.mockResolvedValueOnce(opened(4));
    await usePreviewStore.getState().open("a.mp4", 100, 100);
    invoked.mockResolvedValueOnce(undefined);
    await usePreviewStore.getState().close();
    expect(invoked).toHaveBeenLastCalledWith("close_preview", { session: 4 });
    expect(usePreviewStore.getState().status).toBe("empty");
  });

  it("sends transport commands to the open session and shows the answer", async () => {
    invoked.mockResolvedValueOnce(opened(3));
    await usePreviewStore.getState().open("a.mp4", 100, 100);
    const playing = { ...PAUSED, state: "playing" as const };
    invoked.mockResolvedValueOnce(playing);
    await usePreviewStore.getState().transport({ type: "toggle" });
    expect(invoked).toHaveBeenLastCalledWith("transport", {
      session: 3,
      command: { type: "toggle" },
    });
    expect(usePreviewStore.getState().playback).toEqual(playing);
  });

  it("takes pushed updates for its own session only", async () => {
    invoked.mockResolvedValueOnce(opened(3));
    await usePreviewStore.getState().open("a.mp4", 100, 100);
    const ended = { ...PAUSED, state: "ended" as const };
    usePreviewStore.getState().applyUpdate({ session: 9, status: ended });
    expect(usePreviewStore.getState().playback?.state).toBe("paused");
    usePreviewStore.getState().applyUpdate({ session: 3, status: ended });
    expect(usePreviewStore.getState().playback?.state).toBe("ended");
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
