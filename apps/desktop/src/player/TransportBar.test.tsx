import type { PlaybackStatus } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "./preview.store.js";
import { TransportBar } from "./TransportBar.js";
import {
  commandForKey,
  useTransportShortcuts,
} from "./useTransportShortcuts.js";

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
      usePreviewStore.getState().showFrame(35);
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

describe("the transport keys", () => {
  const key = (k: string, modifiers: Partial<KeyboardEvent> = {}) => ({
    key: k,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    shiftKey: false,
    ...modifiers,
  });

  it("map Space, the arrows, Home and End to the transport", () => {
    expect(commandForKey(key(" "))).toEqual({ type: "toggle" });
    expect(commandForKey(key("ArrowLeft"))).toEqual({
      type: "step",
      frames: -1,
    });
    expect(commandForKey(key("ArrowRight"))).toEqual({
      type: "step",
      frames: 1,
    });
    expect(commandForKey(key("Home"))).toEqual({ type: "jump-to-start" });
    expect(commandForKey(key("End"))).toEqual({ type: "jump-to-end" });
    expect(commandForKey(key("ArrowRight", { ctrlKey: true }))).toBeNull();
    expect(commandForKey(key("a"))).toBeNull();
  });

  function Harness({ send }: { send: (command: object) => void }) {
    useTransportShortcuts(true, send);
    return (
      <>
        <button type="button">Somewhere</button>
        <input aria-label="A name" />
      </>
    );
  }

  it("drive the transport from anywhere but a text field", () => {
    const send = vi.fn();
    render(<Harness send={send} />);
    fireEvent.keyDown(window, { key: " " });
    expect(send).toHaveBeenLastCalledWith({ type: "toggle" });
    // Space on a focused button plays, rather than clicking the button.
    const button = screen.getByRole("button", { name: "Somewhere" });
    const event = fireEvent.keyDown(button, { key: " " });
    expect(event).toBe(false);
    expect(send).toHaveBeenCalledTimes(2);
    // Typing a space in a text field is typing.
    fireEvent.keyDown(screen.getByRole("textbox", { name: "A name" }), {
      key: " ",
    });
    expect(send).toHaveBeenCalledTimes(2);
  });
});
