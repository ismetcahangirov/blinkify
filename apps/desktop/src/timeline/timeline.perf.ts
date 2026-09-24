/**
 * The timeline frame-rate benchmark (#33), run by `pnpm bench:timeline` on
 * the heavy workflow — never on a pull request.
 *
 * A hosted runner has no GPU and a variable load, so a hard 60 fps assertion
 * there would flake, and a flaky gate is disabled within a month. What this
 * measures is what the renderer controls: the JavaScript cost of drawing the
 * content layer of a 200-clip project while a clip is dragged, one frame per
 * step, through a painter that does no pixel work. The floor asserted is
 * generous — a genuine regression, such as losing virtualisation, blows
 * through it — and the real figure on real hardware is measured separately
 * and recorded in `docs/architecture/timeline-rendering.md`.
 */
import type { Placement as EvaluatedPlacement } from "@blinkify/types";
import { appendFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  drawContent,
  drawOverlay,
  TextCache,
  type MediaSource,
  type Painter,
  type Scene,
  type TrackRow,
} from "./draw.js";

/** Milliseconds a frame may cost here before it is a regression. A 60 fps
 * frame is 16.7 ms for everything; the content layer's script must stay a
 * small part of it. */
const P95_FLOOR_MS = 6;

const FRAMES = 600;

class NullPainter implements Painter {
  fillStyle: Painter["fillStyle"] = "";
  strokeStyle: Painter["strokeStyle"] = "";
  lineWidth = 1;
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  calls = 0;
  setTransform() {
    this.calls++;
  }
  clearRect() {
    this.calls++;
  }
  fillRect() {
    this.calls++;
  }
  strokeRect() {
    this.calls++;
  }
  beginPath() {}
  moveTo() {
    this.calls++;
  }
  lineTo() {
    this.calls++;
  }
  closePath() {}
  fill() {}
  stroke() {}
  rect() {}
  clip() {}
  save() {}
  restore() {}
  fillText() {
    this.calls++;
  }
  measureText(text: string) {
    return { width: text.length * 6 };
  }
  drawImage() {
    this.calls++;
  }
}

function placement(
  clip: number,
  start: number,
  kind: "video" | "audio",
): EvaluatedPlacement {
  return {
    track: 1,
    kind,
    clip,
    source: clip % 7,
    stream: kind === "video" ? 0 : 1,
    timeBase: { num: 1, den: 90_000 },
    sourceIn: 0,
    sourceOut: 90_000 * 4,
    start,
    length: 120,
    speed: { num: 1, den: 1 },
    audio: [],
    sequenceTimeBase: { num: 1, den: 30 },
  };
}

/** 200 clips: 100 on the video track, 100 on the audio track. */
function rows(dragged: number): TrackRow[] {
  const track = (id: number, kind: "video" | "audio", top: number) => ({
    id,
    kind,
    top,
    height: kind === "video" ? 56 : 40,
    placements: Array.from({ length: 100 }, (_, i) =>
      placement(id * 1000 + i, i * 130 + (i === 20 ? dragged : 0), kind),
    ),
  });
  return [track(1, "video", 0), track(2, "audio", 56)];
}

const media: MediaSource = {
  tile: () => ({ image: {} as CanvasImageSource, sx: 0, sy: 0, sw: 16, sh: 9 }),
  peaks: (_source, _stream, _seconds, pixels) =>
    new Int16Array(pixels * 2).fill(12_000),
};

function percentile(values: number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return (
    sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0
  );
}

describe("the timeline benchmark", () => {
  it("draws a 200-clip project while a clip is dragged, within the floor", () => {
    const painter = new NullPainter();
    const text = new TextCache();
    const labels = new Map(
      Array.from({ length: 7 }, (_, i) => [i, `clip-${i}-from-the-phone.mp4`]),
    );
    const scene = (frame: number): Scene => ({
      // Zoomed so a realistic stretch is on screen: 1920 px, about 60 clips.
      view: { scale: 1, origin: 1000, scrollTop: 0, width: 1920, height: 320 },
      dpr: 1.5,
      rulerHeight: 24,
      rows: rows(frame % 200),
      frameRate: { num: 30, den: 1 },
      selection: new Set([1020]),
      ineligible: new Set([1021, 2005]),
      labels,
      theme: {
        background: "a",
        track: "b",
        border: "c",
        clipVideo: "d",
        clipVideoBorder: "e",
        clipAudio: "f",
        clipAudioBorder: "g",
        clipLabel: "h",
        selected: "i",
        playhead: "j",
        rulerText: "k",
        warning: "l",
        font: "11px sans-serif",
      },
      media,
      text,
    });

    const times: number[] = [];
    for (let frame = 0; frame < FRAMES; frame++) {
      const current = scene(frame);
      const start = performance.now();
      drawContent(painter, current);
      drawOverlay(painter, current, 1000 + frame);
      times.push(performance.now() - start);
    }
    const warm = times.slice(60);
    const mean = warm.reduce((sum, t) => sum + t, 0) / warm.length;
    const p95 = percentile(warm, 0.95);
    const summary = `Timeline benchmark (#33): 200 clips, ${FRAMES} drag frames — mean ${mean.toFixed(3)} ms, p95 ${p95.toFixed(3)} ms per frame of script (floor ${P95_FLOOR_MS} ms), ${Math.round(painter.calls / FRAMES)} draw calls a frame.`;
    process.stdout.write(`${summary}\n`);
    if (process.env.GITHUB_STEP_SUMMARY)
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, `- ${summary}\n`);
    expect(p95).toBeLessThan(P95_FLOOR_MS);
  });
});
