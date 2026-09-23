import { describe, expect, it } from "vitest";

import {
  parseWireFrame,
  WIRE_HEADER_BYTES,
  WireFormatError,
} from "./frameWire.js";

/** A frame as the engine writes it — the same layout, built independently. */
function wire(
  seq: number,
  pts: number,
  width: number,
  height: number,
  fill = 7,
): ArrayBuffer {
  const buffer = new ArrayBuffer(WIRE_HEADER_BYTES + width * height * 4);
  const view = new DataView(buffer);
  new Uint8Array(buffer, 0, 4).set([0x42, 0x4b, 0x46, 0x31]); // "BKF1"
  view.setUint32(4, width, true);
  view.setUint32(8, height, true);
  view.setBigUint64(16, BigInt(seq), true);
  view.setBigInt64(24, BigInt(pts), true);
  new Uint8Array(buffer, WIRE_HEADER_BYTES).fill(fill);
  return buffer;
}

describe("parseWireFrame", () => {
  it("reads the header and views the pixels without copying them", () => {
    const buffer = wire(5, -1024, 3, 2);
    const frame = parseWireFrame(buffer);
    expect(frame).toMatchObject({ seq: 5, pts: -1024, width: 3, height: 2 });
    expect(frame.pixels.length).toBe(3 * 2 * 4);
    expect(frame.pixels.buffer).toBe(buffer);
    expect(frame.pixels.every((byte) => byte === 7)).toBe(true);
  });

  it("rejects a body that is not a frame", () => {
    const buffer = wire(1, 0, 2, 2);
    new Uint8Array(buffer)[0] = 0;
    expect(() => parseWireFrame(buffer)).toThrow(WireFormatError);
    expect(() => parseWireFrame(new ArrayBuffer(8))).toThrow(WireFormatError);
  });

  it("rejects a frame whose pixels do not match its size", () => {
    const truncated = wire(1, 0, 4, 4).slice(0, WIRE_HEADER_BYTES + 10);
    expect(() => parseWireFrame(truncated)).toThrow(/64 bytes|got/);
  });
});
