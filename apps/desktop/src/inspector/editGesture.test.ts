import type { Edit } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useProjectStore } from "../project/project.store.js";
import { editGesture } from "./editGesture.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

const speed = (num: number): Edit => ({
  edit: "set-speed",
  clips: [7],
  ratio: { num, den: 100 },
});

describe("an edit gesture", () => {
  beforeEach(() => {
    invoked.mockReset();
    invoked.mockImplementation((command: string) =>
      Promise.resolve(
        command === "edit_project"
          ? {
              view: useProjectStore.getState().view,
              context: { selection: [7], playhead: 0 },
              clamped: false,
            }
          : command === "end_gesture"
            ? useProjectStore.getState().view
            : undefined,
      ),
    );
    // Any view will do: the gesture only needs a project to be open.
    useProjectStore.setState({
      view: {
        project: { sequence: { settings: { frameRate: { num: 30, den: 1 } } } },
      } as never,
      selection: [7],
    });
  });

  it("opens once, applies in order, and closes once: one undo entry", async () => {
    const gesture = editGesture("Change speed");
    for (let step = 101; step <= 120; step += 1) gesture.change(speed(step));
    await gesture.end();

    const commands = invoked.mock.calls.map(([command]) => command);
    expect(commands[0]).toBe("begin_gesture");
    expect(commands.at(-1)).toBe("end_gesture");
    expect(commands.filter((c) => c === "begin_gesture")).toHaveLength(1);
    expect(commands.filter((c) => c === "end_gesture")).toHaveLength(1);

    // Edits that were overtaken are skipped, never reordered: the last
    // one sent is the last value asked for.
    const sent = invoked.mock.calls
      .filter(([command]) => command === "edit_project")
      .map(([, args]) => (args as { edit: Edit }).edit);
    expect(sent.at(-1)).toEqual(speed(120));
    const nums = sent.map((e) => (e.edit === "set-speed" ? e.ratio.num : 0));
    expect([...nums].sort((a, b) => a - b)).toEqual(nums);
  });

  it("does nothing when nothing changed", async () => {
    await editGesture("Change speed").end();
    expect(invoked).not.toHaveBeenCalled();
  });
});
