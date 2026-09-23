import type { Diagnostics } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  describe as describeOperation,
  OperationsPanel,
} from "./OperationsPanel.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const AT: Diagnostics = {
  position: 45,
  clips: [
    {
      track: 1,
      clip: 3,
      source: 1,
      sourceName: "beach.mp4",
      sourceTick: 135_000,
      applied: [
        {
          operation: { op: "trim", from: 90_000, to: 180_000 },
          previewed: true,
        },
        {
          operation: { op: "speed", ratio: { num: 3, den: 2 } },
          previewed: true,
        },
        { operation: { op: "gain", db: -3 }, previewed: true },
        {
          operation: { op: "denoise", strength: 0.25 },
          previewed: false,
        },
        {
          operation: { op: "normalise", targetLufs: -16 },
          previewed: false,
        },
      ],
    },
  ],
};

beforeEach(() => {
  invoked.mockReset();
});

describe("the operations panel", () => {
  it("lists what the evaluator applies under the playhead, in order", async () => {
    invoked.mockResolvedValue(AT);
    render(<OperationsPanel session={4} />);
    const items = await screen.findAllByRole("listitem");
    expect(invoked).toHaveBeenCalledWith("operations_at", { session: 4 });
    expect(items.map((item) => item.textContent)).toEqual([
      "Trim: source 90000 to 180000",
      "Speed ×1.500",
      "Gain −3.0 dB",
      "Denoise 25 % — applied on export, not heard in preview yet",
      "Normalise to −16.0 LUFS — applied on export, not heard in preview yet",
    ]);
    expect(screen.getByTestId("operations-clip")).toHaveTextContent(
      "Track 1 · beach.mp4",
    );
  });

  it("says when nothing is under the playhead", async () => {
    invoked.mockResolvedValue({ position: 900, clips: [] });
    render(<OperationsPanel session={4} />);
    expect(
      await screen.findByText("Nothing on the timeline at frame 900."),
    ).toBeInTheDocument();
  });

  it("shows the engine's refusal instead of a stale list", async () => {
    invoked.mockRejectedValue("session 4 is not the project's preview");
    render(<OperationsPanel session={4} />);
    await waitFor(() => {
      expect(
        screen.getByText("session 4 is not the project's preview"),
      ).toBeInTheDocument();
    });
  });

  it("names whole-number speeds and positive gains plainly", () => {
    expect(
      describeOperation({
        operation: { op: "speed", ratio: { num: 4, den: 2 } },
        previewed: true,
      }),
    ).toBe("Speed ×2");
    expect(
      describeOperation({
        operation: { op: "gain", db: 2.5 },
        previewed: true,
      }),
    ).toBe("Gain +2.5 dB");
  });
});
