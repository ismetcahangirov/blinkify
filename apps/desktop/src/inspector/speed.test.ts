import type { SpeedVerdict } from "@blinkify/types";
import { describe, expect, it } from "vitest";
import { shared } from "./mixedValue.js";
import {
  SLIDER_MAX,
  SLIDER_MIN,
  SLIDER_STEP,
  factorToSlider,
  formatFactor,
  formatRate,
  sameRatio,
  sliderToFactor,
  speedRatio,
  speedStatement,
  speedWarnings,
} from "./speed.js";

const copied = (rate: number): SpeedVerdict => ({
  speed: { num: 2, den: 1 },
  outputFrameRate: { num: rate, den: 1 },
  variableFrameRate: false,
  tier: { tier: "stream-copy" },
  problems: [],
});

const tooFast: SpeedVerdict = {
  speed: { num: 4, den: 1 },
  outputFrameRate: { num: 480, den: 1 },
  variableFrameRate: false,
  tier: {
    tier: "full-re-encode",
    reason: "speed-frame-rate-outside-container",
  },
  problems: [
    { problem: "frame-rate-outside-container", rate: { num: 480, den: 1 } },
  ],
};

describe("speed ratios", () => {
  it("sends a ratio in lowest terms on the hundredths grid", () => {
    expect(speedRatio(2)).toEqual({ num: 2, den: 1 });
    expect(speedRatio(1.5)).toEqual({ num: 3, den: 2 });
    expect(speedRatio(0.25)).toEqual({ num: 1, den: 4 });
    expect(speedRatio(1.234)).toEqual({ num: 123, den: 100 });
  });

  it("clamps to the engine's range of 0.1× to 100×", () => {
    expect(speedRatio(0.01)).toEqual({ num: 1, den: 10 });
    expect(speedRatio(1000)).toEqual({ num: 100, den: 1 });
  });

  it("gives the slider and the typed factor the same ratio for every position", () => {
    // The slider readout is what a user would type; typing it must send
    // what the slider sent.
    for (
      let position = SLIDER_MIN;
      position <= SLIDER_MAX;
      position += SLIDER_STEP
    ) {
      const fromSlider = speedRatio(sliderToFactor(position));
      const typed = Number.parseFloat(
        formatFactor(sliderToFactor(position)).replace("×", ""),
      );
      expect(speedRatio(typed)).toEqual(fromSlider);
    }
  });

  it("puts normal speed, half and double evenly on the slider", () => {
    expect(factorToSlider(1)).toBe(0);
    expect(factorToSlider(2)).toBeCloseTo(1);
    expect(factorToSlider(0.5)).toBeCloseTo(-1);
  });

  it("compares ratios exactly", () => {
    expect(sameRatio({ num: 2, den: 4 }, { num: 1, den: 2 })).toBe(true);
    expect(sameRatio({ num: 1, den: 3 }, { num: 1, den: 2 })).toBe(false);
  });
});

describe("the speed statement", () => {
  it("phrases the engine's verdict, never deciding one", () => {
    expect(speedStatement([copied(60)])?.tone).toBe("lossless");
    const reencoded = speedStatement([tooFast]);
    expect(reencoded?.tone).toBe("re-encode");
    expect(reencoded?.text).toContain("480 fps");
    expect(speedStatement([copied(60), tooFast])).toEqual({
      tone: "mixed",
      text: "Mixed: 1 of 2 clips are copied at this speed; the rest are re-encoded.",
    });
    expect(speedStatement([])).toBeNull();
  });

  it("raises each warning once, before export", () => {
    const slow: SpeedVerdict = {
      ...copied(6),
      problems: [{ problem: "below-smooth-motion", rate: { num: 6, den: 1 } }],
    };
    expect(speedWarnings([tooFast, tooFast, slow])).toEqual([
      "480 fps is not a frame rate a video file can carry.",
      "6 fps will stutter: Blinkify copies every recorded frame and does not invent new ones.",
    ]);
  });

  it("shows a rate exactly, with its decimal for reading", () => {
    expect(formatRate({ num: 60000, den: 1001 })).toBe(
      "60000/1001 fps (59.94)",
    );
    expect(formatRate({ num: 25, den: 1 })).toBe("25 fps");
  });
});

describe("the mixed-state rule", () => {
  it("shows a shared value, and mixed where the clips differ", () => {
    expect(shared([])).toEqual({ kind: "none" });
    expect(shared([3, 3])).toEqual({ kind: "same", value: 3 });
    expect(shared([3, 4])).toEqual({ kind: "mixed" });
    expect(
      shared(
        [
          { num: 1, den: 2 },
          { num: 2, den: 4 },
        ],
        sameRatio,
      ),
    ).toEqual({ kind: "same", value: { num: 1, den: 2 } });
  });
});
