import type { MonitorLevels } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { meterPercent, MonitorControls } from "./MonitorControls.js";
import { usePreviewStore } from "./preview.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const UNITY = { volume: 1, muted: false, soloed: [], mutedTracks: [] };

function renderControls() {
  return render(
    <TooltipProvider>
      <MonitorControls />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  invoked.mockReset();
  invoked.mockImplementation((command) =>
    Promise.resolve(
      command === "monitor_levels"
        ? { peakDb: [null, null], shortTermLufs: null, clipped: false }
        : UNITY,
    ),
  );
  usePreviewStore.setState({
    status: "open",
    session: 4,
    monitoring: UNITY,
    levels: null,
  });
});

describe("MonitorControls", () => {
  it("mutes and sets the volume as monitor commands, never as gain", async () => {
    renderControls();
    await userEvent.click(screen.getByRole("button", { name: "Mute monitor" }));
    expect(invoked).toHaveBeenCalledWith("monitor", {
      session: 4,
      command: { type: "mute", muted: true },
    });
    const volume = screen.getByRole("slider", { name: "Monitor volume" });
    volume.focus();
    await userEvent.keyboard("{ArrowLeft}");
    expect(invoked).toHaveBeenCalledWith("monitor", {
      session: 4,
      command: { type: "volume", level: 0.99 },
    });
  });

  it("shows the peaks and the short-term loudness the engine measured", () => {
    usePreviewStore.setState({
      levels: { peakDb: [-12, -30], shortTermLufs: -23.04, clipped: false },
    });
    renderControls();
    expect(screen.getByRole("meter", { name: "Left peak" })).toHaveAttribute(
      "aria-valuenow",
      "-12",
    );
    expect(screen.getByLabelText("Short-term loudness")).toHaveTextContent(
      "-23.0 LUFS",
    );
    expect(screen.getByRole("button", { name: "No clipping" })).toBeDisabled();
  });

  it("keeps a clip lit until it is clicked, and the click resets it", async () => {
    const clipped: MonitorLevels = {
      peakDb: [-1, -1],
      shortTermLufs: -9,
      clipped: true,
    };
    // The engine keeps saying it clipped until it is told to reset.
    invoked.mockImplementation((command) =>
      Promise.resolve(command === "monitor_levels" ? clipped : UNITY),
    );
    usePreviewStore.setState({ levels: clipped });
    renderControls();
    await userEvent.click(
      screen.getByRole("button", { name: "Clipped — reset" }),
    );
    expect(invoked).toHaveBeenCalledWith("monitor", {
      session: 4,
      command: { type: "reset-clip" },
    });
    // The engine is asked again at once, and says it is out.
    await act(async () => {
      await Promise.resolve();
    });
    expect(invoked).toHaveBeenCalledWith("monitor_levels", { session: 4 });
  });

  it("scales the meter from −60 dBFS to full scale", () => {
    expect(meterPercent(null)).toBe(0);
    expect(meterPercent(-80)).toBe(0);
    expect(meterPercent(-30)).toBe(50);
    expect(meterPercent(0)).toBe(100);
    expect(meterPercent(3)).toBe(100);
  });
});
