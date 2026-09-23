import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "../../player/preview.store.js";
import { PlayerZone } from "./PlayerZone.js";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: (path: string, scheme: string) =>
    `http://${scheme}.localhost/${path}`,
}));

const invoked = vi.mocked(invoke);

const PREVIEW = {
  session: 7,
  info: {
    frame: { width: 1280, height: 720 },
    rotation: 90,
    displayWidth: 720,
    displayHeight: 1280,
    timeBase: { num: 1, den: 15360 },
    durationSeconds: 4,
    stream: 0,
  },
};

beforeEach(() => {
  invoked.mockReset();
  // jsdom has no 2D canvas. The surface then draws nothing, which is all
  // these tests need; the drawing itself is tested in `orientation.test.ts`.
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  usePreviewStore.setState({
    status: "empty",
    preview: null,
    path: null,
    error: null,
    stats: null,
  });
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
    usePreviewStore.setState({ status: "playing", preview: PREVIEW });
    render(<PlayerZone />);
    expect(screen.getByTestId("preview-canvas")).toBeInTheDocument();
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
      ended: false,
      error: null,
    });
    usePreviewStore.setState({ status: "playing", preview: PREVIEW });
    render(<PlayerZone />);
    await userEvent.click(screen.getByRole("button", { name: "Stats" }));
    const stats = await screen.findByTestId("decode-stats");
    expect(invoked).toHaveBeenCalledWith("preview_stats", { session: 7 });
    expect(stats).toHaveTextContent("12/16 frames · 12.0 MB");
    expect(stats).toHaveTextContent("58.5 fps");
    expect(stats).toHaveTextContent("Dropped3");
  });
});
