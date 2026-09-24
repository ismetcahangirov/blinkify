import type { ProjectView, SettingsImpact } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useProjectStore } from "./project.store.js";
import {
  displayAspect,
  duration,
  impactStatement,
  SequenceSettingsDialog,
} from "./SequenceSettingsDialog.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const NONE: SettingsImpact = {
  losingClips: 0,
  losingSeconds: 0,
  gainingClips: 0,
  gainingSeconds: 0,
  ineligibleAfter: 0,
};

function view(matchFirstClip = false): ProjectView {
  return {
    path: "C:\\Work\\Trip.blinkify",
    project: {
      schemaVersion: 2,
      name: "Trip",
      sources: {},
      sequence: {
        settings: {
          width: 3840,
          height: 2160,
          frameRate: { num: 25, den: 1 },
          pixelAspect: { num: 1, den: 1 },
          colour: "sdr",
        },
        matchFirstClip,
        tracks: [],
      },
    },
    timeline: null,
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
  };
}

function show() {
  render(
    <TooltipProvider>
      <SequenceSettingsDialog open onOpenChange={() => undefined} />
    </TooltipProvider>,
  );
}

describe("the sequence settings dialog", () => {
  beforeEach(() => {
    invoked.mockReset();
    useProjectStore.setState({ view: view(), selection: [] });
  });

  it("states the cost of a change before it is applied", async () => {
    invoked.mockImplementation((command, args) => {
      if (command !== "preview_settings") return Promise.resolve(null);
      const { settings } = args as { settings: { width: number } };
      return Promise.resolve(
        settings.width === 3840
          ? NONE
          : { ...NONE, losingClips: 3, losingSeconds: 102, ineligibleAfter: 3 },
      );
    });
    show();
    expect(await screen.findByTestId("settings-impact")).toHaveTextContent(
      "Every clip that can be copied losslessly now still can.",
    );
    expect(screen.getByTestId("display-aspect")).toHaveTextContent("16:9");

    const width = screen.getByRole("spinbutton", { name: "Width" });
    await userEvent.clear(width);
    await userEvent.type(width, "1920{Enter}");
    await waitFor(() =>
      expect(screen.getByTestId("settings-impact")).toHaveTextContent(
        "3 clips (1:42) would no longer be copied losslessly",
      ),
    );
    expect(screen.getByTestId("settings-impact")).toHaveAttribute(
      "data-state",
      "costly",
    );
    // Asked of the engine, never worked out here.
    expect(invoked).toHaveBeenCalledWith("preview_settings", {
      settings: expect.objectContaining({
        width: 1920,
        height: 2160,
      }) as unknown,
    });
  });

  it("applies the change as one edit", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(
        command === "preview_settings"
          ? NONE
          : { view: view(), context: { selection: [], playhead: 0 } },
      ),
    );
    show();
    const height = await screen.findByRole("spinbutton", { name: "Height" });
    await userEvent.clear(height);
    await userEvent.type(height, "1080{Enter}");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("edit_project", {
        edit: {
          edit: "set-settings",
          settings: expect.objectContaining({
            width: 3840,
            height: 1080,
          }) as unknown,
        },
        context: { selection: [], playhead: 0 },
      }),
    );
  });

  it("shows the engine's refusal and will not apply it", async () => {
    invoked.mockRejectedValue(
      "a picture of 3840×2161 cannot be encoded: each side must be even",
    );
    show();
    expect(await screen.findByTestId("settings-impact")).toHaveTextContent(
      "cannot be encoded",
    );
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
  });

  it("says the sequence is waiting for its first clip", async () => {
    useProjectStore.setState({ view: view(true) });
    invoked.mockResolvedValue(NONE);
    show();
    expect(await screen.findByTestId("match-first-clip")).toHaveTextContent(
      "from the first video clip you add",
    );
    // Confirming the settings as they are is a choice too.
    expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled();
  });

  it("words durations, aspects and gains", () => {
    expect(duration(102)).toBe("1:42");
    expect(duration(3723)).toBe("1:02:03");
    expect(
      displayAspect({
        width: 1440,
        height: 1080,
        frameRate: { num: 25, den: 1 },
        pixelAspect: { num: 4, den: 3 },
        colour: "sdr",
      }),
    ).toBe("16:9");
    expect(
      impactStatement({ ...NONE, gainingClips: 1, gainingSeconds: 5 }),
    ).toBe("1 clip (0:05) would be copied losslessly instead of re-encoded.");
    expect(impactStatement({ ...NONE, ineligibleAfter: 2 })).toBe(
      "No clip changes how it is exported; 2 clips still cannot be copied.",
    );
  });
});
