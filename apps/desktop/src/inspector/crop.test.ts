import type { ClipsCost, CropFrame } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import {
  costStatement,
  presetOf,
  shownRect,
  sideEdit,
  sideOf,
} from "./crop.js";

/** A 1920 × 1080 picture as the engine describes it. */
const HD: CropFrame = {
  width: 1920,
  height: 1080,
  across: 2,
  down: 2,
  minSize: 16,
  presets: [
    { aspect: "source", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "16:9", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "9:16", rect: { x: 656, y: 0, width: 608, height: 1080 } },
    { aspect: "1:1", rect: { x: 420, y: 0, width: 1080, height: 1080 } },
    { aspect: "4:5", rect: { x: 528, y: 0, width: 864, height: 1080 } },
  ],
};

const COPIED: ClipsCost = {
  clips: 1,
  croppedClips: 0,
  croppedSeconds: 0,
  otherSeconds: 0,
  otherReasons: [],
  seamedSeconds: 0,
  copiedSeconds: 4.2,
  declined: false,
  sound: { copiedSeconds: 4.2, reEncodedSeconds: 0, reasons: [] },
};

const CROPPED: ClipsCost = {
  ...COPIED,
  croppedClips: 1,
  croppedSeconds: 4.2,
  copiedSeconds: 0,
};

describe("the crop's wording", () => {
  it("states a crop's cost as #130 words it, and the copy after a reset", () => {
    expect(costStatement(CROPPED)).toEqual({
      text: "Cropping re-encodes this clip's pictures (4.2 s). Its sound is still copied.",
      tone: "re-encode",
    });
    expect(costStatement(COPIED)).toEqual({
      text: "Not cropped: this clip's pictures are copied bit for bit.",
      tone: "lossless",
    });
  });

  it("says so when the sound is re-encoded for its own reasons", () => {
    const text = costStatement({
      ...CROPPED,
      sound: { copiedSeconds: 0, reEncodedSeconds: 4.2, reasons: ["Gain."] },
    })?.text;
    expect(text).toContain("not because of the crop");
  });

  it("names another reason the pictures are re-encoded anyway", () => {
    const reversed = costStatement({
      ...COPIED,
      copiedSeconds: 0,
      otherSeconds: 4.2,
      otherReasons: [
        "The clip plays backwards, so its pictures are re-encoded.",
      ],
    });
    expect(reversed?.tone).toBe("re-encode");
    expect(reversed?.text).toContain(
      "re-encoded anyway: The clip plays backwards",
    );
  });

  it("counts several clips, and those of them cropped", () => {
    expect(
      costStatement({
        ...CROPPED,
        clips: 3,
        croppedClips: 2,
        croppedSeconds: 8.4,
        sound: { copiedSeconds: 12.6, reEncodedSeconds: 0, reasons: [] },
      })?.text,
    ).toBe(
      "Cropping re-encodes the pictures of 2 of 3 clips (8.4 s). Their sound is still copied.",
    );
    expect(costStatement({ ...COPIED, clips: 0 })).toBeNull();
  });

  it("does not claim a seam is bit for bit everywhere", () => {
    const seamed = costStatement({
      ...COPIED,
      copiedSeconds: 0,
      seamedSeconds: 4.2,
    });
    expect(seamed?.tone).toBe("neutral");
    expect(seamed?.text).toContain("apart from a fraction of a second");
  });

  it("reads which preset a rectangle is off the engine's rectangles", () => {
    expect(presetOf({ x: 656, y: 0, width: 608, height: 1080 }, HD)).toBe(
      "9:16",
    );
    // Moved, it is still 9:16.
    expect(presetOf({ x: 0, y: 0, width: 608, height: 1080 }, HD)).toBe("9:16");
    expect(presetOf({ x: 0, y: 0, width: 700, height: 1080 }, HD)).toBe("free");
    // The whole picture is the source's own shape, listed first.
    const whole = shownRect(undefined, HD);
    expect(whole && presetOf(whole, HD)).toBe("source");
    expect(shownRect(undefined, undefined)).toBeNull();
  });

  it("sends the side changed and nothing else", () => {
    const sent = sideEdit([1, 2], "width", 400);
    expect(sent).toEqual({
      edit: "set-crop-sides",
      clips: [1, 2],
      x: null,
      y: null,
      width: 400,
      height: null,
    });
    expect(sideOf(sent)).toBe("width");
    expect(sideOf({ edit: "reset-crop", clips: [1] })).toBeNull();
  });
});
