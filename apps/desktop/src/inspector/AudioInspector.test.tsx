import type { GainAdvice } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { usePreviewStore } from "../player/preview.store.js";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { AudioInspector } from "./AudioInspector.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const ADVICE: GainAdvice = {
  before: { integratedLufs: -26, rangeLu: 4, truePeakDbtp: -9, seconds: 10 },
  gainDb: 12,
  ceilingDbtp: -1,
  limitingDb: 4,
  targetLufs: -16,
  suggestedDb: 10,
  suggestedLimitingDb: 2,
};

const clip = (id: number, audio: Placement["audio"]): Placement =>
  ({
    clip: id,
    kind: "video",
    sourceIn: 0,
    sourceOut: 90_000,
    audio,
    silent: false,
  }) as unknown as Placement;

const edit = vi.fn(() => Promise.resolve(null));

beforeEach(() => {
  invoked.mockReset();
  edit.mockClear();
  invoked.mockImplementation((command: string) =>
    Promise.resolve(command === "gain_advice" ? ADVICE : undefined),
  );
  useProjectStore.setState({
    edit,
    view: { project: { sequence: {} } } as never,
  });
  usePreviewStore.setState({ session: null });
});

describe("the audio section", () => {
  it("states that the video stream is copied, at the point of use", () => {
    render(<AudioInspector clips={[clip(1, [])]} />);
    expect(screen.getByText(/video stream is still copied/)).toBeTruthy();
  });

  it("shows how far the limiter turns the peaks down, from the engine", async () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "gain", db: 12, ceilingDbtp: -1, bypassed: false }]),
        ]}
      />,
    );
    expect(await screen.findByText(/down by up to 4\.0 dB/)).toBeTruthy();
    expect(invoked).toHaveBeenCalledWith("gain_advice", { clip: 1 });
  });

  it("applies the suggestion only when asked", async () => {
    render(<AudioInspector clips={[clip(1, [])]} />);
    const apply = await screen.findByRole("button", { name: "Apply" });
    expect(edit).not.toHaveBeenCalled();
    fireEvent.click(apply);
    expect(edit).toHaveBeenCalledWith({
      edit: "set-audio",
      clips: [1],
      step: { op: "gain", db: 10, ceilingDbtp: -1, bypassed: false },
    });
  });

  it("shows differing gains as mixed and asks nothing of the engine", async () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "gain", db: 3, ceilingDbtp: -1, bypassed: false }]),
          clip(2, []),
        ]}
      />,
    );
    expect(screen.getAllByText("Mixed").length).toBeGreaterThan(0);
    expect(screen.getByText(/Select one clip/)).toBeTruthy();
    await waitFor(() => {
      expect(invoked).not.toHaveBeenCalledWith("gain_advice", {
        clip: 1,
      });
    });
  });

  it("resets the whole chain of every selected clip", () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "gain", db: 3, ceilingDbtp: -1, bypassed: false }]),
          clip(2, [{ op: "denoise", strength: 0.5, bypassed: false }]),
        ]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Reset audio" }));
    expect(edit).toHaveBeenCalledWith({
      edit: "reset-audio",
      clips: [1, 2],
      stage: null,
    });
  });

  it("bypasses the whole chain of every selected clip as one edit", () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "gain", db: 3, ceilingDbtp: -1, bypassed: false }]),
          clip(2, [{ op: "denoise", strength: 0.5, bypassed: true }]),
        ]}
      />,
    );
    const whole = screen.getByRole("switch", {
      name: "Bypass the whole chain",
    });
    expect(whole.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(whole);
    expect(edit).toHaveBeenCalledWith({
      edit: "bypass-audio",
      clips: [1, 2],
      bypassed: true,
    });
  });

  it("shows the whole chain bypassed only when every step is", () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "gain", db: 3, ceilingDbtp: -1, bypassed: true }]),
          clip(2, [{ op: "denoise", strength: 0.5, bypassed: true }]),
        ]}
      />,
    );
    expect(
      screen
        .getByRole("switch", { name: "Bypass the whole chain" })
        .getAttribute("aria-checked"),
    ).toBe("true");
  });

  it("shows differing noise reductions as mixed, and sets one for all", async () => {
    render(
      <AudioInspector
        clips={[
          clip(1, [{ op: "denoise", strength: 0.5, bypassed: false }]),
          clip(2, [{ op: "denoise", strength: 0.9, bypassed: false }]),
        ]}
      />,
    );
    expect(screen.getAllByText("Mixed").length).toBeGreaterThan(0);
    const strength = screen.getByRole("slider", { name: "Strength" });
    fireEvent.keyDown(strength, { key: "End" });
    await waitFor(() => {
      expect(edit).toHaveBeenCalledWith({
        edit: "set-audio",
        clips: [1, 2],
        step: { op: "denoise", strength: 1, bypassed: false },
      });
    });
  });

  it("shows the level after the chain while a preview is open", () => {
    usePreviewStore.setState({ session: 3 });
    render(<AudioInspector clips={[clip(1, [])]} />);
    expect(screen.getByText("Output, after the chain")).toBeTruthy();
    expect(screen.getByTestId("level-meter")).toBeTruthy();
  });
});
