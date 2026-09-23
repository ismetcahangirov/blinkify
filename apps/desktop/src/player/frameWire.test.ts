import { describe, expect, it } from "vitest";

import {
  parseWireFrame,
  WIRE_HEADER_BYTES,
  WireFormatError,
} from "./frameWire.js";

interface Fields {
  seq?: number;
  pts?: number;
  position?: number;
  frameNumber?: number;
  flags?: number;
  width?: number;
  height?: number;
}

/** A frame as the engine writes it — the same layout, built independently. */
function wire({
  seq = 1,
  pts = 0,
  position = 0,
  frameNumber = 0,
  flags = 0,
  width = 2,
  height = 1,
}: Fields = {}): ArrayBuffer {
  const buffer = new ArrayBuffer(WIRE_HEADER_BYTES + width * height * 4);
  const view = new DataView(buffer);
  new Uint8Array(buffer, 0, 4).set([0x42, 0x4b, 0x46, 0x32]); // "BKF2"
  view.setUint32(4, width, true);
  view.setUint32(8, height, true);
  view.setUint32(12, flags, true);
  view.setBigUint64(16, BigInt(seq), true);
  view.setBigInt64(24, BigInt(pts), true);
  view.setBigInt64(32, BigInt(position), true);
  view.setBigInt64(40, BigInt(frameNumber), true);
  new Uint8Array(buffer, WIRE_HEADER_BYTES).fill(7);
  return buffer;
}

describe("parseWireFrame", () => {
  it("reads the header and views the pixels without copying them", () => {
    const buffer = wire({
      seq: 5,
      pts: -1024,
      position: 1_033_334,
      frameNumber: 31,
      width: 3,
      height: 2,
    });
    const frame = parseWireFrame(buffer);
    expect(frame).toMatchObject({
      seq: 5,
      pts: -1024,
      position: 1_033_334,
      frameNumber: 31,
      rotation: 0,
      black: false,
      width: 3,
      height: 2,
    });
    expect(frame.pixels.length).toBe(3 * 2 * 4);
    expect(frame.pixels.buffer).toBe(buffer);
    expect(frame.pixels.every((byte) => byte === 7)).toBe(true);
  });

  it("reads the rotation as quarter turns and a gap as black", () => {
    expect(parseWireFrame(wire({ flags: 1 })).rotation).toBe(90);
    expect(parseWireFrame(wire({ flags: 3 })).rotation).toBe(270);
    const gap = parseWireFrame(wire({ flags: 4, width: 0, height: 0 }));
    expect(gap.black).toBe(true);
    expect(gap.pixels.length).toBe(0);
  });

  it("rejects a body that is not a frame", () => {
    const buffer = wire();
    new Uint8Array(buffer)[0] = 0;
    expect(() => parseWireFrame(buffer)).toThrow(WireFormatError);
    expect(() => parseWireFrame(new ArrayBuffer(8))).toThrow(WireFormatError);
  });

  it("rejects a frame whose pixels do not match its size", () => {
    const truncated = wire({ width: 4, height: 4 }).slice(
      0,
      WIRE_HEADER_BYTES + 10,
    );
    expect(() => parseWireFrame(truncated)).toThrow(/112 bytes/);
  });
});
