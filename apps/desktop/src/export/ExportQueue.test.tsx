import type { ExportJob, ExportState } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ExportQueuePanel, InterruptedExportsBanner } from "./ExportQueue.js";
import { useExportJobs } from "./exportJobs.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

const invoked = vi.mocked(invoke);

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

beforeEach(() => {
  invoked.mockReset();
  // The engine's list is what the store holds, unless a test changes it.
  invoked.mockImplementation((command) =>
    Promise.resolve(
      command === "export_jobs" ? useExportJobs.getState().jobs : undefined,
    ),
  );
  useExportJobs.setState({ jobs: [], error: null });
});

describe("the export queue", () => {
  it("draws the engine's progress, and cancels through the engine", async () => {
    useExportJobs.setState({
      jobs: [
        job(1, {
          state: "running",
          stage: "exporting",
          fraction: 0.25,
          remainingSeconds: 30,
        }),
        job(2, { state: "queued" }),
        job(3, { state: "completed", bytes: 2_000_000 }),
      ],
    });
    render(<ExportQueuePanel />);

    const bar = screen.getByRole("progressbar", {
      name: "Progress of trip-1.mp4",
    });
    expect(bar).toHaveAttribute("value", "0.25");
    expect(screen.getByText("Exporting — 25%, about 30 s left")).toBeVisible();
    expect(screen.getByText("Waiting for the exports before it")).toBeVisible();
    expect(screen.getByText("Exported — 2.0 MB")).toBeVisible();

    const row = screen.getByTestId("export-job-1");
    await userEvent.click(
      row.querySelector("button") ?? document.createElement("button"),
    );
    expect(invoked).toHaveBeenCalledWith("cancel_export", { id: 1 });
  });

  it("does not guess how long preparing takes", () => {
    useExportJobs.setState({
      jobs: [
        job(1, {
          state: "running",
          stage: "preparing",
          fraction: 0,
          remainingSeconds: null,
        }),
      ],
    });
    render(<ExportQueuePanel />);
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("value");
  });

  it("offers an interrupted export again, from the start, or discards it", async () => {
    useExportJobs.setState({
      jobs: [
        job(4, { state: "interrupted" }),
        job(5, { state: "interrupted" }),
      ],
    });
    render(<InterruptedExportsBanner />);
    expect(
      screen.getByText(/2 exports were interrupted when Blinkify closed/),
    ).toBeVisible();

    await userEvent.click(
      screen.getAllByRole("button", { name: "Export again" })[0] as HTMLElement,
    );
    expect(invoked).toHaveBeenCalledWith("resume_export", { id: 4 });
    await userEvent.click(
      screen.getAllByRole("button", { name: "Discard" })[1] as HTMLElement,
    );
    expect(invoked).toHaveBeenCalledWith("discard_export", { id: 5 });
  });

  it("shows nothing when nothing was interrupted", () => {
    useExportJobs.setState({ jobs: [job(1, { state: "queued" })] });
    const { container } = render(<InterruptedExportsBanner />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says why a request was refused", () => {
    useExportJobs.setState({ error: "there is no export 9" });
    render(<ExportQueuePanel />);
    expect(screen.getByRole("alert")).toHaveTextContent("there is no export 9");
  });
});
