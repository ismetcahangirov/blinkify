import type { ExportJob, ExportState } from "@blinkify/types";
import { describe, expect, it } from "vitest";

import {
  baseName,
  isActive,
  isFinished,
  isInterrupted,
  remaining,
  size,
  statusLine,
} from "./exportJobs.js";
import { upsert } from "./exportJobs.store.js";

function job(id: number, state: ExportState): ExportJob {
  return {
    id,
    name: "Trip",
    target: `C:\\Exports\\trip-${id}.mp4`,
    audio: { codec: "aac", kilobits: 256 },
    submitted: 0,
    started: null,
    finished: null,
    state,
  };
}

const running = (
  fraction: number,
  remainingSeconds: number | null,
): ExportState => ({
  state: "running",
  stage: "exporting",
  fraction,
  remainingSeconds,
});

describe("export jobs in words", () => {
  it("sorts every state into queue, crash offer or history", () => {
    expect(isActive(job(1, { state: "queued" }))).toBe(true);
    expect(isActive(job(1, running(0.5, null)))).toBe(true);
    expect(isInterrupted(job(1, { state: "interrupted" }))).toBe(true);
    for (const state of [
      { state: "completed", bytes: 1 },
      { state: "failed", message: "x" },
      { state: "cancelled" },
    ] as const) {
      expect(isFinished(job(1, state))).toBe(true);
      expect(isActive(job(1, state))).toBe(false);
    }
    expect(isFinished(job(1, { state: "interrupted" }))).toBe(false);
  });

  it("states progress from the engine's fraction, and the estimate rounded", () => {
    expect(statusLine(job(1, running(0.426, null)))).toBe("Exporting — 42%");
    expect(statusLine(job(1, running(0.5, 200)))).toBe(
      "Exporting — 50%, about 3 min left",
    );
    expect(remaining(44)).toBe("about 40 s left");
    expect(remaining(4)).toBe("a few seconds left");
    expect(
      statusLine(
        job(1, {
          state: "running",
          stage: "preparing",
          fraction: 0,
          remainingSeconds: null,
        }),
      ),
    ).toMatch(/^Preparing/);
  });

  it("says that a failed or cancelled export left nothing behind", () => {
    expect(
      statusLine(job(1, { state: "failed", message: "the disk is full" })),
    ).toBe("Failed: the disk is full. Nothing was left at the target.");
    expect(statusLine(job(1, { state: "cancelled" }))).toMatch(
      /Nothing was left/,
    );
    expect(statusLine(job(1, { state: "completed", bytes: 1_234_000 }))).toBe(
      "Exported — 1.2 MB",
    );
  });

  it("names sizes and files", () => {
    expect(size(512)).toBe("512 bytes");
    expect(size(640_000_000)).toBe("640 MB");
    expect(size(1_500_000_000)).toBe("1.5 GB");
    expect(baseName("C:\\Exports\\trip.mp4")).toBe("trip.mp4");
    expect(baseName("/home/u/trip.mkv")).toBe("trip.mkv");
  });
});

describe("the queue as the engine last described it", () => {
  it("replaces a job in place and adds a new one last", () => {
    const jobs = [job(1, { state: "queued" }), job(2, { state: "queued" })];
    const updated = upsert(jobs, job(1, running(0.1, null)));
    expect(updated.map((j) => j.state.state)).toEqual(["running", "queued"]);
    expect(
      upsert(updated, job(3, { state: "queued" })).map((j) => j.id),
    ).toEqual([1, 2, 3]);
  });
});
