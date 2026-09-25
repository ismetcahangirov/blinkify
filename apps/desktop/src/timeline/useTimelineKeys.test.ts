import type { ProjectView } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import { runTimelineAction, timelineActionForKey } from "./useTimelineKeys.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const key = (key: string, modifiers: Partial<KeyboardEvent> = {}) => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...modifiers,
});

function view(): ProjectView {
  const placement = (clip: number, start: number) => ({
    track: 1,
    kind: "video" as const,
    clip,
    source: 1,
    stream: 0,
    timeBase: { num: 1, den: 1000 },
    sourceIn: 0,
    sourceOut: 1000,
    start,
    length: 30,
    speed: { num: 1, den: 1 },
    audio: [],
    sequenceTimeBase: { num: 1, den: 30 },
    silent: false,
  });
  return {
    path: "C:\\Work\\Trip.blinkify",
    dirty: false,
    project: {
      schemaVersion: 2,
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
    timeline: {
      timeBase: { num: 1, den: 30 },
      frameRate: { num: 30, den: 1 },
      tracks: [
        {
          id: 1,
          kind: "video",
          visible: true,
          audible: true,
          placements: [placement(1, 0), placement(2, 30), placement(3, 60)],
        },
      ],
    },
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
    assets: {},
    speeds: {},
  };
}

describe("the timeline's editing keys", () => {
  beforeEach(() => {
    invoked.mockReset();
    invoked.mockResolvedValue({
      view: view(),
      context: { selection: [], playhead: 0 },
      clamped: false,
    });
    useProjectStore.setState({ view: view(), selection: [2] });
  });

  it("maps Delete, Shift+Delete and Ctrl+A", () => {
    expect(timelineActionForKey(key("Delete"))).toBe("delete");
    expect(timelineActionForKey(key("Backspace"))).toBe("delete");
    expect(timelineActionForKey(key("Delete", { shiftKey: true }))).toBe(
      "ripple-delete",
    );
    expect(timelineActionForKey(key("a", { ctrlKey: true }))).toBe(
      "select-all",
    );
    expect(timelineActionForKey(key("b", { ctrlKey: true }))).toBe("split");
    expect(timelineActionForKey(key("a"))).toBeNull();
    expect(timelineActionForKey(key("Delete", { altKey: true }))).toBeNull();
  });

  it("deletes, ripple-deletes and selects all as one edit each", async () => {
    runTimelineAction("ripple-delete");
    await vi.waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("edit_project", {
        edit: { edit: "ripple-delete", clips: [2] },
        context: { selection: [2], playhead: 0 },
      }),
    );
    runTimelineAction("select-all");
    expect(useProjectStore.getState().selection).toEqual([1, 2, 3]);
    invoked.mockClear();
    useProjectStore.setState({ selection: [] });
    runTimelineAction("delete");
    expect(invoked).not.toHaveBeenCalled();
  });
});
