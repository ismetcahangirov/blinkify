import { describe, expect, it } from "vitest";

import { formatTimecode } from "./timecode.js";

const THIRTY = { num: 30, den: 1 };
const NTSC = { num: 30000, den: 1001 };

describe("formatTimecode", () => {
  // The same cases as `timecode_is_hours_minutes_seconds_frames` in
  // crates/blinkify-engine/src/playback/timecode.rs: one layout, two languages.
  it("lays a frame number out as hours, minutes, seconds, frames", () => {
    expect(formatTimecode(0, THIRTY)).toBe("00:00:00:00");
    expect(formatTimecode(119, THIRTY)).toBe("00:00:03:29");
    expect(formatTimecode(120, THIRTY)).toBe("00:00:04:00");
    expect(formatTimecode(30 * 3661 + 7, THIRTY)).toBe("01:01:01:07");
    expect(formatTimecode(30, NTSC)).toBe("00:00:01:00");
    expect(formatTimecode(-5, THIRTY)).toBe("00:00:00:00");
  });
});
