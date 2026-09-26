import type { ExportOverview, ProjectView } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useProjectStore } from "../project/project.store.js";
import { ExportDialog } from "./ExportDialog.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(() => Promise.resolve("C:\\Exports\\trip.mp4")),
}));

const invoked = vi.mocked(invoke);

function view(): ProjectView {
  return {
    path: "C:\\Work\\Trip.blinkify",
    dirty: false,
    project: {
      schemaVersion: 6,
      name: "Trip",
      sources: {
        1: {
          path: "C:\\Clips\\trip.mp4",
          fingerprint: { size: 1, headSha256: "x" },
        } as never,
      },
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
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
    assets: {},
    speeds: {},
  };
}

const SMART_CUT: ExportOverview = {
  video: { lossless: false, copiedSeconds: 9.6, reEncodedSeconds: 0.4 },
  audio: { lossless: true, copiedSeconds: 10, reEncodedSeconds: 0 },
  lossless: false,
  durationSeconds: 10,
  reasons: [
    {
      media: "video",
      atSeconds: 2,
      tier: { tier: "smart-cut", reason: "in-point-not-keyframe-aligned" },
      sentence:
        "The clip starts 0.40 s from the nearest keyframe, so the pictures up to the next keyframe are re-encoded and the rest is copied.",
      declined: false,
    },
  ],
  snap: {
    edit: {
      edit: "snap-to-keyframes",
      snaps: [{ clip: 2, from: 0, to: 9000 }],
    },
    cuts: [{ clip: 2, atSeconds: 2, inShiftSeconds: -0.4, outShiftSeconds: 0 }],
    lengthChangeSeconds: 0.4,
    unsnappable: 0,
  },
  size: { bytes: 25_000_000, precise: false },
  space: { available: 500_000_000_000, enough: true },
  targetExists: false,
  problems: [],
};

function answer(overview: ExportOverview) {
  invoked.mockImplementation((command) => {
    switch (command) {
      case "export_overview":
        return Promise.resolve(overview);
      case "edit_project":
        return Promise.resolve({
          view: view(),
          context: { selection: [], playhead: 0 },
          clamped: false,
        });
      case "export_jobs":
        return Promise.resolve([]);
      default:
        return Promise.resolve(null);
    }
  });
}

function show() {
  render(
    <TooltipProvider>
      <ExportDialog open onOpenChange={() => undefined} />
    </TooltipProvider>,
  );
}

describe("the export dialog", () => {
  beforeEach(() => {
    invoked.mockReset();
    vi.mocked(save).mockClear();
    useProjectStore.setState({ view: view(), selection: [] });
  });

  it("states what the plan does before the export, stream by stream", async () => {
    answer(SMART_CUT);
    show();
    expect(await screen.findByTestId("export-lossless")).toHaveTextContent(
      "Not fully lossless",
    );
    expect(
      screen.getByText(
        "Pictures: 0.40 s of 10.00 s re-encoded, the rest copied bit for bit",
      ),
    ).toBeVisible();
    expect(screen.getByText("Sound: copied bit for bit")).toBeVisible();
    // The reason names its timecode and the distance to the keyframe.
    const reasons = screen.getByTestId("export-reasons");
    expect(reasons).toHaveTextContent("00:00:02:00");
    expect(reasons).toHaveTextContent("0.40 s from the nearest keyframe");
    expect(screen.getByText(/^Roughly/)).toBeVisible();
    // The preset preserving the source asks for lossless sound, in the
    // source's container.
    expect(invoked).toHaveBeenCalledWith("export_overview", {
      target: "Trip.mp4",
      audio: { codec: "flac" },
    });
  });

  it("offers the snap with each cut's shift, and applies it as an edit", async () => {
    answer(SMART_CUT);
    show();
    const offer = await screen.findByTestId("snap-offer");
    expect(offer).toHaveTextContent(
      "Clip at 00:00:02:00: start 0.40 s earlier, end unchanged",
    );
    expect(offer).toHaveTextContent("0.40 s longer");
    await userEvent.click(
      screen.getByRole("button", { name: "Snap to keyframes" }),
    );
    expect(invoked).toHaveBeenCalledWith(
      "edit_project",
      expect.objectContaining({ edit: SMART_CUT.snap?.edit }),
    );
  });

  it("exports only to a destination the user chose", async () => {
    answer({ ...SMART_CUT, lossless: true, reasons: [], snap: undefined });
    show();
    const start = await screen.findByTestId("start-export");
    expect(start).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await waitFor(() => expect(start).toBeEnabled());
    await userEvent.click(start);
    expect(invoked).toHaveBeenCalledWith("submit_export", {
      target: "C:\\Exports\\trip.mp4",
      overwrite: false,
      audio: { codec: "flac" },
    });
  });

  it("replaces an existing file only when it was chosen in the save dialog", async () => {
    answer({ ...SMART_CUT, targetExists: true });
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Choose…" }),
    );
    const start = screen.getByTestId("start-export");
    await waitFor(() => expect(start).toBeEnabled());
    await userEvent.click(start);
    expect(invoked).toHaveBeenCalledWith(
      "submit_export",
      expect.objectContaining({ overwrite: true }),
    );
  });

  it("refuses before the export when the drive is too full", async () => {
    answer({
      ...SMART_CUT,
      space: { available: 1024, enough: false },
      problems: [
        "the target's drive has 0 MB free, and the export needs about 25 MB",
      ],
    });
    show();
    await userEvent.click(
      await screen.findByRole("button", { name: "Choose…" }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Cannot export: the target's drive has 0 MB free",
    );
    expect(screen.getByTestId("start-export")).toBeDisabled();
  });
});
