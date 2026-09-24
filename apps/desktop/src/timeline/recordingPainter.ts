import type { Painter } from "./draw.js";

/**
 * A painter that records what was drawn instead of drawing it — how the
 * timeline tests and the benchmark see a frame without a browser.
 */
export class RecordingPainter implements Painter {
  fillStyle: Painter["fillStyle"] = "";
  strokeStyle: Painter["strokeStyle"] = "";
  lineWidth = 1;
  font = "";
  textBaseline: CanvasTextBaseline = "alphabetic";
  globalAlpha = 1;
  readonly calls: { op: string; args: unknown[]; style?: unknown }[] = [];
  private record(op: string, args: unknown[], style?: unknown) {
    this.calls.push({ op, args, style });
  }
  setTransform(...args: number[]) {
    this.record("setTransform", args);
  }
  clearRect(...args: number[]) {
    this.record("clearRect", args);
  }
  fillRect(...args: number[]) {
    this.record("fillRect", args, this.fillStyle);
  }
  strokeRect(...args: number[]) {
    this.record("strokeRect", args, this.strokeStyle);
  }
  beginPath() {}
  moveTo(...args: number[]) {
    this.record("moveTo", args, this.lineWidth);
  }
  lineTo(...args: number[]) {
    this.record("lineTo", args);
  }
  closePath() {}
  fill() {}
  stroke() {}
  rect() {}
  clip() {}
  save() {}
  restore() {}
  fillText(text: string, x: number, y: number) {
    this.record("fillText", [text, x, y]);
  }
  measureText(text: string) {
    return { width: text.length * 6 };
  }
  drawImage(...args: unknown[]) {
    this.record("drawImage", args);
  }
  of(op: string) {
    return this.calls.filter((call) => call.op === op);
  }
}
