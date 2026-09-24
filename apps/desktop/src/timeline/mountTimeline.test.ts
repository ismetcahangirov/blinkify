import type { ProjectView } from "@blinkify/types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { usePreviewStore } from "../player/preview.store.js";
import { useProjectStore } from "../project/project.store.js";
import {
  ineligibleClips,
  mountTimeline,
  type MountedTimeline,
} from "./mountTimeline.js";
import { RedrawScheduler, type FrameSource } from "./scheduler.js";
import { useTimelineStore } from "./timeline.store.js";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => new Promise(() => undefined)),
  convertFileSrc: (_: string, scheme: string) => `http://${scheme}.localhost/`,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

/** Frames that arrive only when the test says so. */
function manualFrames(): FrameSource & { run: () => void; pending: number } {
  let queued: (() => void)[] = [];
  return {
    get pending() {
      return queued.length;
    },
    request: (callback) => {
      queued.push(callback);
      return queued.length;
    },
    cancel: () => {
      queued = [];
    },
    run: () => {
      const now = queued;
      queued = [];
      for (const callback of now) callback();
    },
  };
}

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
      schemaVersion: 1,
      name: "Trip",
      sources: {
        1: {
          path: "C:\\clips\\a.mp4",
          fingerprint: { size: 1, modified: null, contentHash: "0" },
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
    timeline: {
      timeBase: { num: 1, den: 30 },
      frameRate: { num: 30, den: 1 },
      tracks: [
        {
          id: 1,
          kind: "video",
          visible: true,
          audible: true,
          placements: Array.from({ length: 200 }, (_, i) =>
            placement(i + 1, i * 30),
          ),
        },
      ],
    },
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: {},
    extents: [],
    assets: {},
  };
}

describe("the redraw scheduler", () => {
  it("draws each dirty layer once per frame, however often it was marked", () => {
    const frames = manualFrames();
    const drawn: string[][] = [];
    const scheduler = new RedrawScheduler(
      (layers) => drawn.push([...layers]),
      frames,
    );
    for (let i = 0; i < 100; i++) scheduler.invalidate("content");
    scheduler.invalidate("overlay");
    expect(frames.pending).toBe(1);
    frames.run();
    expect(drawn).toEqual([["content", "overlay"]]);
    frames.run();
    expect(drawn).toHaveLength(1);
    scheduler.invalidate("overlay");
    scheduler.dispose();
    frames.run();
    expect(drawn).toHaveLength(1);
  });
});

describe("the mounted timeline", () => {
  let frames: ReturnType<typeof manualFrames>;
  let mounted: MountedTimeline;

  beforeEach(() => {
    useProjectStore.setState({ view: view(), selection: [] });
    usePreviewStore.setState({ kind: "project", frameNumber: 0 });
    useTimelineStore.setState({ fitted: false, playhead: null });
    frames = manualFrames();
    const host = document.createElement("div");
    mounted = mountTimeline(
      host,
      document.createElement("canvas"),
      document.createElement("canvas"),
      frames,
    );
    frames.run();
  });

  afterEach(() => mounted.dispose());

  it("fits a newly opened project into view", () => {
    expect(useTimelineStore.getState().fitted).toBe(true);
    expect(useTimelineStore.getState().view.origin).toBe(0);
  });

  it("moves the playhead without redrawing a single clip", () => {
    const { draws } = mounted.scheduler;
    const content = draws.content;
    for (let frame = 1; frame <= 120; frame++) {
      usePreviewStore.setState({ frameNumber: frame });
      frames.run();
    }
    expect(draws.content).toBe(content);
    expect(draws.overlay).toBeGreaterThanOrEqual(120);
    expect(useTimelineStore.getState().playhead).toBe(120);
  });

  it("redraws the clips for a selection, a zoom or a new graph", () => {
    const { draws } = mounted.scheduler;
    let content = draws.content;
    useProjectStore.getState().select([3]);
    frames.run();
    expect(draws.content).toBe(content + 1);
    content = draws.content;
    useTimelineStore.getState().zoomIn();
    frames.run();
    expect(draws.content).toBe(content + 1);
    content = draws.content;
    useProjectStore.setState({ view: view() });
    frames.run();
    expect(draws.content).toBe(content + 1);
  });

  it("stops drawing once disposed", () => {
    mounted.dispose();
    const { draws } = mounted.scheduler;
    const before = { ...draws };
    useProjectStore.getState().select([1]);
    usePreviewStore.setState({ frameNumber: 9 });
    frames.run();
    expect(draws).toEqual(before);
  });
});

describe("the copy-ineligible mark", () => {
  it("marks the video clips of every source the model says cannot be copied", () => {
    const base = view();
    const rows = [
      {
        id: 1,
        kind: "video" as const,
        top: 0,
        height: 56,
        placements: [
          { ...base.timeline!.tracks[0]!.placements[0]!, clip: 1, source: 1 },
          { ...base.timeline!.tracks[0]!.placements[1]!, clip: 2, source: 2 },
        ],
      },
      {
        id: 2,
        kind: "audio" as const,
        top: 56,
        height: 40,
        placements: [
          { ...base.timeline!.tracks[0]!.placements[2]!, clip: 3, source: 1 },
        ],
      },
    ];
    const eligibility = {
      1: {
        eligible: false,
        mismatches: [
          {
            reason: "frame-rate" as const,
            sequence: { num: 30, den: 1 },
            source: { num: 25, den: 1 },
          },
        ],
        notes: [],
      },
      2: { eligible: true, mismatches: [], notes: [] },
    };
    // Only the video clip of source 1: sound is not what the rule is about.
    expect([...ineligibleClips({ ...base, eligibility }, rows)]).toEqual([1]);
    expect(ineligibleClips(null, rows).size).toBe(0);
  });
});
