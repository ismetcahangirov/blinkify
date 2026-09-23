import type { PlaybackStatus } from "@blinkify/types";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "./preview.store.js";
import { ScrubBar } from "./ScrubBar.js";

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

beforeEach(() => {
  invoked.mockReset();
  invoked.mockResolvedValue(PAUSED);
  usePreviewStore.setState({
    status: "open",
    session: 2,
    playback: PAUSED,
    frameNumber: null,
    framePosition: null,
  });
});

describe("ScrubBar", () => {
  it("follows the frame on screen", () => {
    render(<ScrubBar />);
    const playhead = screen.getByRole("slider", { name: "Playhead" });
    expect(playhead).toHaveAttribute("aria-valuenow", "0");
    act(() => {
      usePreviewStore.getState().showFrame(60, 2_000_000);
    });
    expect(playhead).toHaveAttribute("aria-valuenow", "2000000");
  });

  it("moves a frame on a key press, as a seek and not a drag", () => {
    render(<ScrubBar />);
    const playhead = screen.getByRole("slider", { name: "Playhead" });
    fireEvent.keyDown(playhead, { key: "ArrowRight" });
    const commands = invoked.mock.calls
      .filter(([name]) => name === "transport")
      .map(([, args]) => (args as { command: object }).command);
    // Radix commits before it reports the change; a key press must still
    // leave the player seeking, not scrubbing.
    expect(commands).toEqual([{ type: "seek", position: 33_333 }]);
  });
});
