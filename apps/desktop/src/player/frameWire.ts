/**
 * The preview frame wire format.
 *
 * The engine writes it (`blinkify_engine::decode::wire_frame`) and this reads
 * it: a 32-byte little-endian header, then `width * height * 4` bytes of RGBA.
 * Binary end to end — a 1080p frame is 8 MB thirty times a second, and base64
 * or JSON would inflate and parse every one of them (#27).
 */

export const WIRE_HEADER_BYTES = 32;

/** `BKF1`, little-endian. */
const WIRE_MAGIC = 0x3146_4b42;

export interface WireFrame {
  /** Increases by one for every frame the session presents. */
  seq: number;
  /** Presentation timestamp, in the session's time base. */
  pts: number;
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
  const expected = WIRE_HEADER_BYTES + width * height * 4;
  if (buffer.byteLength !== expected) {
    throw new WireFormatError(
      `a ${String(width)}x${String(height)} frame is ${String(expected)} bytes, got ${String(buffer.byteLength)}`,
    );
  }
  return {
    // Sequence numbers and timestamps fit a double exactly: a session would
    // have to run for millennia to exceed 2^53 of either.
    seq: Number(header.getBigUint64(16, true)),
    pts: Number(header.getBigInt64(24, true)),
    width,
    height,
    pixels: new Uint8ClampedArray(buffer, WIRE_HEADER_BYTES),
  };
}
