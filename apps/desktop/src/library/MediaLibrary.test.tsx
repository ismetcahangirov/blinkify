import type {
  AssetInfo,
  ImportOutcome,
  ProjectView,
  Track,
} from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import { assetsOf, codecBadge, duration, filterAssets } from "./assets.js";
import { useLibraryStore } from "./library.store.js";
import { MediaLibrary } from "./MediaLibrary.js";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
// The engine's thumbnails and waveforms: still preparing, throughout.
vi.mock("./libraryMedia.js", () => ({
  cardTile: () => null,
  waveformReady: () => false,
  onLibraryMedia: () => () => undefined,
  setLibrarySources: () => undefined,
}));

const invoked = vi.mocked(invoke);
const chosen = vi.mocked(open);

const fingerprint = { size: 10, modified: null, contentHash: "ab" };

const PHONE: AssetInfo = {
  durationSeconds: 102.4,
  video: {
    stream: 0,
    codec: "hevc",
    profile: "Main 10",
    width: 3840,
    height: 2160,
    displayWidth: 2160,
    displayHeight: 3840,
    rotation: 90,
    bitDepth: 10,
    frameRate: { num: 30, den: 1 },
    variableFrameRate: true,
    pixelFormat: "yuv420p10le",
    colourPrimaries: "bt2020",
    colourTransfer: "arib-std-b67",
    colourMatrix: "bt2020nc",
    colourRange: "tv",
    hdr: true,
  },
  audio: { stream: 1, codec: "aac", sampleRate: 48000, channels: 2 },
  matching: {
    width: 2160,
    height: 3840,
    frameRate: { num: 30, den: 1 },
    pixelAspect: { num: 1, den: 1 },
    colour: "sdr",
  },
};

const SONG: AssetInfo = {
  durationSeconds: 3725,
  video: null,
  audio: { stream: 0, codec: "mp3", sampleRate: 44100, channels: 2 },
  matching: null,
};

const V1: Track = {
  id: 1,
  kind: "video",
  name: "",
  muted: false,
  solo: false,
  locked: false,
  collapsed: false,
  clips: [
    {
      id: 7,
      source: 1,
      stream: 0,
      timeBase: { num: 1, den: 15360 },
      start: 0,
      operations: [],
      detached: false,
    },
  ],
};

const VIEW: ProjectView = {
  path: "C:\\Work\\Trip.blinkify",
  dirty: false,
  project: {
    schemaVersion: 4,
    name: "Trip",
    sources: {
      1: { path: "D:\\Çəkiliş\\Bakı sahili.mp4", fingerprint },
      2: { path: "C:\\Music\\song.mp3", fingerprint },
      3: { path: "E:\\Card\\gone.mov", fingerprint },
    },
    sequence: {
      settings: {
        width: 1920,
        height: 1080,
        frameRate: { num: 25, den: 1 },
        pixelAspect: { num: 1, den: 1 },
        colour: "sdr",
      },
      matchFirstClip: false,
      tracks: [V1],
    },
  },
  unavailable: { 3: { state: "missing" } },
  affectedClips: [],
  eligibility: {
    1: {
      eligible: false,
      mismatches: [
        {
          reason: "frame-rate",
          sequence: { num: 25, den: 1 },
          source: { num: 30, den: 1 },
        },
      ],
      notes: [],
    },
  },
  extents: [],
  assets: { 1: PHONE, 2: SONG },
  timeline: null,
  history: { entries: [], applied: 0 },
};

beforeEach(() => {
  invoked.mockReset();
  chosen.mockReset();
  useProjectStore.setState({ view: VIEW, lifecycleError: null });
  useLibraryStore.setState({
    query: "",
    kind: "all",
    density: "grid",
    importing: null,
    refused: [],
  });
});

describe("the library's view of the sources", () => {
  it("lists every source, its use and its state", () => {
    const assets = assetsOf(VIEW);
    expect(assets.map((a) => [a.name, a.kind, a.uses])).toEqual([
      ["Bakı sahili.mp4", "video", 1],
      ["gone.mov", "audio", 0],
      ["song.mp3", "audio", 0],
    ]);
    expect(assets[0]?.eligible).toBe(false);
    expect(assets[1]?.status).toEqual({ state: "missing" });
  });

  it("searches by name and filters by type", () => {
    const assets = assetsOf(VIEW);
    expect(filterAssets(assets, "BAKI", "all").map((a) => a.id)).toEqual([1]);
    expect(filterAssets(assets, "sahİli", "all").map((a) => a.id)).toEqual([1]);
    expect(filterAssets(assets, "", "video").map((a) => a.id)).toEqual([1]);
    expect(filterAssets(assets, "so", "audio").map((a) => a.id)).toEqual([2]);
  });

  it("says durations and codecs as a person would", () => {
    expect(duration(102.4)).toBe("1:42");
    expect(duration(3725)).toBe("1:02:05");
    expect(duration(null)).toBe("—");
    expect(codecBadge(PHONE)).toBe("HEVC");
    expect(codecBadge(SONG)).toBe("MP3");
    expect(codecBadge(null)).toBe("?");
  });
});

describe("the media library", () => {
  it("shows each asset's probed facts and its state", () => {
    render(<MediaLibrary />);
    const phone = screen.getByTestId("asset-1");
    expect(phone.textContent).toContain("Bakı sahili.mp4");
    expect(phone.textContent).toContain("HEVC");
    expect(phone.textContent).toContain("2160×3840");
    expect(phone.textContent).toContain("1:42");
    expect(phone.textContent).toContain("HDR");
    expect(phone.textContent).toContain("VFR");
    expect(screen.getByTestId("asset-reencodes")).toBeTruthy();
    expect(screen.getByTestId("asset-3").textContent).toContain("Missing");
  });

  it("has the tab row, with only Media active", () => {
    render(<MediaLibrary />);
    expect(
      screen.getByRole("tab", { name: "Media" }).getAttribute("aria-selected"),
    ).toBe("true");
    for (const name of ["Audio", "Text", "Effects", "Transitions"])
      expect(screen.getByRole("tab", { name }).hasAttribute("disabled")).toBe(
        true,
      );
  });

  it("imports what the dialog chose and lists what was refused", async () => {
    chosen.mockResolvedValueOnce(["C:\\a.mp4", "C:\\notes.mp4"]);
    const outcome: ImportOutcome = {
      view: VIEW,
      imported: [1],
      refused: [{ path: "C:\\notes.mp4", reason: "it is not a media file" }],
    };
    invoked.mockResolvedValueOnce(outcome);
    render(<MediaLibrary />);
    fireEvent.click(screen.getByRole("button", { name: "Import" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith("import_media", {
        paths: ["C:\\a.mp4", "C:\\notes.mp4"],
        context: expect.anything() as unknown,
      }),
    );
    expect((await screen.findByTestId("refused")).textContent).toContain(
      "notes.mp4: it is not a media file.",
    );
  });

  it("warns before removing an asset the timeline uses", async () => {
    invoked.mockResolvedValue({ view: VIEW, clamped: false });
    render(<MediaLibrary />);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Remove Bakı sahili.mp4 from the project",
      }),
    );
    expect(invoked).not.toHaveBeenCalled();
    expect(await screen.findByText(/used by 1 clip/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith(
        "edit_project",
        expect.objectContaining({
          edit: { edit: "remove-source", source: 1 },
        }),
      ),
    );
  });

  it("removes an unused asset at once: Undo is the way back", async () => {
    invoked.mockResolvedValue({ view: VIEW, clamped: false });
    render(<MediaLibrary />);
    fireEvent.click(
      screen.getByRole("button", { name: "Remove song.mp3 from the project" }),
    );
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith(
        "edit_project",
        expect.objectContaining({
          edit: { edit: "remove-source", source: 2 },
        }),
      ),
    );
  });

  it("relinks a missing asset to the file chosen", async () => {
    chosen.mockResolvedValueOnce("F:\\found\\gone.mov");
    invoked.mockResolvedValueOnce(VIEW);
    render(<MediaLibrary />);
    fireEvent.click(screen.getByRole("button", { name: "Relink…" }));
    await waitFor(() =>
      expect(invoked).toHaveBeenCalledWith(
        "relink_source",
        expect.objectContaining({ source: 3, path: "F:\\found\\gone.mov" }),
      ),
    );
  });

  it("filters the grid by the search", () => {
    render(<MediaLibrary />);
    fireEvent.change(screen.getByLabelText("Search media"), {
      target: { value: "song" },
    });
    expect(screen.queryByTestId("asset-1")).toBeNull();
    expect(screen.getByTestId("asset-2")).toBeTruthy();
  });

  it("stays responsive through a hundred-file import", () => {
    const store = useLibraryStore.getState();
    const began = performance.now();
    for (let done = 1; done <= 100; done++)
      store.progress({
        path: `C:\\clip${done}.mp4`,
        done,
        total: 100,
        refused: null,
      });
    expect(performance.now() - began).toBeLessThan(250);
    expect(useLibraryStore.getState().importing).toBeNull();
  });
});
