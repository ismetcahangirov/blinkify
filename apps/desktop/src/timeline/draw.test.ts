import type { Placement as EvaluatedPlacement } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import {
  drawContent,
  drawOverlay,
  firstVisible,
  placementsIn,
  rulerLabel,
  TextCache,
  type MediaSource,
  type Scene,
  type Theme,
  type TrackRow,
} from "./draw.js";
import { RecordingPainter } from "./recordingPainter.js";

const THEME: Theme = {
  background: "bg",
  track: "track",
  border: "border",
  clipVideo: "video",
  clipVideoBorder: "video-edge",
  clipAudio: "audio",
  clipAudioBorder: "audio-edge",
  clipLabel: "label",
  selected: "selected",
  playhead: "playhead",
  rulerText: "ruler",
  warning: "warning",
  font: "11px sans-serif",
};

function placement(
  clip: number,
  start: number,
  length: number,
  kind: "video" | "audio" = "video",
): EvaluatedPlacement {
  return {
    track: 1,
    kind,
    clip,
    source: 1,
    stream: kind === "video" ? 0 : 1,
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

/** `count` clips of 30 frames with 10-frame gaps. */
function row(count: number, kind: "video" | "audio" = "video"): TrackRow {
  return {
    id: 1,
    kind,
    top: 0,
    height: 56,
    placements: Array.from({ length: count }, (_, i) =>
      placement(i + 1, i * 40, 30, kind),
    ),
  };
}

function scene(overrides: Partial<Scene> = {}): Scene {
  return {
    view: { scale: 2, origin: 0, scrollTop: 0, width: 800, height: 200 },
    dpr: 1,
    rulerHeight: 24,
    rows: [row(200)],
    frameRate: { num: 30, den: 1 },
    selection: new Set(),
    ineligible: new Set(),
    labels: new Map([[1, "beach.mp4"]]),
    theme: THEME,
    media: null,
    text: new TextCache(),
    ...overrides,
  };
}

const clipFills = (painter: RecordingPainter) =>
  painter.of("fillRect").filter((call) => call.style === "video");

describe("the timeline painter", () => {
  it("finds the first visible clip by binary search", () => {
    const placements = row(200).placements;
    expect(firstVisible(placements, 0)).toBe(0);
    expect(firstVisible(placements, 30)).toBe(1);
    expect(firstVisible(placements, 39)).toBe(1);
    expect(firstVisible(placements, 41)).toBe(1);
    expect(firstVisible(placements, 70)).toBe(2);
    expect(firstVisible(placements, 1e9)).toBe(200);
    expect(placementsIn(placements, 100, 200).map((p) => p.clip)).toEqual([
      3, 4, 5,
    ]);
  });

  it("draws only the clips in view, however many the project has", () => {
    // 800 px at 2 px per frame is 400 frames: clips 1 to 10.
    const painter = new RecordingPainter();
    drawContent(painter, scene());
    expect(clipFills(painter)).toHaveLength(10);

    const far = new RecordingPainter();
    drawContent(
      far,
      scene({
        view: { scale: 2, origin: 4000, scrollTop: 0, width: 800, height: 200 },
      }),
    );
    expect(clipFills(far)).toHaveLength(10);
    expect(clipFills(far)[0]?.args[0]).toBe(0);
  });

  it("skips tracks scrolled out of view", () => {
    const rows = [
      row(10),
      { ...row(10), id: 2, top: 56 },
      { ...row(10), id: 3, top: 112 },
    ];
    const painter = new RecordingPainter();
    drawContent(
      painter,
      scene({
        rows,
        view: { scale: 2, origin: 0, scrollTop: 60, width: 800, height: 70 },
      }),
    );
    // 46 px below the ruler, scrolled 60 down: rows 24–70 on screen hold
    // track 2 only; track 1 is above, track 3 below.
    expect(clipFills(painter)).toHaveLength(10);
  });

  it("marks the selection and the clips that cannot be copied", () => {
    const painter = new RecordingPainter();
    drawContent(
      painter,
      scene({ selection: new Set([2]), ineligible: new Set([3]) }),
    );
    const selected = painter
      .of("strokeRect")
      .filter((call) => call.style === "selected");
    expect(selected).toHaveLength(1);
    expect(selected[0]?.args[0]).toBeCloseTo(80 + 1, 5);
    const marks = painter
      .of("fillRect")
      .filter((call) => call.style === "warning");
    expect(marks).toHaveLength(1);
    expect(marks[0]?.args[0]).toBe(160);
  });

  it("keeps lines on device pixels at every display scaling", () => {
    for (const dpr of [1, 1.25, 1.5, 2]) {
      const painter = new RecordingPainter();
      drawContent(painter, scene({ dpr }));
      // A one-pixel line — vertical or horizontal — sits on a device
      // pixel's centre across its width.
      let checked = 0;
      painter.calls.forEach((call, index) => {
        const next = painter.calls[index + 1];
        if (call.op !== "moveTo" || call.style !== 1 / dpr) return;
        if (next?.op !== "lineTo") return;
        const [x0, y0] = call.args as number[];
        const [x1, y1] = next.args as number[];
        const across = x0 === x1 ? x0 : y0 === y1 ? y0 : undefined;
        if (across === undefined) return;
        const device = (across ?? 0) * dpr;
        expect(device - Math.floor(device)).toBeCloseTo(0.5, 6);
        checked += 1;
      });
      expect(checked).toBeGreaterThan(10);
      expect(painter.of("setTransform")[0]?.args).toEqual([
        dpr,
        0,
        0,
        dpr,
        0,
        0,
      ]);
    }
  });

  it("labels the ruler in frames close in and in time further out", () => {
    expect(rulerLabel(0, 30, false)).toBe("0:00");
    expect(rulerLabel(95, 30, true)).toBe("0:03:05");
    expect(rulerLabel(30 * 3725, 30, false)).toBe("1:02:05");
    const close = new RecordingPainter();
    drawContent(
      close,
      scene({
        view: { scale: 96, origin: 30, scrollTop: 0, width: 400, height: 200 },
      }),
    );
    expect(close.of("fillText").map((call) => call.args[0])).toContain(
      "0:01:01",
    );
  });

  it("cuts a label to fit, and measures it once per zoom", () => {
    const text = new TextCache();
    const painter = new RecordingPainter();
    text.forZoom(2);
    expect(text.fit(painter, "a-very-long-file-name.mp4", 64)).toBe(
      "a-very-lo…",
    );
    const measured = text.measured;
    text.fit(painter, "a-very-long-file-name.mp4", 64);
    expect(text.measured).toBe(measured);
    text.forZoom(3);
    text.fit(painter, "a-very-long-file-name.mp4", 64);
    expect(text.measured).toBeGreaterThan(measured);
    expect(text.fit(painter, "short", 64)).toBe("short");
    expect(text.fit(painter, "anything", 4)).toBe("");
  });

  it("draws thumbnails and waveforms from what the media source has", () => {
    const media: MediaSource = {
      tile: (_source, seconds) => ({
        image: {} as CanvasImageSource,
        sx: Math.round(seconds) * 10,
        sy: 0,
        sw: 16,
        sh: 9,
      }),
      peaks: (_source, _stream, _seconds, pixels) =>
        new Int16Array(pixels * 2).fill(16384),
    };
    const painter = new RecordingPainter();
    drawContent(
      painter,
      scene({ media, rows: [row(3), { ...row(3, "audio"), id: 2, top: 56 }] }),
    );
    expect(painter.of("drawImage").length).toBeGreaterThan(0);
    expect(painter.of("lineTo").length).toBeGreaterThan(3 * 60);
  });

  it("draws the playhead on its own layer, and nothing else", () => {
    const painter = new RecordingPainter();
    drawOverlay(painter, scene(), 50);
    expect(painter.of("fillRect")).toHaveLength(0);
    expect(painter.of("moveTo")[0]?.args[0]).toBeCloseTo(100.5, 6);
    const none = new RecordingPainter();
    drawOverlay(none, scene(), null);
    expect(none.of("moveTo")).toHaveLength(0);
  });
});
