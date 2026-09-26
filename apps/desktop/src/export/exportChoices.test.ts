import type { ExportPlan, Segment } from "@blinkify/types";
import { describe, expect, it } from "vitest";

import {
  PRESERVE,
  PRESETS,
  audioChoicesFor,
  containerOf,
  cutLine,
  encodingName,
  picturesLine,
  planSummary,
  preservingContainer,
  shift,
  sizeLine,
  soundLine,
  withContainer,
} from "./exportChoices.js";

const RATE = { num: 30, den: 1 };

function segment(
  media: "video" | "audio",
  tier: Segment["tier"],
  extra: Partial<Segment> = {},
): Segment {
  return {
    media,
    start: 0,
    length: 30,
    sources: [],
    tier,
    causes: [],
    windows: [],
    ...extra,
  };
}

function plan(segments: Segment[], lossless: boolean): ExportPlan {
  return {
    timeBase: { num: 1, den: 30 },
    sequence: {
      width: 1920,
      height: 1080,
      frameRate: RATE,
      pixelAspect: { num: 1, den: 1 },
      colour: "sdr",
    },
    length: 30,
    segments,
    summary: {
      video: { copied: 30, reEncoded: lossless ? 0 : 5 },
      audio: { copied: 30, reEncoded: 0 },
      lossless,
      exportable: segments.every((s) => s.decline === undefined),
    },
  };
}

describe("presets", () => {
  it("preserve the source in its own container, with lossless sound", () => {
    const preserve = PRESETS.find((p) => p.id === PRESERVE);
    expect(preserve?.container).toBe("source");
    expect(preserve?.audio).toBe("flac");
    expect(preservingContainer("C:\\Clips\\trip.MOV")).toBe("mov");
    expect(preservingContainer("C:\\Clips\\trip.mp4")).toBe("mp4");
    // WebM cannot hold lossless sound; Matroska holds the same packets.
    expect(preservingContainer("C:\\Clips\\trip.webm")).toBe("mkv");
    expect(preservingContainer(undefined)).toBe("mp4");
  });

  it("offer only the sound a container can hold, and always name it", () => {
    expect(audioChoicesFor("webm").map((c) => c.target.codec)).toEqual([
      "opus",
    ]);
    expect(audioChoicesFor("mp4").some((c) => c.target.codec === "pcm")).toBe(
      false,
    );
    expect(audioChoicesFor("mkv").filter((c) => c.lossless)).toHaveLength(2);
    for (const preset of PRESETS) {
      const container =
        preset.container === "source" ? "mp4" : preset.container;
      expect(
        audioChoicesFor(container).some((c) => c.id === preset.audio),
      ).toBe(true);
    }
  });

  it("name the container by the extension, and change it in place", () => {
    expect(containerOf("D:\\out\\a.M4V")).toBe("mp4");
    expect(containerOf("D:\\out\\a.avi")).toBeNull();
    expect(withContainer("D:\\out.v2\\trip.mp4", "mkv")).toBe(
      "D:\\out.v2\\trip.mkv",
    );
    expect(withContainer("D:\\out.v2\\trip", "mov")).toBe(
      "D:\\out.v2\\trip.mov",
    );
  });
});

describe("the verdict in words", () => {
  it("claims pictures and sound separately", () => {
    expect(
      picturesLine({ lossless: true, copiedSeconds: 10, reEncodedSeconds: 0 }),
    ).toBe("Pictures: copied bit for bit");
    expect(
      picturesLine({
        lossless: false,
        copiedSeconds: 9.6,
        reEncodedSeconds: 0.4,
      }),
    ).toBe(
      "Pictures: 0.40 s of 10.00 s re-encoded, the rest copied bit for bit",
    );
    expect(
      soundLine(
        { lossless: false, copiedSeconds: 0, reEncodedSeconds: 10 },
        {
          codec: "flac",
          encoder: "flac",
          kilobits: null,
          sampleRate: 48000,
          channels: 2,
          matchesCopied: false,
          lossless: true,
        },
      ),
    ).toBe("Sound: 10.00 s re-encoded as FLAC, lossless, the rest copied");
    expect(
      encodingName({
        codec: "aac",
        encoder: "aac",
        kilobits: 256,
        sampleRate: 48000,
        channels: 2,
        matchesCopied: true,
        lossless: false,
      }),
    ).toBe("AAC 256 kb/s");
  });

  it("states how far each snapped cut moves", () => {
    expect(shift(0.4)).toBe("0.40 s later");
    expect(shift(-1.2)).toBe("1.20 s earlier");
    expect(shift(0.001)).toBe("unchanged");
    expect(
      cutLine(
        {
          clip: 3,
          atSeconds: 2,
          inShiftSeconds: -0.4,
          outShiftSeconds: 0.2,
        },
        RATE,
      ),
    ).toBe("Clip at 00:00:02:00: start 0.40 s earlier, end 0.20 s later");
  });

  it("says whether the size is an estimate", () => {
    expect(sizeLine({ bytes: 1_200_000_000, precise: true })).toBe(
      "About 1.2 GB",
    );
    expect(sizeLine({ bytes: 1_200_000_000, precise: false })).toMatch(
      /^Roughly 1.2 GB \(an estimate/,
    );
  });
});

describe("the application bar's summary", () => {
  it("is the plan's, and never lossless when the plan is not", () => {
    expect(
      planSummary(plan([segment("video", { tier: "stream-copy" })], true)),
    ).toEqual({ text: "Lossless", state: "lossless" });
    const seam = segment("video", {
      tier: "smart-cut",
      reason: "in-point-not-keyframe-aligned",
    });
    const whole = segment("audio", {
      tier: "full-re-encode",
      reason: "audio-filter",
    });
    expect(planSummary(plan([seam, seam], false))).toEqual({
      text: "2 seams",
      state: "seams",
    });
    expect(planSummary(plan([seam, whole], false))).toEqual({
      text: "1 seam · 1 segment re-encoded",
      state: "re-encoded",
    });
    expect(
      planSummary(
        plan(
          [
            segment(
              "video",
              { tier: "smart-cut", reason: "in-point-not-keyframe-aligned" },
              { decline: { decline: "hdr-would-be-rendered" } },
            ),
          ],
          false,
        ),
      ),
    ).toEqual({ text: "1 cut cannot export", state: "declined" });
  });
});
