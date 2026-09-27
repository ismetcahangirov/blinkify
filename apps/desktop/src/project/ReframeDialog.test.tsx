import type { ProjectView, ReframeImpact } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "./project.store.js";
import { reframeStatement } from "./reframe.js";
import { ReframeDialog } from "./ReframeDialog.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

/** #132's acceptance: three landscape clips and a phone clip, to 9:16. */
const VERTICAL: ReframeImpact = {
  reframe: {
    aspect: "9:16",
    settings: {
      width: 1080,
      height: 1920,
      frameRate: { num: 30, den: 1 },
      pixelAspect: { num: 1, den: 1 },
      colour: "sdr",
    },
    basis: { basis: "native", source: 2 },
    cropped: [1, 2, 3],
    kept: [],
    uncropped: [4],
    locked: [],
    scaledUp: [1, 2, 3],
  },
  reEncodedClips: [1, 2, 3],
  reEncodedSeconds: 9,
  stillReEncodedClips: [],
  copiedClips: [4],
  copiedSeconds: 3,
  declined: false,
};

const names = (source: number) => (source === 2 ? "phone.mp4" : "trip.mp4");

describe("the reframe statement", () => {
  it("says how many clips are re-encoded and for how long, and which stay copies", () => {
    expect(reframeStatement(VERTICAL, names)).toEqual([
      "3 clips (0:09) would no longer be copied and would be re-encoded because of the reframe.",
      "1 clip (0:03) would still be copied bit for bit: 1 clip already 9:16 is not cropped.",
      "The sequence becomes 1080 × 1920, the size of phone.mp4, which is already 9:16, so its clips are copied.",
      "3 clips are smaller than that and would be scaled up to fill it.",
    ]);
  });

  it("says what size it chose from a crop, and what it left on locked tracks", () => {
    const lines = reframeStatement(
      {
        ...VERTICAL,
        reframe: {
          ...VERTICAL.reframe,
          settings: { ...VERTICAL.reframe.settings, width: 608, height: 1080 },
          basis: { basis: "cropped", source: 1 },
          uncropped: [],
          locked: [5, 6],
          scaledUp: [],
        },
        copiedClips: [],
        stillReEncodedClips: [5, 6],
      },
      names,
    );
    expect(lines).toContain(
      "The sequence becomes 608 × 1080, the 9:16 crop of trip.mp4, so it is not scaled up.",
    );
    expect(lines).toContain(
      "2 clips on locked tracks are left as they are, not cropped; the new size still applies to them.",
    );
    expect(lines).toContain("2 clips already re-encoded would stay so.");
  });
});

function view(): ProjectView {
  return {
    path: null,
    dirty: false,
    project: {
      schemaVersion: 7,
      name: "Trip",
      sources: {
        1: {
          path: "C:\\Clips\\trip.mp4",
          fingerprint: { size: 1, modified: null, contentHash: "a" },
        },
        2: {
          path: "C:\\Clips\\phone.mp4",
          fingerprint: { size: 2, modified: null, contentHash: "b" },
        },
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
    frames: {},
  };
}

const edit = vi.fn((_: unknown) => Promise.resolve<string | null>(null));

function show(onOpenChange = vi.fn()) {
  useProjectStore.setState({ view: view(), edit });
  render(
    <TooltipProvider>
      <ReframeDialog open onOpenChange={onOpenChange} />
    </TooltipProvider>,
  );
  return onOpenChange;
}

describe("the reframe dialog", () => {
  beforeEach(() => {
    invoked.mockReset();
    edit.mockClear();
    edit.mockImplementation(() => Promise.resolve(null));
  });

  it("states the engine's cost before applying, then applies one edit", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(command === "preview_reframe" ? VERTICAL : null),
    );
    const onOpenChange = show();
    const user = userEvent.setup();
    expect(
      await screen.findByText(/3 clips \(0:09\) would no longer be copied/),
    ).toBeVisible();
    // Asked for the first shape the 16:9 sequence is not.
    expect(invoked).toHaveBeenCalledWith("preview_reframe", { aspect: "9:16" });
    expect(edit).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Reframe" }));
    expect(edit).toHaveBeenCalledTimes(1);
    expect(edit).toHaveBeenCalledWith({ edit: "reframe", aspect: "9:16" });
    await waitFor(() => {
      expect(onOpenChange).toHaveBeenCalledWith(false);
    });
  });

  it("asks again for another shape", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(command === "preview_reframe" ? VERTICAL : null),
    );
    show();
    const user = userEvent.setup();
    await screen.findByText(/would no longer be copied/);
    screen.getByRole("combobox", { name: "Shape" }).focus();
    await user.keyboard("{Enter}");
    await user.click(await screen.findByRole("option", { name: "1:1 square" }));
    await waitFor(() => {
      expect(invoked).toHaveBeenCalledWith("preview_reframe", {
        aspect: "1:1",
      });
    });
  });

  it("shows the engine's refusal and applies nothing", async () => {
    invoked.mockImplementation(() =>
      Promise.reject(
        new Error(
          "clip 3's source is offline or could not be read, so its crop cannot be placed: relink it before reframing",
        ),
      ),
    );
    show();
    expect(await screen.findByText(/relink it before reframing/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Reframe" })).toBeDisabled();
  });
});
