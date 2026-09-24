import type {
  Placement as EvaluatedPlacement,
  StreamExtent,
} from "@blinkify/types";
import { describe, expect, it } from "vitest";
import {
  dropAt,
  droppedAssets,
  extentFrames,
  type DropInput,
} from "./assetDrop.js";
import type { TrackRow } from "./draw.js";

function clip(id: number, start: number, length: number): EvaluatedPlacement {
  return {
    track: 1,
    kind: "video",
    clip: id,
    source: 9,
    stream: 0,
    timeBase: { num: 1, den: 1000 },
    sourceIn: 0,
    sourceOut: Math.round((length * 1000) / 30),
    start,
    length,
    speed: { num: 1, den: 1 },
    audio: [],
    sequenceTimeBase: { num: 1, den: 30 },
    silent: false,
  };
}

const V1: TrackRow = {
  id: 1,
  kind: "video",
  top: 0,
  height: 56,
  placements: [clip(1, 300, 60)],
};
const A1: TrackRow = {
  id: 2,
  kind: "audio",
  top: 56,
  height: 40,
  placements: [],
};

/** Two seconds of pictures in a 1/15360 time base; two of sound at 48 kHz. */
const EXTENTS: StreamExtent[] = [
  {
    source: 1,
    stream: 0,
    kind: "video",
    timeBase: { num: 1, den: 15360 },
    start: 0,
    end: 30720,
  },
  {
    source: 1,
    stream: 1,
    kind: "audio",
    timeBase: { num: 1, den: 48000 },
    start: 0,
    end: 96000,
  },
  {
    source: 2,
    stream: 0,
    kind: "audio",
    timeBase: { num: 1, den: 44100 },
    start: 0,
    end: 44100,
  },
];

const CLIP = { source: 1, name: "beach.mp4", video: 0, audio: 1 };
const SONG = { source: 2, name: "song.mp3", video: null, audio: 0 };

const input = (over: Partial<DropInput>): DropInput => ({
  assets: [CLIP],
  rows: [V1, A1],
  row: V1,
  x: 0,
  // 4 px a frame, from frame 0.
  view: { scale: 4, origin: 0, scrollTop: 0, width: 1000, height: 200 },
  playhead: null,
  extents: EXTENTS,
  frameRate: { num: 30, den: 1 },
  noSnap: false,
  ...over,
});

describe("dropping library assets on the timeline", () => {
  it("places the whole stream at the frame under the pointer", () => {
    const result = dropAt(input({ x: 400 }));
    expect(result.invalid).toBeNull();
    expect(result.edits).toEqual([
      {
        edit: "add-clip",
        track: 1,
        source: 1,
        stream: 0,
        timeBase: { num: 1, den: 15360 },
        start: 100,
        from: 0,
        to: 30720,
      },
    ]);
    expect(result.ghosts).toEqual([
      { clip: -1, row: 1, start: 100, length: 60 },
    ]);
  });

  it("puts the sound of a file on an audio track", () => {
    const result = dropAt(input({ row: A1, x: 40 }));
    expect(result.edits[0]).toMatchObject({ track: 2, stream: 1, start: 10 });
  });

  it("snaps the clip's end to the next clip, as a clip drag does", () => {
    // The end would be at 298; clip 1 starts at 300, 8 px away.
    const result = dropAt(input({ x: 238 * 4 }));
    expect(result.snapped).toBe(300);
    expect(result.edits[0]).toMatchObject({ start: 240 });
    // Alt suspends it.
    expect(dropAt(input({ x: 238 * 4, noSnap: true })).edits[0]).toMatchObject({
      start: 238,
    });
  });

  it("places several assets one after another, in drag order", () => {
    const result = dropAt(input({ assets: [CLIP, SONG], row: A1, x: 0 }));
    expect(result.edits.map((e) => e.edit === "add-clip" && e.start)).toEqual([
      0, 60,
    ]);
    expect(result.ghosts.map((g) => g.length)).toEqual([60, 30]);
  });

  it("refuses, with the reason, what cannot land", () => {
    expect(dropAt(input({ row: null })).invalid).toBe("Drop onto a track.");
    expect(dropAt(input({ assets: [SONG] })).invalid).toBe(
      "song.mp3 has no pictures to put on a video track.",
    );
    expect(dropAt(input({ row: { ...V1, locked: true } })).invalid).toMatch(
      /locked/,
    );
    const blocked = dropAt(input({ x: 1200, noSnap: true }));
    expect(blocked.invalid).toMatch(/clip in the way/);
    expect(blocked.edits).toEqual([]);
    expect(dropAt(input({ extents: [] })).invalid).toMatch(/not known/);
  });

  it("converts a stream's ticks to sequence frames exactly", () => {
    const [video] = EXTENTS;
    if (!video) throw new Error("fixture");
    expect(extentFrames(video, { num: 30000, den: 1001 })).toBe(60);
    expect(extentFrames(video, { num: 25, den: 1 })).toBe(50);
  });

  it("reads what it needs of each source from the project", () => {
    expect(droppedAssets(null, [1])).toEqual([]);
  });
});
