import type { ProjectView } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { HistoryControls } from "./HistoryControls.js";
import { useProjectStore } from "./project.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

function view(entries: string[], applied: number): ProjectView {
  return {
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
    timeline: null,
    history: { entries, applied },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
    assets: {},
    speeds: {},
  };
}

function show() {
  render(
    <TooltipProvider>
      <HistoryControls />
    </TooltipProvider>,
  );
}

describe("history controls", () => {
  beforeEach(() => {
    invoked.mockReset();
    useProjectStore.setState({ view: null, selection: [] });
  });

  it("disables what cannot be done and names what can", () => {
    useProjectStore.setState({ view: view(["Move clip", "Trim clip"], 1) });
    show();
    expect(
      screen.getByRole("button", { name: "Undo Move clip" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Redo Trim clip" }),
    ).toBeEnabled();

    act(() => useProjectStore.setState({ view: view([], 0) }));
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Redo" })).toBeDisabled();
  });

  it("steps through the history to the entry chosen", async () => {
    const entries = ["Add clip", "Move clip", "Trim clip"];
    useProjectStore.setState({ view: view(entries, 3) });
    let applied = 3;
    invoked.mockImplementation((command) => {
      applied += command === "undo_edit" ? -1 : 1;
      return Promise.resolve({
        view: view(entries, applied),
        context: { selection: [], playhead: 0 },
      });
    });
    show();
    await userEvent.click(screen.getByRole("button", { name: "History" }));
    const list = await screen.findByRole("list", { name: "Edit history" });
    expect(list.querySelector('[aria-current="step"]')?.textContent).toBe(
      "Trim clip",
    );
    await userEvent.click(screen.getByRole("button", { name: "Add clip" }));
    await waitFor(() => expect(applied).toBe(1));
    expect(invoked.mock.calls.map(([command]) => command)).toEqual([
      "undo_edit",
      "undo_edit",
    ]);
  });
});
