import type { ExportJob, ExportReport } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ExportQueuePanel } from "./ExportQueue.js";
import { ExportReportDialog, clock } from "./ExportReportDialog.js";
import { statusLine } from "./exportJobs.js";
import { useExportJobs } from "./exportJobs.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: vi.fn(() => Promise.resolve("C:\\Reports\\trip report.txt")),
}));

const invoked = vi.mocked(invoke);

const FILE = {
  name: "out.mp4",
  path: "C:\\Exports\\out.mp4",
  container: "QuickTime / MOV",
  codecs: ["h264", "aac"],
  sizeBytes: 12_300_000,
  durationSeconds: 42.1,
};

const REPORT: ExportReport = {
  created: 1_790_458_800_000,
  name: "Trip",
  sources: [{ ...FILE, name: "raw.mp4", path: "C:\\Clips\\raw.mp4" }],
  output: FILE,
  video: {
    copiedSeconds: 41.6,
    reEncodedSeconds: 0.5,
    packets: 1263,
    identical: 1248,
  },
  audio: null,
  identicalPercent: 98.8,
  lossless: false,
  summary:
    "Not fully lossless: 98.8 % of the output's packets are bit-identical to their source's; 0.50 s of the pictures re-encoded.",
  segments: [
    {
      media: "video",
      startSeconds: 0,
      endSeconds: 42.1,
      planned: { tier: "smart-cut", reason: "in-point-not-keyframe-aligned" },
      execution: "partly-re-encoded",
      asPlanned: true,
      packets: 1263,
      identical: 1248,
      reasons: ["The clip starts 0.40 s from the nearest keyframe, so …"],
      suggestions: [
        "Snap the cut to the nearest keyframe (Export, then Snap to keyframes) and it is copied whole.",
      ],
      encoder: "h264_nvenc (high profile, yuv420p, level 41)",
    },
  ],
  suggestions: [],
  audioEncoding: null,
  commands: [],
};

function job(state: ExportJob["state"], hasReport: boolean): ExportJob {
  return {
    id: 7,
    name: "Trip",
    target: "C:\\Exports\\out.mp4",
    audio: { codec: "aac", kilobits: 256 },
    submitted: 0,
    started: null,
    finished: null,
    state,
    hasReport,
  };
}

beforeEach(() => {
  invoked.mockReset();
  vi.mocked(save).mockClear();
  invoked.mockImplementation((command, args) => {
    if (command === "export_report") return Promise.resolve(REPORT);
    if (command === "export_report_text") {
      const { withPaths } = args as { withPaths: boolean };
      return Promise.resolve(withPaths ? "with C:\\Exports" : "out.mp4 only");
    }
    return Promise.resolve(null);
  });
  useExportJobs.setState({ jobs: [], error: null, reportOf: null });
});

describe("the export report", () => {
  it("opens from the history of an export that kept one", async () => {
    useExportJobs.setState({
      jobs: [
        job({ state: "completed", bytes: 12_300_000 }, true),
        { ...job({ state: "cancelled" }, false), id: 8 },
      ],
    });
    render(<ExportQueuePanel />);
    const reports = screen.getAllByRole("button", { name: "Report" });
    expect(reports).toHaveLength(1);
    await userEvent.click(reports[0] as HTMLElement);
    expect(useExportJobs.getState().reportOf).toBe(7);
  });

  it("shows what was measured, segment by segment, with what to do instead", async () => {
    useExportJobs.setState({ reportOf: 7 });
    render(
      <TooltipProvider>
        <ExportReportDialog />
      </TooltipProvider>,
    );
    const report = await screen.findByTestId("export-report");
    expect(invoked).toHaveBeenCalledWith("export_report", { id: 7 });
    expect(report).toHaveTextContent("Not fully lossless: 98.8 %");
    expect(report).toHaveTextContent("1248 of 1263 packets bit-identical");
    expect(report).toHaveTextContent(
      "0:00.0–0:42.1 Pictures: smart-cut: partly re-encoded",
    );
    expect(report).toHaveTextContent("Encoded with h264_nvenc");
    expect(report).toHaveTextContent("Instead: Snap the cut");
  });

  it("gives the text without folders unless they are included", async () => {
    const written = vi.fn(() => Promise.resolve());
    Object.assign(navigator, { clipboard: { writeText: written } });
    useExportJobs.setState({ reportOf: 7 });
    render(
      <TooltipProvider>
        <ExportReportDialog />
      </TooltipProvider>,
    );
    await screen.findByTestId("export-report");
    await userEvent.click(screen.getByRole("button", { name: "Copy as text" }));
    await waitFor(() => expect(written).toHaveBeenCalledWith("out.mp4 only"));
    expect(invoked).toHaveBeenCalledWith("export_report_text", {
      id: 7,
      withPaths: false,
    });

    await userEvent.click(
      screen.getByRole("switch", { name: "Include folders" }),
    );
    await userEvent.click(
      screen.getByRole("button", { name: "Save as text…" }),
    );
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("save_export_report", {
        id: 7,
        path: "C:\\Reports\\trip report.txt",
        withPaths: true,
      }),
    );
  });

  it("says what verifying is", () => {
    expect(
      statusLine(
        job(
          {
            state: "running",
            stage: "verifying",
            fraction: 0,
            remainingSeconds: null,
          },
          false,
        ),
      ),
    ).toMatch(/^Verifying: comparing every packet/);
    expect(clock(65.44)).toBe("1:05.4");
  });
});
