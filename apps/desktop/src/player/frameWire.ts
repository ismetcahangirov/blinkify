/**
 * The preview frame wire format.
 *
 * The engine writes it (`blinkify_engine::playback::ShownFrame::wire`) and
 * this reads it: a 48-byte little-endian header, then `width * height * 4`
 * bytes of RGBA. Binary end to end — a 1080p frame is 8 MB thirty times a
 * second, and base64 or JSON would inflate and parse every one of them (#27).
 *
 * | offset | size | field                                      |
 * | ------ | ---- | ------------------------------------------ |
 * | 0      | 4    | magic `BKF2`                               |
 * | 4      | 4    | width (0 for a black frame)                |
 * | 8      | 4    | height                                     |
 * | 12     | 4    | flags: bits 0–1 quarter turns, bit 2 black |
 * | 16     | 8    | sequence number                            |
 * | 24     | 8    | source presentation timestamp              |
 * | 32     | 8    | timeline position, microseconds            |
 * | 40     | 8    | timeline frame number                      |
 */

export const WIRE_HEADER_BYTES = 48;

/** `BKF2`, little-endian. */
const WIRE_MAGIC = 0x3246_4b42;

const FLAG_BLACK = 1 << 2;

export interface WireFrame {
  /** Increases by one for every frame the session presents. */
  seq: number;
  /** Presentation timestamp in the source's time base. */
  pts: number;
  /** Where the frame starts on the timeline, in microseconds. */
  position: number;
  /** The timeline frame number, for the timecode. */
  frameNumber: number;
  /** Counter-clockwise rotation to draw it with: 0, 90, 180 or 270. */
  rotation: number;
  /** A gap in the timeline: draw nothing. */
  black: boolean;
  width: number;
  height: number;
  /** A view onto the response body; no copy is made. */
  pixels: Uint8ClampedArray<ArrayBuffer>;
}

export class WireFormatError extends Error {
  override name = "WireFormatError";
}

export function parseWireFrame(buffer: ArrayBuffer): WireFrame {
  if (buffer.byteLength < WIRE_HEADER_BYTES) {
    throw new WireFormatError(
      `a frame is at least ${String(WIRE_HEADER_BYTES)} bytes, got ${String(buffer.byteLength)}`,
    );
  }
  const header = new DataView(buffer, 0, WIRE_HEADER_BYTES);
  if (header.getUint32(0, true) !== WIRE_MAGIC) {
    throw new WireFormatError("not a Blinkify frame");
  }
  const width = header.getUint32(4, true);
  const height = header.getUint32(8, true);
  const flags = header.getUint32(12, true);
  const expected = WIRE_HEADER_BYTES + width * height * 4;
  if (buffer.byteLength !== expected) {
    throw new WireFormatError(
      `a ${String(width)}x${String(height)} frame is ${String(expected)} bytes, got ${String(buffer.byteLength)}`,
    );
  }
  return {
    // Every field fits a double exactly: a session would have to run for
    // millennia to exceed 2^53 of any of them.
    seq: Number(header.getBigUint64(16, true)),
    pts: Number(header.getBigInt64(24, true)),
    position: Number(header.getBigInt64(32, true)),
    frameNumber: Number(header.getBigInt64(40, true)),
    rotation: (flags & 0b11) * 90,
    black: (flags & FLAG_BLACK) !== 0,
    width,
    height,
    pixels: new Uint8ClampedArray(buffer, WIRE_HEADER_BYTES),
  };
}
