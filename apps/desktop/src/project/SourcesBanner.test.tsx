import type { ProjectView } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fileName, projectName, useProjectStore } from "./project.store.js";
import { SourcesBanner } from "./SourcesBanner.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const fingerprint = { size: 10, modified: null, contentHash: "ab" };

const VIEW: ProjectView = {
  path: "C:\\Work\\Trip.blinkify",
  project: {
    schemaVersion: 1,
    name: "Trip",
    sources: {
      1: { path: "E:\\Card\\beach.mp4", fingerprint },
      2: { path: "C:\\Footage\\city.mp4", fingerprint },
      3: { path: "C:\\Footage\\here.mp4", fingerprint },
    },
    sequence: {
      settings: {
        width: 1920,
        height: 1080,
        frameRate: { num: 30, den: 1 },
        pixelAspect: { num: 1, den: 1 },
        colour: "sdr",
      },
      tracks: [],
    },
  },
  unavailable: {
    1: { state: "missing" },
    2: { state: "changed", found: { ...fingerprint, size: 11 } },
  },
  affectedClips: [4, 5, 9],
  timeline: null,
  history: { entries: [], applied: 0 },
};

beforeEach(() => {
  invoked.mockReset();
  useProjectStore.setState({ view: null, error: null, relinkError: null });
});

describe("the sources banner", () => {
  it("says nothing when Blinkify was launched without a project", async () => {
    invoked.mockResolvedValueOnce(null);
    const { container } = render(<SourcesBanner />);
    await waitFor(() => {
      expect(invoked).toHaveBeenCalledWith("launch_project");
    });
    expect(container).toBeEmptyDOMElement();
  });

  it("opens the project and marks what is offline, and why", async () => {
    invoked.mockResolvedValueOnce(VIEW);
    render(<SourcesBanner />);
    const banner = await screen.findByTestId("sources-banner");
    expect(banner).toHaveTextContent("3 clips are offline.");
    expect(screen.getByTestId("source-1")).toHaveTextContent(
      "beach.mp4 is not at E:\\Card\\beach.mp4.",
    );
    expect(screen.getByTestId("source-2")).toHaveTextContent(
      "city.mp4 has changed since the project was saved.",
    );
    // A present source is not mentioned.
    expect(screen.queryByTestId("source-3")).toBeNull();
    expect(banner).toHaveTextContent(
      "Drop the moved file of beach.mp4 here to relink it.",
    );
  });

  it("says why a project could not be opened", async () => {
    invoked.mockRejectedValueOnce(
      "this project was saved by a newer version of Blinkify (schema 2); this version reads up to schema 1",
    );
    render(<SourcesBanner />);
    expect(await screen.findByTestId("project-error")).toHaveTextContent(
      "The project could not be opened: this project was saved by a newer version",
    );
  });
});

describe("the project store", () => {
  it("relinks through the engine and shows what it found", async () => {
    const relinked: ProjectView = {
      ...VIEW,
      unavailable: {},
      affectedClips: [],
    };
    invoked.mockResolvedValueOnce(relinked);
    await useProjectStore.getState().relink(1, "D:\\Moved\\beach.mp4");
    expect(invoked).toHaveBeenCalledWith("relink_source", {
      source: 1,
      path: "D:\\Moved\\beach.mp4",
      context: { selection: [], playhead: 0 },
    });
    expect(useProjectStore.getState().view).toBe(relinked);
  });

  it("keeps the project and reports a refused relink", async () => {
    useProjectStore.setState({ view: VIEW });
    invoked.mockRejectedValueOnce(
      "that file is not the one the project was made with",
    );
    await useProjectStore.getState().relink(1, "D:\\other.mp4");
    expect(useProjectStore.getState()).toMatchObject({
      view: VIEW,
      relinkError: "that file is not the one the project was made with",
    });
  });

  it("sends an edit with its context and shows what came back", async () => {
    useProjectStore.setState({ view: VIEW, selection: [4] });
    const renamed = {
      ...VIEW,
      project: { ...VIEW.project, name: "Trip 2" },
      history: { entries: ["Rename project"], applied: 1 },
    };
    invoked.mockResolvedValueOnce({
      view: renamed,
      context: { selection: [4], playhead: 0 },
    });
    const edit = { edit: "rename", name: "Trip 2" } as const;
    expect(await useProjectStore.getState().edit(edit)).toBeNull();
    expect(invoked).toHaveBeenCalledWith("edit_project", {
      edit,
      context: { selection: [4], playhead: 0 },
    });
    expect(useProjectStore.getState().view?.project.name).toBe("Trip 2");

    invoked.mockRejectedValueOnce("clips 1 and 2 overlap on track 1");
    expect(await useProjectStore.getState().edit(edit)).toBe(
      "clips 1 and 2 overlap on track 1",
    );
    expect(useProjectStore.getState()).toMatchObject({
      editError: "clips 1 and 2 overlap on track 1",
    });
    expect(useProjectStore.getState().view?.project.name).toBe("Trip 2");
  });

  it("restores the selection that came with an undone edit", async () => {
    useProjectStore.setState({ view: VIEW, selection: [] });
    invoked.mockResolvedValueOnce({
      view: VIEW,
      context: { selection: [4, 5], playhead: 0 },
    });
    await useProjectStore.getState().undo();
    expect(invoked).toHaveBeenCalledWith("undo_edit");
    expect(useProjectStore.getState().selection).toEqual([4, 5]);

    // Nothing to redo: the engine says so with null, and nothing changes.
    invoked.mockResolvedValueOnce(null);
    await useProjectStore.getState().redo();
    expect(useProjectStore.getState().selection).toEqual([4, 5]);
  });

  it("names the project, or says it has none", () => {
    expect(projectName(null)).toBe("Untitled project");
    expect(projectName(VIEW)).toBe("Trip");
    expect(
      projectName({ ...VIEW, project: { ...VIEW.project, name: "  " } }),
    ).toBe("Untitled project");
    expect(fileName("C:\\a\\b.mp4")).toBe("b.mp4");
    expect(fileName("/a/b.mp4")).toBe("b.mp4");
  });
});
