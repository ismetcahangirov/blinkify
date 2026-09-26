import type { GainAdvice } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import type { Placement } from "../timeline/draw.js";
import {
  NO_GAIN,
  formatDb,
  gainOf,
  limiterStatement,
  measuredLevel,
  setGain,
  suggestionText,
} from "./gain.js";

const ADVICE: GainAdvice = {
  before: {
    integratedLufs: -26,
    rangeLu: 4,
    truePeakDbtp: -9,
    seconds: 10,
  },
  gainDb: 12,
  ceilingDbtp: -1,
  limitingDb: 4,
  targetLufs: -16,
  suggestedDb: 10,
  suggestedLimitingDb: 2,
};

const placement = (audio: Placement["audio"]): Placement =>
  ({ clip: 1, audio }) as unknown as Placement;

describe("clip gain in the inspector", () => {
  it("reads the clip's gain step, or no gain at the default ceiling", () => {
    expect(gainOf(placement([]))).toEqual(NO_GAIN);
    expect(
      gainOf(
        placement([
          { op: "denoise", strength: 0.5, bypassed: false },
          { op: "gain", db: 6, ceilingDbtp: -2, bypassed: true },
        ]),
      ),
    ).toEqual({ db: 6, ceilingDbtp: -2, bypassed: true });
  });

  it("sends one set-audio edit for every selected clip", () => {
    expect(
      setGain([3, 4], { db: 2.5, ceilingDbtp: -1, bypassed: false }),
    ).toEqual({
      edit: "set-audio",
      clips: [3, 4],
      step: { op: "gain", db: 2.5, ceilingDbtp: -1, bypassed: false },
    });
  });

  it("says when the limiter engages, and by how much", () => {
    expect(limiterStatement(ADVICE)).toBe(
      "The limiter is turning the loudest peaks down by up to 4.0 dB to hold them at −1.0 dBTP, rather than letting them clip.",
    );
    expect(limiterStatement({ ...ADVICE, limitingDb: 0 })).toBe(
      "The peaks stay under −1.0 dBTP: the limiter is not engaging.",
    );
    expect(
      limiterStatement({
        ...ADVICE,
        before: { ...ADVICE.before, truePeakDbtp: null },
      }),
    ).toMatch(/silent/);
  });

  it("offers the suggestion with its cost, and none for silence", () => {
    expect(suggestionText(ADVICE)).toBe(
      "+10.0 dB brings it to −16.0 LUFS, with the limiter turning peaks down by up to 2.0 dB.",
    );
    expect(suggestionText({ ...ADVICE, suggestedLimitingDb: 0 })).toBe(
      "+10.0 dB brings it to −16.0 LUFS.",
    );
    expect(suggestionText({ ...ADVICE, suggestedDb: null })).toMatch(
      /too quiet/,
    );
  });

  it("formats decibels with a sign and a typographic minus", () => {
    expect(formatDb(3)).toBe("+3.0");
    expect(formatDb(-0.04)).toBe("0.0");
    expect(formatDb(-12.25)).toBe("−12.3");
    expect(measuredLevel(ADVICE)).toBe("−26.0 LUFS, peak −9.0 dBTP");
  });
});
