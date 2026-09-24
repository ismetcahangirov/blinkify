import type { PlaybackStatus } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "../../player/preview.store.js";
import { useProjectStore } from "../../project/project.store.js";
import { PlayerZone } from "./PlayerZone.js";

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: (path: string, scheme: string) =>
    `http://${scheme}.localhost/${path}`,
}));

const invoked = vi.mocked(invoke);

const PLAYBACK: PlaybackStatus = {
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

const OPEN = { status: "open" as const, session: 7, playback: PLAYBACK };

beforeEach(() => {
  invoked.mockReset();
  // jsdom has no 2D canvas. The surface then draws nothing, which is all
  // these tests need; the drawing itself is tested in `orientation.test.ts`.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  usePreviewStore.setState({
    status: "empty",
    session: null,
    playback: null,
    frameNumber: null,
    path: null,
    kind: null,
    error: null,
    stats: null,
  });
  useProjectStore.setState({ view: null, error: null, relinkError: null });
});

describe("PlayerZone", () => {
  it("invites a drop when nothing is open", () => {
    render(<PlayerZone />);
    expect(screen.getByTestId("player-surface")).toHaveTextContent(
      "Drop a video file here to preview it.",
    );
    expect(screen.queryByTestId("preview-canvas")).toBeNull();
    expect(screen.queryByRole("button", { name: "Stats" })).toBeNull();
  });

  it("shows the video surface for an open preview", () => {
    usePreviewStore.setState(OPEN);
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    expect(screen.getByTestId("preview-canvas")).toBeInTheDocument();
    expect(
      screen.getByRole("toolbar", { name: "Transport" }),
    ).toBeInTheDocument();
  });

  it("says when the frame is still on its way, and when it is a proxy", () => {
    usePreviewStore.setState({
      ...OPEN,
      playback: { ...PLAYBACK, resolving: true, proxy: true },
    });
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    expect(screen.getByTestId("resolving")).toHaveTextContent(
      "Finding the frame…",
    );
    expect(screen.getByTestId("proxy-badge")).toHaveTextContent("Proxy");
  });

  it("shows neither once the frame is up and the picture is the file", () => {
    usePreviewStore.setState(OPEN);
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    expect(screen.queryByTestId("resolving")).toBeNull();
    expect(screen.queryByTestId("proxy-badge")).toBeNull();
    expect(
      screen.getByRole("slider", { name: "Playhead" }),
    ).toBeInTheDocument();
  });

  it("states why a preview could not open", () => {
    usePreviewStore.setState({
      status: "failed",
      error: "C:\\a.txt has no video to preview",
    });
    render(<PlayerZone />);
    expect(screen.getByRole("alert")).toHaveTextContent(
      "C:\\a.txt has no video to preview",
    );
  });

  it("shows the engine's decode statistics on request", async () => {
    invoked.mockResolvedValue({
      bufferedFrames: 12,
      capacityFrames: 16,
      bufferedBytes: 12 * 1024 * 1024,
      decodedFrames: 300,
      droppedFrames: 3,
      presentedFrames: 290,
      decodeFps: 58.5,
      resyncs: 0,
      error: null,
    });
    usePreviewStore.setState(OPEN);
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "Stats" }));
    const stats = await screen.findByTestId("decode-stats");
    expect(invoked).toHaveBeenCalledWith("preview_stats", { session: 7 });
    expect(stats).toHaveTextContent("12/16 frames · 12.0 MB");
    expect(stats).toHaveTextContent("58.5 fps");
    expect(stats).toHaveTextContent("Dropped3");
  });

  it("plays the open project through the evaluator, and lists its operations on request", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(
        command === "open_project_preview"
          ? { session: 9, status: PLAYBACK }
          : { position: 0, clips: [] },
      ),
    );
    useProjectStore.setState({
      view: {
        path: "C:\\Work\\Trip.blinkify",
        dirty: false,
        project: {
          schemaVersion: 1,
          name: "Trip",
          sources: {},
          sequence: {
            settings: {
              width: 1920,
              height: 1080,
              frameRate: { num: 30, den: 1 },
              pixelAspect: { num: 1, den: 1 },
              colour: "sdr",
            },
            matchFirstClip: false,
            tracks: [],
          },
        },
        unavailable: {},
        affectedClips: [],
        eligibility: {},
        extents: [],
        assets: {},
        timeline: null,
        history: { entries: [], applied: 0 },
      },
    });
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    const operations = await screen.findByRole("button", {
      name: "Operations",
    });
    expect(invoked).toHaveBeenCalledWith("open_project_preview", {
      maxWidth: 0,
      maxHeight: 0,
    });
    expect(usePreviewStore.getState()).toMatchObject({
      session: 9,
      kind: "project",
      path: "Trip",
    });
    await userEvent.click(operations);
    expect(await screen.findByTestId("operations-panel")).toHaveTextContent(
      "Nothing on the timeline at frame 0.",
    );
    expect(invoked).toHaveBeenCalledWith("operations_at", { session: 9 });
  });

  it("offers no operations for a dropped file, which has no graph", () => {
    usePreviewStore.setState({ ...OPEN, kind: "file" });
    render(
      <TooltipProvider>
        <PlayerZone />
      </TooltipProvider>,
    );
    expect(screen.queryByRole("button", { name: "Operations" })).toBeNull();
  });
});
