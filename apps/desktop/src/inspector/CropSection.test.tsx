import type { ClipsCost, CropFrame, CropRect } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { CropSection } from "./CropSection.js";

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

const clip = (id: number, crop?: CropRect, source = 1): Placement =>
  ({
    clip: id,
    kind: "video",
    source,
    audio: [],
    silent: false,
    ...(crop ? { crop, forced: "filter-changes-pixels" } : {}),
  }) as unknown as Placement;

const cost = (cropped: boolean): ClipsCost => ({
  clips: 1,
  croppedClips: cropped ? 1 : 0,
  croppedSeconds: cropped ? 4.2 : 0,
  otherSeconds: 0,
  otherReasons: [],
  seamedSeconds: 0,
  copiedSeconds: cropped ? 0 : 4.2,
  declined: false,
  sound: { copiedSeconds: 4.2, reEncodedSeconds: 0, reasons: [] },
});

const edit = vi.fn((_: unknown) => Promise.resolve<string | null>(null));
const beginGesture = vi.fn(() => Promise.resolve());
const endGesture = vi.fn(() => Promise.resolve());

function show(
  clips: Placement[],
  frames: Record<number, CropFrame> = { 1: HD },
) {
  useProjectStore.setState({
    edit,
    beginGesture,
    endGesture,
    view: { project: { sequence: {} }, frames } as never,
  });
  return render(
    <TooltipProvider>
      <CropSection clips={clips} />
    </TooltipProvider>,
  );
}

const field = (name: string): HTMLInputElement =>
  screen.getByRole("spinbutton", { name });

beforeEach(() => {
  invoked.mockReset();
  edit.mockClear();
  edit.mockImplementation(() => Promise.resolve(null));
  beginGesture.mockClear();
  endGesture.mockClear();
  invoked.mockImplementation((command: string, args) =>
    Promise.resolve(
      command === "clip_cost"
        ? cost(
            // Cropped when asked about clip 2, in these tests.
            (args as { clips: number[] }).clips.includes(2),
          )
        : undefined,
    ),
  );
});

describe("the crop section", () => {
  it("shows an uncropped clip's whole picture and states the copy, from the plan", async () => {
    show([clip(1)]);
    expect(screen.getByRole("heading", { name: "Crop" })).toBeVisible();
    expect(field("Left").value).toBe("0");
    expect(field("Width").value).toBe("1920");
    expect(field("Height").value).toBe("1080");
    expect(screen.getByRole("combobox", { name: "Aspect" })).toHaveTextContent(
      "Source",
    );
    expect(
      await screen.findByText(
        "Not cropped: this clip's pictures are copied bit for bit.",
      ),
    ).toBeVisible();
    expect(invoked).toHaveBeenCalledWith("clip_cost", { clips: [1] });
    expect(screen.getByRole("button", { name: "Reset crop" })).toBeDisabled();
    // Only what exists: no scale or rotation.
    expect(
      screen.queryByRole("spinbutton", { name: /scale|rotat/i }),
    ).toBeNull();
  });

  it("states what a crop costs at the point of use", async () => {
    show([clip(2, VERTICAL)]);
    expect(field("Left").value).toBe("656");
    expect(field("Width").value).toBe("608");
    expect(screen.getByRole("combobox", { name: "Aspect" })).toHaveTextContent(
      "9:16",
    );
    const statement = await screen.findByText(
      "Cropping re-encodes this clip's pictures (4.2 s). Its sound is still copied.",
    );
    expect(statement).toHaveAttribute("data-tone", "re-encode");
  });

  it("asks the engine to fit a preset, as one edit", async () => {
    const user = userEvent.setup();
    show([clip(1)]);
    screen.getByRole("combobox", { name: "Aspect" }).focus();
    await user.keyboard("{Enter}");
    await user.click(
      await screen.findByRole("option", { name: "9:16 vertical" }),
    );
    expect(edit).toHaveBeenCalledWith({
      edit: "crop-to-aspect",
      clips: [1],
      aspect: "9:16",
    });
    expect(edit).toHaveBeenCalledTimes(1);
  });

  it("resets every selected clip's crop", () => {
    show([clip(2, VERTICAL), clip(3)]);
    fireEvent.click(screen.getByRole("button", { name: "Reset crop" }));
    expect(edit).toHaveBeenCalledWith({ edit: "reset-crop", clips: [2, 3] });
  });

  it("shows sides that differ as mixed and sets a side on every clip", async () => {
    show([clip(2, VERTICAL), clip(3)]);
    expect(field("Left").placeholder).toBe("Mixed");
    expect(field("Width").placeholder).toBe("Mixed");
    // Both are 1080 tall: that side is shared.
    expect(field("Height").value).toBe("1080");
    expect(screen.getByRole("combobox", { name: "Aspect" })).toHaveTextContent(
      "Mixed",
    );
    fireEvent.keyDown(field("Height"), { key: "ArrowDown" });
    fireEvent.blur(field("Height"));
    await waitFor(() => {
      expect(edit).toHaveBeenCalledWith({
        edit: "set-crop-sides",
        clips: [2, 3],
        x: null,
        y: null,
        width: null,
        height: 1078,
      });
    });
  });

  it("makes a field's changes one gesture, so one undo restores the value before it", async () => {
    show([clip(2, VERTICAL)]);
    const left = field("Left");
    fireEvent.keyDown(left, { key: "ArrowUp" });
    fireEvent.keyDown(left, { key: "ArrowUp" });
    fireEvent.blur(left);
    await waitFor(() => {
      expect(endGesture).toHaveBeenCalledTimes(1);
    });
    expect(beginGesture).toHaveBeenCalledTimes(1);
    expect(beginGesture).toHaveBeenCalledWith("Crop clip");
  });

  it("shows a refusal at the field it came from", async () => {
    edit.mockImplementation(() =>
      Promise.resolve(
        "clip 2: the crop runs outside the picture: it must lie within 1920 × 1080 pixels",
      ),
    );
    show([clip(2, VERTICAL)]);
    fireEvent.change(field("Width"), { target: { value: "1500" } });
    fireEvent.blur(field("Width"));
    const refusal = await screen.findByRole("alert");
    expect(refusal).toHaveTextContent("must lie within 1920 × 1080");
    // Beside the width, not the left edge or the whole section.
    expect(
      within(
        field("Width").closest(".crop-section__field") as HTMLElement,
      ).getByRole("alert"),
    ).toBe(refusal);
  });

  it("cannot crop a clip whose source is offline, but can reset it", () => {
    show([clip(2, VERTICAL)], {});
    expect(screen.getByText(/offline or could not be read/)).toBeVisible();
    expect(field("Width")).toBeDisabled();
    expect(screen.getByRole("combobox", { name: "Aspect" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reset crop" })).toBeEnabled();
  });

  it("says the cost is not stated when there is no plan", async () => {
    invoked.mockImplementation(() =>
      Promise.reject(
        new Error("source 3 has not been read yet, or is missing"),
      ),
    );
    show([clip(1)]);
    expect(await screen.findByText(/The cost is not stated/)).toBeVisible();
  });
});
