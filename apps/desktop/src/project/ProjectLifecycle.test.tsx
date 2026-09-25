import type { ProjectView, RecoveryOffer } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProjectLifecycle, useLifecycleUi, when } from "./ProjectLifecycle.js";
import { projectTitle, useProjectStore } from "./project.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(() => Promise.resolve(null)),
  save: vi.fn(() => Promise.resolve(null)),
}));

const invoked = vi.mocked(invoke);

function view(dirty = false, path: string | null = null): ProjectView {
  return {
    path,
    dirty,
    project: {
      schemaVersion: 4,
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
        matchFirstClip: true,
        tracks: [],
      },
    },
    timeline: null,
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
    assets: {},
    speeds: {},
  };
}

const OFFER: RecoveryOffer = {
  recovery: "C:\\Work\\Trip.blinkify.recovery",
  project: "C:\\Work\\Trip.blinkify",
  name: "Trip",
  recoveredAt: Date.UTC(2026, 8, 25, 10, 30),
  savedAt: Date.UTC(2026, 8, 25, 9, 0),
};

function show() {
  render(
    <TooltipProvider>
      <ProjectLifecycle />
    </TooltipProvider>,
  );
}

describe("the project lifecycle", () => {
  beforeEach(() => {
    invoked.mockReset();
    useProjectStore.setState({
      view: null,
      error: null,
      recovery: [],
      recent: [],
      lifecycleError: null,
    });
    useLifecycleUi.setState({ creating: false, closing: null });
  });

  it("starts a new untitled project when there is nothing to open or recover", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(
        command === "launch_project"
          ? null
          : command === "recovery_offers"
            ? []
            : command === "new_project"
              ? view()
              : [],
      ),
    );
    show();
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("new_project", {
        name: "Untitled project",
        settings: null,
      }),
    );
    expect(useProjectStore.getState().view?.project.name).toBe("Trip");
  });

  it("offers unsaved work back with both times, and restores it", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(
        command === "launch_project"
          ? null
          : command === "recovery_offers"
            ? [OFFER]
            : command === "restore_recovery"
              ? view(true, OFFER.project)
              : [],
      ),
    );
    show();
    const facts = await screen.findByTestId("recovery-offer");
    expect(facts).toHaveTextContent(when(OFFER.recoveredAt));
    expect(facts).toHaveTextContent(when(OFFER.savedAt));
    await userEvent.click(screen.getByRole("button", { name: "Restore" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("restore_recovery", {
        offer: OFFER,
      }),
    );
    expect(useProjectStore.getState().view?.dirty).toBe(true);
    expect(invoked).not.toHaveBeenCalledWith("new_project", expect.anything());
  });

  it("discards an offer, and then starts fresh", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(
        command === "launch_project"
          ? null
          : command === "recovery_offers"
            ? [OFFER]
            : command === "new_project"
              ? view()
              : null,
      ),
    );
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Discard" }),
    );
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("discard_recovery", {
        offer: OFFER,
      }),
    );
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("new_project", expect.anything()),
    );
  });

  it("asks before closing the window on unsaved work", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(command === "launch_project" ? view(true) : null),
    );
    show();
    await waitFor(() => expect(useProjectStore.getState().view).not.toBeNull());
    act(() => useLifecycleUi.getState().setClosing("window"));
    expect(
      await screen.findByRole("dialog", { name: "Save changes to Trip?" }),
    ).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Don't save" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("quit_app", { discard: true }),
    );
  });

  it("marks unsaved work in the title", () => {
    expect(projectTitle(view(true))).toBe("Trip •");
    expect(projectTitle(view(false))).toBe("Trip");
  });
});
