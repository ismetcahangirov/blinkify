import type { CropFrame, CropRect, Edit, ProjectView } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen } from "@testing-library/react";
import {
  afterAll,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import { useProjectStore } from "../project/project.store.js";
import { CropOverlay } from "./CropOverlay.js";
import { useCropFramingStore } from "./cropFraming.store.js";
import { usePreviewStore } from "./preview.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const VERTICAL: CropRect = { x: 656, y: 0, width: 608, height: 1080 };

const HD: CropFrame = {
  width: 1920,
  height: 1080,
  across: 2,
  down: 2,
  minSize: 16,
  presets: [
    { aspect: "source", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "16:9", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
    { aspect: "9:16", rect: VERTICAL },
    { aspect: "1:1", rect: { x: 420, y: 0, width: 1080, height: 1080 } },
    { aspect: "4:5", rect: { x: 528, y: 0, width: 864, height: 1080 } },
  ],
};

/** A view with clip 1 of source 1 on the timeline, cropped to `crop`. */
function view(crop: CropRect | undefined, known = true): ProjectView {
  return {
    frames: known ? { 1: HD } : {},
    timeline: {
      tracks: [
        {
          placements: [
            {
              clip: 1,
              source: 1,
              start: 0,
              length: 90,
              ...(crop ? { crop } : {}),
            },
          ],
        },
      ],
    },
  } as unknown as ProjectView;
}

const edit = vi.fn((_: Edit) => Promise.resolve<string | null>(null));
const undo = vi.fn(() => Promise.resolve());
const beginGesture = vi.fn((_: string) => Promise.resolve());
const endGesture = vi.fn(() => Promise.resolve());

// jsdom lays nothing out: the overlay is 960 × 540 CSS pixels, the size of
// the picture, so one CSS pixel is two source pixels.
const size = { width: 960, height: 540 };
let restore: (() => void) | null = null;

beforeAll(() => {
  const proto = HTMLElement.prototype;
  const width = Object.getOwnPropertyDescriptor(proto, "clientWidth");
  const height = Object.getOwnPropertyDescriptor(proto, "clientHeight");
  Object.defineProperty(proto, "clientWidth", {
    configurable: true,
    get: () => size.width,
  });
  Object.defineProperty(proto, "clientHeight", {
    configurable: true,
    get: () => size.height,
  });
  restore = () => {
    if (width) Object.defineProperty(proto, "clientWidth", width);
    if (height) Object.defineProperty(proto, "clientHeight", height);
  };
});

afterAll(() => {
  restore?.();
});

beforeEach(() => {
  invoked.mockReset();
  invoked.mockResolvedValue(undefined);
  for (const mock of [edit, undo, beginGesture, endGesture]) mock.mockClear();
  useProjectStore.setState({
    view: view(VERTICAL),
    selection: [1],
    edit,
    undo,
    beginGesture,
    endGesture,
  });
  usePreviewStore.setState({
    kind: "project",
    picture: { width: 1920, height: 1080, rotation: 0 },
    frameNumber: 10,
  });
  useCropFramingStore.setState({ clip: 1 });
});

function overlay() {
  render(
    <TooltipProvider>
      <CropOverlay />
    </TooltipProvider>,
  );
  return screen.getByTestId("crop-overlay");
}

/** The canvas point of source point `x`, `y`. */
const at = (x: number, y: number) => ({ clientX: x / 2, clientY: y / 2 });

async function settle() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe("framing a crop on the preview", () => {
  it("makes a drag one gesture: one entry, the rectangle on the grid", async () => {
    const canvas = overlay();
    // The bottom-right corner of the 9:16 crop, dragged up and left.
    fireEvent.pointerDown(canvas, { pointerId: 1, ...at(1264, 1080) });
    fireEvent.pointerMove(canvas, { pointerId: 1, ...at(1200, 1000) });
    fireEvent.pointerMove(canvas, { pointerId: 1, ...at(1101, 901) });
    fireEvent.pointerUp(canvas, { pointerId: 1, ...at(1101, 901) });
    await settle();
    expect(beginGesture).toHaveBeenCalledTimes(1);
    expect(endGesture).toHaveBeenCalledTimes(1);
    const last = edit.mock.calls.at(-1)?.[0];
    expect(last).toEqual({
      edit: "set-crop",
      clips: [1],
      // A 9:16 preset keeps its shape while its corner is dragged.
      rect: expect.objectContaining({ x: 656, y: 0 }) as unknown,
    });
    const rect = (last as { rect: CropRect }).rect;
    expect(rect.width).toBeLessThan(608);
    for (const value of Object.values(rect)) expect(value % 2).toBe(0);
    expect(undo).not.toHaveBeenCalled();
  });

  it("puts the crop back when Escape is pressed during a drag", async () => {
    const canvas = overlay();
    fireEvent.pointerDown(canvas, { pointerId: 1, ...at(960, 540) });
    fireEvent.pointerMove(canvas, { pointerId: 1, ...at(700, 540) });
    fireEvent.keyDown(canvas, { key: "Escape" });
    await settle();
    expect(endGesture).toHaveBeenCalledTimes(1);
    expect(undo).toHaveBeenCalledTimes(1);
    // Still framing: Escape ended the drag, not the mode.
    expect(useCropFramingStore.getState().clip).toBe(1);
  });

  it("leaves on Escape or Enter without changing anything", async () => {
    const canvas = overlay();
    fireEvent.keyDown(canvas, { key: "Escape" });
    await settle();
    expect(useCropFramingStore.getState().clip).toBeNull();
    expect(invoked).toHaveBeenCalledWith("frame_crop", { clip: null });
    act(() => {
      useCropFramingStore.setState({ clip: 1 });
    });
    fireEvent.keyDown(screen.getByTestId("crop-overlay"), { key: "Enter" });
    await settle();
    expect(useCropFramingStore.getState().clip).toBeNull();
    expect(edit).not.toHaveBeenCalled();
  });

  it("moves the rectangle a grid step with an arrow, ten with Shift", async () => {
    const canvas = overlay();
    fireEvent.keyDown(canvas, { key: "ArrowRight" });
    fireEvent.keyUp(canvas, { key: "ArrowRight" });
    await settle();
    expect(edit).toHaveBeenLastCalledWith({
      edit: "set-crop",
      clips: [1],
      rect: { ...VERTICAL, x: 658 },
    });
    fireEvent.keyDown(canvas, { key: "ArrowLeft", shiftKey: true });
    fireEvent.keyUp(canvas, { key: "ArrowLeft" });
    await settle();
    expect(edit).toHaveBeenLastCalledWith({
      edit: "set-crop",
      clips: [1],
      rect: { ...VERTICAL, x: 636 },
    });
    expect(endGesture).toHaveBeenCalledTimes(2);
  });

  it("leaves, keeping the crop, when another clip is selected mid-drag", async () => {
    const canvas = overlay();
    fireEvent.pointerDown(canvas, { pointerId: 1, ...at(960, 540) });
    fireEvent.pointerMove(canvas, { pointerId: 1, ...at(700, 540) });
    act(() => {
      useProjectStore.setState({ selection: [2] });
    });
    await settle();
    expect(useCropFramingStore.getState().clip).toBeNull();
    expect(endGesture).toHaveBeenCalledTimes(1);
    expect(undo).not.toHaveBeenCalled();
  });

  it("says so, and takes no drag, when the source is offline", async () => {
    useProjectStore.setState({ view: view(VERTICAL, false) });
    const canvas = overlay();
    expect(screen.getByRole("status")).toHaveTextContent("offline");
    fireEvent.pointerDown(canvas, { pointerId: 1, ...at(960, 540) });
    fireEvent.pointerMove(canvas, { pointerId: 1, ...at(700, 540) });
    fireEvent.pointerUp(canvas, { pointerId: 1, ...at(700, 540) });
    await settle();
    expect(edit).not.toHaveBeenCalled();
  });
});
