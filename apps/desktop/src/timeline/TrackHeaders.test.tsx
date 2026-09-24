import type { ProjectView, Track } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import { detachAction } from "./editActions.js";
import { withLinked } from "./interaction.js";
import { COLLAPSED_HEIGHT, layoutRows, trackNames } from "./rows.js";
import { TrackHeaders } from "./TrackHeaders.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const track = (
  id: number,
  kind: "video" | "audio",
  extra: Partial<Track> = {},
): Track => ({
  id,
  kind,
  name: "",
  muted: false,
  solo: false,
  locked: false,
  collapsed: false,
  clips: [],
  ...extra,
});

function view(tracks: Track[]): ProjectView {
  return {
    path: "C:\\Work\\Trip.blinkify",
    dirty: false,
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
        matchFirstClip: false,
        tracks,
      },
    },
    timeline: {
      timeBase: { num: 1, den: 30 },
      frameRate: { num: 30, den: 1 },
      tracks: tracks.map((t) => ({
        id: t.id,
        kind: t.kind,
        visible: t.kind === "video" && !t.muted,
        audible: !t.muted,
        placements: [],
      })),
    },
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
  };
}

const TRACKS = [
  track(1, "video"),
  track(2, "video", { name: "B-roll", collapsed: true }),
  track(3, "audio", { locked: true }),
];

function show() {
  render(
    <TooltipProvider>
      <TrackHeaders />
    </TooltipProvider>,
  );
}

describe("track headers", () => {
  beforeEach(() => {
    invoked.mockReset();
    invoked.mockResolvedValue({
      view: view(TRACKS),
      context: { selection: [], playhead: 0 },
      clamped: false,
    });
    useProjectStore.setState({ view: view(TRACKS), selection: [] });
  });

  it("names tracks by kind from the top unless the user named them", () => {
    expect([...trackNames(TRACKS).values()]).toEqual(["V1", "B-roll", "A1"]);
    const rows = layoutRows(view(TRACKS).timeline, TRACKS);
    expect(rows.map((r) => r.height)).toEqual([56, COLLAPSED_HEIGHT, 40]);
    expect(rows[2]?.locked).toBe(true);
  });

  it("sends each switch as an edit", async () => {
    show();
    await userEvent.click(screen.getByRole("button", { name: "Hide V1" }));
    expect(invoked).toHaveBeenCalledWith("edit_project", {
      edit: {
        edit: "set-track",
        track: 1,
        muted: true,
        solo: null,
        locked: null,
        collapsed: null,
      },
      context: { selection: [], playhead: 0 },
    });
    await userEvent.click(screen.getByRole("button", { name: "Unlock A1" }));
    expect(invoked).toHaveBeenLastCalledWith(
      "edit_project",
      expect.objectContaining({
        edit: expect.objectContaining({ track: 3, locked: false }) as unknown,
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: "+ Audio" }));
    expect(invoked).toHaveBeenLastCalledWith(
      "edit_project",
      expect.objectContaining({
        edit: { edit: "add-track", kind: "audio" },
      }),
    );
  });

  it("renames a track in place", async () => {
    show();
    fireEvent.doubleClick(screen.getByText("V1"));
    const field = screen.getByRole("textbox", { name: "Rename V1" });
    await userEvent.type(field, "Main{Enter}");
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("edit_project", {
        edit: { edit: "rename-track", track: 1, name: "Main" },
        context: { selection: [], playhead: 0 },
      }),
    );
  });
});

describe("linked clips", () => {
  it("select together", () => {
    const tracks = [
      { clips: [{ id: 1, link: 1 }, { id: 2 }] },
      { clips: [{ id: 3, link: 1 }] },
    ];
    expect(withLinked([1], tracks)).toEqual([1, 3]);
    expect(withLinked([2], tracks)).toEqual([2]);
    expect(withLinked([3], tracks)).toEqual([3, 1]);
  });

  it("detach only what still has its sound", () => {
    const timeline = view(TRACKS).timeline;
    const placement = {
      track: 1,
      kind: "video" as const,
      clip: 7,
      source: 1,
      stream: 0,
      timeBase: { num: 1, den: 1000 },
      sourceIn: 0,
      sourceOut: 1000,
      start: 0,
      length: 30,
      speed: { num: 1, den: 1 },
      audio: [],
      sequenceTimeBase: { num: 1, den: 30 },
      silent: false,
    };
    const withClip = {
      ...timeline!,
      tracks: [{ ...timeline!.tracks[0]!, placements: [placement] }],
    };
    expect(detachAction(withClip, [7]).edit).toEqual({
      edit: "detach-audio",
      clips: [7],
    });
    const detached = {
      ...withClip,
      tracks: [
        {
          ...withClip.tracks[0]!,
          placements: [{ ...placement, silent: true }],
        },
      ],
    };
    expect(detachAction(detached, [7]).edit).toBeNull();
  });
});
