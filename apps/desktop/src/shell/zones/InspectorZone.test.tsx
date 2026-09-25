import type {
  AssetInfo,
  Edit,
  Placement,
  ProjectView,
  SpeedVerdict,
} from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../../project/project.store.js";
import { InspectorZone } from "./InspectorZone.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const placement = (clip: number, speed = { num: 1, den: 1 }): Placement => ({
  track: 1,
  kind: "video",
  clip,
  source: 1,
  stream: 0,
  timeBase: { num: 1, den: 15360 },
  sourceIn: 0,
  sourceOut: 153600,
  start: clip * 300,
  length: 300,
  speed,
  audio: [],
  sequenceTimeBase: { num: 1, den: 30 },
  silent: false,
});

const copy = (rate: number): SpeedVerdict => ({
  speed: { num: 1, den: 1 },
  outputFrameRate: { num: rate, den: 1 },
  variableFrameRate: false,
  tier: { tier: "stream-copy" },
  problems: [],
});

const PHONE: AssetInfo = {
  durationSeconds: 10,
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
    frameRate: { num: 30000, den: 1001 },
    variableFrameRate: true,
    pixelFormat: "yuv420p10le",
    colourPrimaries: "bt2020",
    colourTransfer: "arib-std-b67",
    colourMatrix: "bt2020nc",
    colourRange: "tv",
    hdr: true,
  },
  audio: null,
  matching: null,
};

function view(
  placements: Placement[],
  speeds: Record<number, SpeedVerdict> = {},
): ProjectView {
  return {
    path: "C:\\Work\\Trip.blinkify",
    dirty: false,
    project: {
      schemaVersion: 4,
      name: "Trip",
      sources: {
        1: {
          path: "C:\\Clips\\IMG_0042.MOV",
          fingerprint: { size: 1, modified: null, contentHash: "a" },
        },
      },
      sequence: {
        settings: {
          width: 1920,
          height: 1080,
          frameRate: { num: 30, den: 1 },
          pixelAspect: { num: 1, den: 1 },
          colour: "sdr",
        },
        matchFirstClip: false,
        tracks: [],
      },
    },
    timeline: {
      timeBase: { num: 1, den: 30 },
      frameRate: { num: 30, den: 1 },
      tracks: [
        {
          id: 1,
          kind: "video",
          visible: true,
          audible: true,
          placements,
        },
      ],
    },
    history: { entries: [], applied: 0 },
    unavailable: {},
    affectedClips: [],
    eligibility: { 1: { eligible: true, mismatches: [], notes: [] } },
    extents: [],
    assets: { 1: PHONE },
    speeds,
  };
}

/** The edits sent to the engine, in order. */
const sentEdits = (): Edit[] =>
  invoked.mock.calls
    .filter(([command]) => command === "edit_project")
    .map(([, args]) => (args as { edit: Edit }).edit);

const commands = () => invoked.mock.calls.map(([command]) => command);

function show(current: ProjectView, selection: number[]) {
  useProjectStore.setState({ view: current, selection });
  render(
    <TooltipProvider>
      <InspectorZone />
    </TooltipProvider>,
  );
}

describe("the inspector", () => {
  beforeEach(() => {
    invoked.mockReset();
    invoked.mockImplementation((command: string) =>
      Promise.resolve(
        command === "edit_project"
          ? {
              view: useProjectStore.getState().view,
              context: {
                selection: useProjectStore.getState().selection,
                playhead: 0,
              },
              clamped: false,
            }
          : command === "end_gesture"
            ? useProjectStore.getState().view
            : undefined,
      ),
    );
  });

  it("shows the sequence when nothing is selected", () => {
    show(view([placement(1)]), []);
    expect(screen.getByRole("heading", { name: "Sequence" })).toBeVisible();
    expect(screen.getByText(/Nothing is selected/)).toBeVisible();
    expect(screen.getByText("1920×1080 (16:9)")).toBeVisible();
    expect(screen.getByText(/1 of 1 sources match the sequence/)).toBeVisible();
  });

  it("shows a selected video clip's timing, speed and source verbatim", () => {
    show(view([placement(1)], { 1: copy(30) }), [1]);
    expect(screen.getByRole("heading", { name: "Video clip" })).toBeVisible();
    expect(screen.getByRole("group", { name: "Clip timing" })).toBeVisible();
    const facts = within(
      screen.getByLabelText("Source properties", { selector: "dl" }),
    );
    // What a bug report quotes, exactly as the probe said it.
    expect(facts.getByText("IMG_0042.MOV")).toBeVisible();
    expect(facts.getByText("hevc")).toBeVisible();
    expect(facts.getByText("Main 10")).toBeVisible();
    expect(
      facts.getByText("3840×2160, shown 2160×3840 (rotated 90°)"),
    ).toBeVisible();
    expect(facts.getByText("10-bit")).toBeVisible();
    expect(facts.getByText("30000/1001 fps (29.97)")).toBeVisible();
    expect(facts.getByText("arib-std-b67")).toBeVisible();
    expect(
      facts.getByText("Variable frame rate").nextElementSibling,
    ).toHaveTextContent("Yes");
  });

  it("states the engine's verdict for the speed while it is chosen", () => {
    const doubled = placement(1, { num: 2, den: 1 });
    show(
      view([doubled], {
        1: {
          ...copy(60),
          speed: { num: 2, den: 1 },
          variableFrameRate: true,
        },
      }),
      [1],
    );
    expect(screen.getByText(/Lossless: the video is copied/)).toBeVisible();
    expect(screen.getByText("60 fps, variable")).toBeVisible();
    expect(screen.getByText(/The sound is resampled/)).toBeVisible();
  });

  it("warns about a frame rate no container accepts before export", () => {
    const fast = placement(1, { num: 100, den: 1 });
    show(
      view([fast], {
        1: {
          speed: { num: 100, den: 1 },
          outputFrameRate: { num: 3000, den: 1 },
          variableFrameRate: false,
          tier: {
            tier: "full-re-encode",
            reason: "speed-frame-rate-outside-container",
          },
          problems: [
            {
              problem: "frame-rate-outside-container",
              rate: { num: 3000, den: 1 },
            },
          ],
        },
      }),
      [1],
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "3000 fps is not a frame rate a video file can carry.",
    );
    expect(screen.getByText(/Re-encoded: 3000 fps is outside/)).toBeVisible();
  });

  it("sets the same speed from the slider and from the typed factor", async () => {
    const user = userEvent.setup();
    show(view([placement(1)], { 1: copy(30) }), [1]);

    // The slider's lowest position is 0.1×.
    const thumb = screen.getByRole("slider", { name: "Speed" });
    act(() => thumb.focus());
    await user.keyboard("{Home}");
    await waitFor(() => expect(commands()).toContain("end_gesture"));
    const fromSlider = sentEdits().at(-1);
    // A key press is one gesture, closed; the change Radix reports after
    // its commit does not open another.
    await act(() => Promise.resolve());
    expect(commands().filter((c) => c === "begin_gesture")).toHaveLength(1);
    expect(commands().at(-1)).toBe("end_gesture");

    invoked.mockClear();
    const field = screen.getByRole("spinbutton", { name: "Factor" });
    await user.clear(field);
    await user.type(field, "0.1");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(commands()).toContain("end_gesture"));
    const typed = sentEdits().at(-1);

    expect(fromSlider).toEqual({
      edit: "set-speed",
      clips: [1],
      ratio: { num: 1, den: 10 },
    });
    expect(typed).toEqual(fromSlider);
  });

  it("makes a speed drag one gesture: one undo entry", async () => {
    show(view([placement(1)], { 1: copy(30) }), [1]);
    const label = screen.getByText("Factor", { selector: "label" });
    fireEvent.pointerDown(label, { pointerId: 1, clientX: 0 });
    for (let x = 4; x <= 40; x += 4)
      fireEvent.pointerMove(label, { pointerId: 1, clientX: x });
    fireEvent.pointerUp(label, { pointerId: 1, clientX: 40 });
    await waitFor(() => expect(commands()).toContain("end_gesture"));

    expect(commands().filter((c) => c === "begin_gesture")).toHaveLength(1);
    expect(commands().filter((c) => c === "end_gesture")).toHaveLength(1);
    expect(commands().at(-1)).toBe("end_gesture");
    expect(sentEdits().at(-1)).toEqual({
      edit: "set-speed",
      clips: [1],
      ratio: { num: 11, den: 10 },
    });
  });

  it("shows differing speeds as mixed and never overwrites them", async () => {
    const user = userEvent.setup();
    show(
      view([placement(1), placement(2, { num: 2, den: 1 })], {
        1: copy(30),
        2: { ...copy(60), speed: { num: 2, den: 1 } },
      }),
      [1, 2],
    );
    expect(
      screen.getByRole("heading", { name: "2 video clips" }),
    ).toBeVisible();
    const field = screen.getByRole("spinbutton", { name: "Factor" });
    expect(field).toHaveAttribute("aria-valuetext", "Mixed");
    expect(screen.getByText("Mixed", { selector: "output" })).toBeVisible();
    // Showing the selection sent nothing.
    expect(sentEdits()).toEqual([]);

    // Setting it is explicit, and applies to every selected clip.
    await user.type(field, "3");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(commands()).toContain("end_gesture"));
    expect(sentEdits().at(-1)).toEqual({
      edit: "set-speed",
      clips: [1, 2],
      ratio: { num: 3, den: 1 },
    });
  });

  it("resets the speed as one edit, per control and for the section", async () => {
    const user = userEvent.setup();
    show(
      view([placement(1, { num: 3, den: 2 })], {
        1: { ...copy(45), speed: { num: 3, den: 2 } },
      }),
      [1],
    );
    await user.click(screen.getByRole("button", { name: "Reset speed" }));
    await user.click(screen.getByRole("button", { name: "Reset all" }));
    const normal = { edit: "set-speed", clips: [1], ratio: { num: 1, den: 1 } };
    expect(sentEdits()).toEqual([normal, normal]);
    expect(commands()).not.toContain("begin_gesture");
  });

  it("says when a clip is re-encoded whatever its speed", () => {
    show(
      view([{ ...placement(1), motion: "reverse", forced: "reverse" }], {
        1: copy(30),
      }),
      [1],
    );
    expect(
      screen.getByText("A reversed clip is re-encoded whatever its speed."),
    ).toBeVisible();
  });
});
