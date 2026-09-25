import type { PlaybackStatus } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "./preview.store.js";
import { TransportBar } from "./TransportBar.js";

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

function renderBar() {
  return render(
    <TooltipProvider>
      <TransportBar />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  invoked.mockReset();
  invoked.mockResolvedValue(PAUSED);
  usePreviewStore.setState({
    status: "open",
    session: 5,
    playback: PAUSED,
    frameNumber: null,
    error: null,
  });
});

describe("TransportBar", () => {
  it("sends each control's command to the open session", async () => {
    renderBar();
    const expectations: Array<[string, object]> = [
      ["Jump to start", { type: "jump-to-start" }],
      ["Previous frame", { type: "step", frames: -1 }],
      ["Play", { type: "toggle" }],
      ["Next frame", { type: "step", frames: 1 }],
      ["Jump to end", { type: "jump-to-end" }],
      ["Stop", { type: "stop" }],
      [
        "Loop playback",
        { type: "set-loop", range: { start: 0, end: 4_000_000 } },
      ],
    ];
    for (const [name, command] of expectations) {
      await userEvent.click(screen.getByRole("button", { name }));
      expect(invoked).toHaveBeenLastCalledWith("transport", {
        session: 5,
        command,
      });
    }
  });

  it("shows pause while playing, and the loop as pressed while looping", () => {
    usePreviewStore.setState({
      playback: {
        ...PAUSED,
        state: "playing",
        loopRange: { start: 0, end: 4_000_000 },
      },
    });
    renderBar();
    expect(screen.getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Stop looping" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("shows the timecode of the frame on screen over the total", () => {
    renderBar();
    expect(screen.getByTestId("timecode")).toHaveTextContent(
      "00:00:00:00 / 00:00:04:00",
    );
    act(() => {
      usePreviewStore.getState().showFrame(35, 1_166_667);
    });
    expect(screen.getByTestId("timecode")).toHaveTextContent(
      "00:00:01:05 / 00:00:04:00",
    );
  });
});

describe("the audio output", () => {
  it("says so when playback is silent, and why", () => {
    usePreviewStore.setState({
      playback: {
        ...PAUSED,
        audio: { kind: "silent", reason: "there is no audio output device" },
      },
    });
    renderBar();
    expect(screen.getByTestId("audio-silent")).toHaveTextContent(
      "No sound: there is no audio output device",
    );
  });

  it("says nothing while a device is playing", () => {
    renderBar();
    expect(screen.queryByTestId("audio-silent")).toBeNull();
  });
});
