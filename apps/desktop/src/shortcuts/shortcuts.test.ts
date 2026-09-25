import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "@tauri-apps/api/core";
import ts from "typescript";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePreviewStore } from "../player/preview.store.js";
import { useProjectStore } from "../project/project.store.js";
import { useTimelineStore } from "../timeline/timeline.store.js";
import { HANDLERS } from "./handlers.js";
import {
  ACTIONS,
  BINDINGS,
  chordOf,
  displayChord,
  reference,
  type ActionId,
  type Chord,
} from "./shortcuts.js";
import { useShortcutsUi } from "./shortcuts.store.js";
import { dispatch, isTyping, shortcutFor } from "./useShortcuts.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

const invoked = vi.mocked(invoke);
const chords = Object.keys(BINDINGS) as Chord[];
const bound = BINDINGS as Partial<Record<Chord, ActionId>>;
/** The action `chord` is bound to; every chord in `chords` is. */
const actionOf = (chord: Chord): ActionId => bound[chord] ?? "undo";
const HERE = dirname(fileURLToPath(import.meta.url));

/** The key press a chord is, as a browser would report it. */
function press(chord: Chord, target: EventTarget = document.body) {
  const plus = chord.endsWith("++");
  const parts = plus
    ? [...chord.slice(0, -2).split("+"), "+"]
    : chord.split("+");
  const key = parts.at(-1) ?? "";
  const event = new KeyboardEvent("keydown", {
    key: key === "Space" ? " " : key.length === 1 ? key.toLowerCase() : key,
    ctrlKey: parts.includes("Ctrl"),
    shiftKey: parts.includes("Shift"),
    altKey: parts.includes("Alt"),
    bubbles: true,
    cancelable: true,
  });
  Object.defineProperty(event, "target", { value: target });
  return event;
}

describe("the registry", () => {
  it("reads every registered chord back from its own key press", () => {
    for (const chord of chords) expect(chordOf(press(chord))).toBe(chord);
  });

  it("binds every action, and only known actions", () => {
    const bound = new Set(Object.values(BINDINGS));
    expect([...bound].sort()).toEqual(Object.keys(ACTIONS).sort());
  });

  it("says, for every shortcut, whether it is CapCut's or why it is not", () => {
    for (const info of Object.values(ACTIONS)) expect(info.capcut).not.toBe("");
  });

  it("shows chords as they are printed on the key", () => {
    expect(displayChord("ArrowLeft")).toBe("←");
    expect(displayChord("Ctrl++")).toBe("Ctrl++");
    expect(displayChord("Ctrl+Shift+Z")).toBe("Ctrl+Shift+Z");
  });
});

describe("dispatch", () => {
  const spies = new Map<ActionId, ReturnType<typeof vi.fn>>();

  beforeEach(() => {
    for (const action of Object.keys(HANDLERS) as ActionId[]) {
      const spy = vi.fn(() => true);
      spies.set(action, spy);
      vi.spyOn(HANDLERS, action).mockImplementation(spy);
    }
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("runs every registered shortcut's action, and takes the key", () => {
    for (const chord of chords) {
      const event = press(chord);
      dispatch(event);
      expect(spies.get(actionOf(chord)), chord).toHaveBeenCalledTimes(1);
      expect(event.defaultPrevented, chord).toBe(true);
      spies.get(actionOf(chord))?.mockClear();
    }
  });

  it("never fires while a text field has focus", () => {
    const field = document.createElement("input");
    const area = document.createElement("textarea");
    for (const target of [field, area])
      for (const chord of chords) dispatch(press(chord, target));
    for (const spy of spies.values()) expect(spy).not.toHaveBeenCalled();
    expect(isTyping(field)).toBe(true);
    const range = document.createElement("input");
    range.type = "range";
    expect(isTyping(range)).toBe(false);
  });

  it("leaves a control its own keys, but plays from a focused button", () => {
    const slider = document.createElement("div");
    slider.setAttribute("role", "slider");
    for (const chord of [
      "ArrowLeft",
      "ArrowRight",
      "Home",
      "End",
      "Space",
    ] as const)
      expect(shortcutFor(press(chord, slider)), chord).toBeNull();
    // Ctrl+Z is not a slider's key.
    expect(shortcutFor(press("Ctrl+Z", slider))).toBe("undo");

    const button = document.createElement("button");
    const space = press("Space", button);
    dispatch(space);
    expect(spies.get("play-pause")).toHaveBeenCalledTimes(1);
    expect(space.defaultPrevented).toBe(true);
  });

  it("does nothing behind an open dialog", () => {
    const dialog = document.body.appendChild(document.createElement("div"));
    dialog.setAttribute("role", "dialog");
    const inside = dialog.appendChild(document.createElement("button"));
    dispatch(press("Space", inside));
    dispatch(press("Ctrl+Z", inside));
    expect(spies.get("play-pause")).not.toHaveBeenCalled();
    expect(spies.get("undo")).not.toHaveBeenCalled();
    dialog.remove();
  });

  it("ignores a held toggle but repeats a step", () => {
    const held = (chord: Chord) => {
      const event = press(chord);
      Object.defineProperty(event, "repeat", { value: true });
      return event;
    };
    dispatch(held("Space"));
    dispatch(held("ArrowRight"));
    expect(spies.get("play-pause")).not.toHaveBeenCalled();
    expect(spies.get("next-frame")).toHaveBeenCalledTimes(1);
  });

  it("leaves a key alone when its action has nothing to act on", () => {
    spies.get("copy")?.mockReturnValue(false);
    const event = press("Ctrl+C");
    dispatch(event);
    expect(event.defaultPrevented).toBe(false);
  });
});

describe("what the shortcuts do", () => {
  beforeEach(() => {
    invoked.mockReset();
    invoked.mockResolvedValue(undefined);
    usePreviewStore.setState({
      session: 3,
      framePosition: 1_000_000,
      marks: { in: null, out: null },
      playback: {
        state: "paused",
        position: 1_000_000,
        duration: 4_000_000,
        frameRate: { num: 25, den: 1 },
        timecode: "00:00:01:00",
        durationTimecode: "00:00:04:00",
        speed: "normal",
        loopRange: null,
        audio: { kind: "device", name: "Speakers" },
        resolving: false,
        proxy: false,
      },
    });
  });

  const transportSent = () =>
    invoked.mock.calls
      .filter(([name]) => name === "transport")
      .map(([, args]) => (args as { command: object }).command);

  it("drives the transport", () => {
    for (const chord of [
      "Space",
      "ArrowLeft",
      "ArrowRight",
      "Home",
      "End",
      "J",
      "K",
      "L",
    ] as const)
      dispatch(press(chord));
    expect(transportSent()).toEqual([
      { type: "toggle" },
      { type: "step", frames: -1 },
      { type: "step", frames: 1 },
      { type: "jump-to-start" },
      { type: "jump-to-end" },
      { type: "step", frames: -25 },
      { type: "pause" },
      { type: "play" },
    ]);
  });

  it("does not take the transport keys with no preview open", () => {
    usePreviewStore.setState({ session: null });
    const event = press("Space");
    dispatch(event);
    expect(event.defaultPrevented).toBe(false);
    expect(invoked).not.toHaveBeenCalled();
  });

  it("sets the in and out points of the loop", async () => {
    dispatch(press("I"));
    usePreviewStore.setState({ framePosition: 2_000_000 });
    dispatch(press("O"));
    await vi.waitFor(() =>
      expect(usePreviewStore.getState().marks).toEqual({
        in: 1_000_000,
        out: 2_000_000,
      }),
    );
  });

  it("toggles snapping and opens the reference", () => {
    useTimelineStore.setState({ snapping: true });
    dispatch(press("N"));
    expect(useTimelineStore.getState().snapping).toBe(false);
    dispatch(press("N"));
    expect(useTimelineStore.getState().snapping).toBe(true);
    dispatch(press("Ctrl+/"));
    expect(useShortcutsUi.getState().referenceOpen).toBe(true);
  });

  it("copies the selection and pastes it at the playhead as one edit", async () => {
    useProjectStore.setState({
      view: { timeline: { tracks: [] } } as never,
      selection: [4, 5],
    });
    useTimelineStore.setState({ playhead: 90, clipboard: [] });
    dispatch(press("Ctrl+C"));
    expect(useTimelineStore.getState().clipboard).toEqual([4, 5]);
    dispatch(press("Ctrl+V"));
    await vi.waitFor(() =>
      expect(
        invoked.mock.calls.find(([name]) => name === "edit_project")?.[1],
      ).toMatchObject({ edit: { edit: "paste", clips: [4, 5], at: 90 } }),
    );
  });

  it("says export is not available rather than doing nothing", () => {
    dispatch(press("Ctrl+E"));
    expect(useTimelineStore.getState().notice).toBe(
      "Export is not available yet.",
    );
  });
});

describe("the reference and the document", () => {
  it("lists every action and every chord of the registry, once", () => {
    const rows = reference().flatMap((group) => group.rows);
    expect(rows.map((row) => row.action).sort()).toEqual(
      Object.keys(ACTIONS).sort(),
    );
    expect(rows.flatMap((row) => row.keys).sort()).toEqual(
      chords.map(displayChord).sort(),
    );
  });

  it("matches docs/design/shortcuts.md row for row", () => {
    const doc = readFileSync(
      join(HERE, "..", "..", "..", "..", "docs", "design", "shortcuts.md"),
      "utf8",
    );
    const cells = (line: string) =>
      line
        .split("|")
        .slice(1, -1)
        .map((cell) => cell.trim());
    const documented = new Map(
      doc
        .split("\n")
        .filter((line) => line.startsWith("| "))
        .map(cells)
        .filter((row) => row.length === 3)
        .map(([label, keys, capcut]) => [label, { keys, capcut }]),
    );
    for (const row of reference().flatMap((group) => group.rows)) {
      expect(documented.get(row.label), row.label).toEqual({
        keys: row.keys.join(", "),
        capcut: row.capcut,
      });
    }
  });
});

describe("conflicts", () => {
  /** Type-check `source` beside the registry, as the build would. */
  function diagnostics(source: string): readonly ts.Diagnostic[] {
    const registry = join(HERE, "shortcuts.ts");
    // TypeScript names files with forward slashes, on Windows too.
    const probe = registry
      .replaceAll("\\", "/")
      .replace(/shortcuts\.ts$/, "__conflict_probe__.ts");
    const options: ts.CompilerOptions = {
      strict: true,
      noEmit: true,
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.ESNext,
      moduleResolution: ts.ModuleResolutionKind.Bundler,
      allowImportingTsExtensions: true,
      skipLibCheck: true,
    };
    const host = ts.createCompilerHost(options);
    const read = host.readFile.bind(host);
    const exists = host.fileExists.bind(host);
    const get = host.getSourceFile.bind(host);
    host.readFile = (file) => (file === probe ? source : read(file));
    host.fileExists = (file) => file === probe || exists(file);
    host.getSourceFile = (file, version, ...rest) =>
      file === probe
        ? ts.createSourceFile(file, source, version)
        : get(file, version, ...rest);
    const program = ts.createProgram([probe], options, host);
    return ts
      .getPreEmitDiagnostics(program)
      .filter((d) => d.file?.fileName === probe);
  }

  const header = `import type { ActionId, Chord } from "./shortcuts.ts";\n`;

  it("fails the build on a chord bound twice", () => {
    const found = diagnostics(
      `${header}export const B = { "Ctrl+B": "split", "Ctrl+B": "copy" } as const satisfies Partial<Record<Chord, ActionId>>;\n`,
    );
    expect(found.map((d) => d.code)).toContain(1117);
  }, 60_000);

  it("fails the build on a key Windows keeps, or on AltGr", () => {
    for (const chord of ["Alt+F4", "Ctrl+Alt+E"]) {
      const found = diagnostics(
        `${header}export const B = { "${chord}": "export" } as const satisfies Partial<Record<Chord, ActionId>>;\n`,
      );
      expect(found.length, chord).toBeGreaterThan(0);
    }
  }, 60_000);

  it("accepts the registry as it is", () => {
    expect(
      diagnostics(
        `${header}export const B = { "Ctrl+B": "split" } as const satisfies Partial<Record<Chord, ActionId>>;\n`,
      ),
    ).toEqual([]);
  }, 60_000);
});
