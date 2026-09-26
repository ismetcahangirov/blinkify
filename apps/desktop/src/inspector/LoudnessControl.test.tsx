import type { LoudnessReport } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { LoudnessControl } from "./LoudnessControl.js";
import {
  describeLoudness,
  presetOf,
  reportStatement,
  setSequenceLoudness,
} from "./loudness.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const REPORT: LoudnessReport = {
  before: {
    integratedLufs: -25.2,
    rangeLu: 6.1,
    truePeakDbtp: -3.4,
    seconds: 20,
  },
  after: {
    integratedLufs: -14.05,
    rangeLu: 6.0,
    truePeakDbtp: -1.1,
    seconds: 20,
  },
  targetLufs: -14,
  ceilingDbtp: -1,
  gainDb: 11.6,
};

const clip = (id: number, audio: Placement["audio"]): Placement =>
  ({ clip: id, audio, sourceIn: 0, sourceOut: 100 }) as unknown as Placement;

const edit = vi.fn(() => Promise.resolve(null));

beforeEach(() => {
  invoked.mockReset();
  edit.mockClear();
  invoked.mockImplementation(() => Promise.resolve(REPORT));
  useProjectStore.setState({
    edit,
    view: { project: { sequence: {} } } as never,
  });
});

describe("loudness in the inspector", () => {
  it("names the platform a target belongs to", () => {
    expect(presetOf({ targetLufs: -23, ceilingDbtp: -1 })).toBe("broadcast");
    expect(presetOf({ targetLufs: -18, ceilingDbtp: -1 })).toBe("custom");
    expect(setSequenceLoudness(null)).toEqual({
      edit: "set-sequence-loudness",
      loudness: null,
    });
  });

  it("states a measurement in LUFS, and silence as below the gate", () => {
    expect(describeLoudness(REPORT.before)).toBe(
      "−25.2 LUFS, range 6.1 LU, peak −3.4 dBTP",
    );
    expect(
      describeLoudness({
        integratedLufs: null,
        rangeLu: null,
        truePeakDbtp: null,
        seconds: 1,
      }),
    ).toMatch(/too quiet/);
    expect(reportStatement(REPORT)).toMatch(/One gain of \+11\.6 dB/);
  });

  it("starts a clip's normalisation at the streaming target", () => {
    render(<LoudnessControl clips={[clip(1, [])]} />);
    fireEvent.click(screen.getByRole("button", { name: "Normalise" }));
    expect(edit).toHaveBeenCalledWith({
      edit: "set-audio",
      clips: [1],
      step: {
        op: "normalise",
        targetLufs: -14,
        ceilingDbtp: -1,
        bypassed: false,
      },
    });
  });

  it("shows a normalised clip's loudness before and after", async () => {
    render(
      <LoudnessControl
        clips={[
          clip(1, [
            {
              op: "normalise",
              targetLufs: -14,
              ceilingDbtp: -1,
              bypassed: false,
            },
          ]),
        ]}
      />,
    );
    expect(await screen.findByText(/−14\.1 LUFS/)).toBeTruthy();
    expect(screen.getByText(/−25\.2 LUFS/)).toBeTruthy();
    expect(invoked).toHaveBeenCalledWith("loudness_report", { clip: 1 });
  });

  it("measures the whole mix only when asked", async () => {
    useProjectStore.setState({
      view: {
        project: {
          sequence: { loudness: { targetLufs: -16, ceilingDbtp: -1 } },
        },
      } as never,
    });
    render(<LoudnessControl clips={[clip(1, [])]} />);
    expect(invoked).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Measure the mix" }));
    expect(invoked).toHaveBeenCalledWith("sequence_loudness_report", {});
    expect(await screen.findByText(/One gain/)).toBeTruthy();
  });
});
