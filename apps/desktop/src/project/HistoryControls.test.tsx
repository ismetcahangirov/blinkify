import type { ProjectView } from "@blinkify/types";
import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  HistoryControls,
  historyActionForKey,
  isTyping,
} from "./HistoryControls.js";
import { useProjectStore } from "./project.store.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invoked = vi.mocked(invoke);

function view(entries: string[], applied: number): ProjectView {
  return {
    path: "C:\\Work\\Trip.blinkify",
    project: {
      schemaVersion: 1,
      name: "Trip",
      sources: {},
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
    timeline: null,
    history: { entries, applied },
    unavailable: {},
    affectedClips: [],
  };
}

const key = (key: string, modifiers: Partial<KeyboardEvent> = {}) => ({
  key,
  ctrlKey: false,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  ...modifiers,
});

function show() {
  render(
    <TooltipProvider>
      <HistoryControls />
    </TooltipProvider>,
  );
}

describe("history controls", () => {
  beforeEach(() => {
    invoked.mockReset();
    useProjectStore.setState({ view: null, selection: [] });
  });

  it("maps the standard keys to undo and redo", () => {
    expect(historyActionForKey(key("z", { ctrlKey: true }))).toBe("undo");
    expect(historyActionForKey(key("y", { ctrlKey: true }))).toBe("redo");
    expect(
      historyActionForKey(key("Z", { ctrlKey: true, shiftKey: true })),
    ).toBe("redo");
    expect(historyActionForKey(key("z"))).toBeNull();
    expect(
      historyActionForKey(key("z", { ctrlKey: true, altKey: true })),
    ).toBeNull();
  });

  it("leaves a text field its own undo", () => {
    const input = document.createElement("input");
    expect(isTyping(input)).toBe(true);
    const range = document.createElement("input");
    range.type = "range";
    expect(isTyping(range)).toBe(false);
    expect(isTyping(document.createElement("textarea"))).toBe(true);
    expect(isTyping(document.createElement("button"))).toBe(false);

    useProjectStore.setState({ view: view(["Move clip"], 1) });
    show();
    const field = document.body.appendChild(document.createElement("input"));
    fireEvent.keyDown(field, { key: "z", ctrlKey: true });
    expect(invoked).not.toHaveBeenCalled();
    fireEvent.keyDown(document.body, { key: "z", ctrlKey: true });
    expect(invoked).toHaveBeenCalledWith("undo_edit");
  });

  it("disables what cannot be done and names what can", () => {
    useProjectStore.setState({ view: view(["Move clip", "Trim clip"], 1) });
    show();
    expect(
      screen.getByRole("button", { name: "Undo Move clip" }),
    ).toBeEnabled();
    expect(
      screen.getByRole("button", { name: "Redo Trim clip" }),
    ).toBeEnabled();

    act(() => useProjectStore.setState({ view: view([], 0) }));
    expect(screen.getByRole("button", { name: "Undo" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Redo" })).toBeDisabled();
  });

  it("steps through the history to the entry chosen", async () => {
    const entries = ["Add clip", "Move clip", "Trim clip"];
    useProjectStore.setState({ view: view(entries, 3) });
    let applied = 3;
    invoked.mockImplementation((command) => {
      applied += command === "undo_edit" ? -1 : 1;
      return Promise.resolve({
        view: view(entries, applied),
        context: { selection: [], playhead: 0 },
      });
    });
    show();
    await userEvent.click(screen.getByRole("button", { name: "History" }));
    const list = await screen.findByRole("list", { name: "Edit history" });
    expect(list.querySelector('[aria-current="step"]')?.textContent).toBe(
      "Trim clip",
    );
    await userEvent.click(screen.getByRole("button", { name: "Add clip" }));
    await waitFor(() => expect(applied).toBe(1));
    expect(invoked.mock.calls.map(([command]) => command)).toEqual([
      "undo_edit",
      "undo_edit",
    ]);
  });
});
