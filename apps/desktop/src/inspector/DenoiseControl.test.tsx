import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { DenoiseControl } from "./DenoiseControl.js";
import { denoiseOf, formatStrength, setDenoise } from "./denoise.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const clip = (id: number, audio: Placement["audio"]): Placement =>
  ({ clip: id, audio }) as unknown as Placement;

const edit = vi.fn(() => Promise.resolve(null));

beforeEach(() => {
  edit.mockClear();
  useProjectStore.setState({ edit });
});

describe("noise reduction in the inspector", () => {
  it("reads the clip's step, or none", () => {
    expect(denoiseOf(clip(1, []))).toEqual({ strength: 0, bypassed: false });
    expect(
      denoiseOf(clip(1, [{ op: "denoise", strength: 0.4, bypassed: true }])),
    ).toEqual({ strength: 0.4, bypassed: true });
    expect(formatStrength(0.456)).toBe("46 %");
    expect(setDenoise([2], { strength: 0.5, bypassed: false })).toEqual({
      edit: "set-audio",
      clips: [2],
      step: { op: "denoise", strength: 0.5, bypassed: false },
    });
  });

  it("starts noise reduction at a blend, not at full strength", () => {
    render(<DenoiseControl clips={[clip(1, [])]} />);
    fireEvent.click(screen.getByRole("button", { name: "Reduce noise" }));
    expect(edit).toHaveBeenCalledWith({
      edit: "set-audio",
      clips: [1],
      step: { op: "denoise", strength: 0.7, bypassed: false },
    });
  });

  it("compares with the original by bypassing, keeping the strength", () => {
    render(
      <DenoiseControl
        clips={[clip(1, [{ op: "denoise", strength: 0.6, bypassed: false }])]}
      />,
    );
    fireEvent.click(screen.getByRole("switch", { name: "Hear original" }));
    expect(edit).toHaveBeenCalledWith({
      edit: "set-audio",
      clips: [1],
      step: { op: "denoise", strength: 0.6, bypassed: true },
    });
  });

  it("says it is a speech denoiser", () => {
    render(<DenoiseControl clips={[clip(1, [])]} />);
    expect(screen.getByText(/speech denoiser/)).toBeTruthy();
    expect(screen.getByText(/On music/)).toBeTruthy();
  });
});
