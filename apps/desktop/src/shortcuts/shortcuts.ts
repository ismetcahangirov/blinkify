/**
 * Every keyboard shortcut in Blinkify, in one place (#38).
 *
 * `docs/design/shortcuts.md` is the same table in prose; the in-app
 * reference is generated from this file; one listener dispatches from it
 * (`useShortcuts`). Nothing else in the renderer binds a key, so a binding
 * cannot be registered twice or left firing after the component that
 * registered it has gone.
 *
 * ── Conflicts fail the build ───────────────────────────────────────────────
 *
 * `BINDINGS` is keyed by chord. Two bindings of one chord are two properties
 * of the same name in one object literal, which TypeScript rejects (TS1117)
 * — so a duplicate fails `pnpm typecheck` and the build, not a user. Chords
 * are written in one canonical form (`Ctrl+Shift+Alt+Key`, typed by `Chord`),
 * so the same chord cannot be spelled two ways to slip past the check.
 *
 * ── Keys Windows keeps ─────────────────────────────────────────────────────
 *
 * `Chord` excludes what Windows takes before an application sees it (Alt+Tab,
 * Alt+F4, Ctrl+Esc, …) and every Ctrl+Alt combination, which is AltGr on the
 * keyboards that type ə, ş and ğ. Binding one is a type error.
 *
 * Customisable shortcuts are out of scope for v1. The map is data, so a user
 * layer over `BINDINGS` is an addition, not a rewrite.
 */

type Letter =
  | "A"
  | "B"
  | "C"
  | "D"
  | "E"
  | "F"
  | "G"
  | "H"
  | "I"
  | "J"
  | "K"
  | "L"
  | "M"
  | "N"
  | "O"
  | "P"
  | "Q"
  | "R"
  | "S"
  | "T"
  | "U"
  | "V"
  | "W"
  | "X"
  | "Y"
  | "Z";
type Digit = "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9";
type Named =
  | "Space"
  | "ArrowLeft"
  | "ArrowRight"
  | "ArrowUp"
  | "ArrowDown"
  | "Home"
  | "End"
  | "PageUp"
  | "PageDown"
  | "Delete"
  | "Backspace"
  | "Enter"
  | "Escape"
  | "Tab"
  | "F1"
  | "F2"
  | "F3"
  | "F4"
  | "F5"
  | "F6"
  | "F7"
  | "F8"
  | "F9"
  | "F10"
  | "F11"
  | "F12";
/** Symbols as typed. Shift is never part of their chord: it is what made them. */
type Punctuation = "=" | "+" | "-" | "/" | "," | "." | "[" | "]";
export type Key = Letter | Digit | Named | Punctuation;

type Modifiers =
  "" | "Ctrl+" | "Shift+" | "Alt+" | "Ctrl+Shift+" | "Shift+Alt+";

/** What Windows handles itself, before any application sees the key. */
type Reserved =
  | "Alt+Tab"
  | "Shift+Alt+Tab"
  | "Alt+F4"
  | "Alt+Escape"
  | "Alt+Space"
  | "Ctrl+Escape"
  | "Ctrl+Shift+Escape"
  | "F12";

export type Chord = Exclude<`${Modifiers}${Key}`, Reserved>;

export type Group = "Transport" | "Editing" | "Timeline" | "Application";

export type ActionId =
  | "play-pause"
  | "previous-frame"
  | "next-frame"
  | "jump-to-start"
  | "jump-to-end"
  | "shuttle-back"
  | "shuttle-pause"
  | "shuttle-forward"
  | "mark-in"
  | "mark-out"
  | "undo"
  | "redo"
  | "copy"
  | "paste"
  | "duplicate"
  | "delete"
  | "ripple-delete"
  | "select-all"
  | "split"
  | "zoom-in"
  | "zoom-out"
  | "zoom-to-fit"
  | "toggle-snapping"
  | "new-project"
  | "open-project"
  | "save"
  | "save-as"
  | "export"
  | "show-shortcuts";

export interface ActionInfo {
  readonly label: string;
  readonly group: Group;
  /** "Same" where CapCut's desktop editor uses the same key; otherwise
   * what CapCut does and why Blinkify differs. */
  readonly capcut: string;
  /** Held down, the key repeats the action. Off for toggles. */
  readonly repeats: boolean;
}

const SAME = "Same";
const NONE = "No CapCut shortcut";

export const ACTIONS: Readonly<Record<ActionId, ActionInfo>> = {
  "play-pause": {
    label: "Play / pause",
    group: "Transport",
    capcut: SAME,
    repeats: false,
  },
  "previous-frame": {
    label: "Previous frame",
    group: "Transport",
    capcut: SAME,
    repeats: true,
  },
  "next-frame": {
    label: "Next frame",
    group: "Transport",
    capcut: SAME,
    repeats: true,
  },
  "jump-to-start": {
    label: "Jump to the start",
    group: "Transport",
    capcut: `${NONE}. Home and End are where every other editor puts them.`,
    repeats: false,
  },
  "jump-to-end": {
    label: "Jump to the last frame",
    group: "Transport",
    capcut: `${NONE}. Home and End are where every other editor puts them.`,
    repeats: false,
  },
  "shuttle-back": {
    label: "Back one second; held, keeps going",
    group: "Transport",
    capcut:
      "CapCut plays in reverse. Blinkify has no reverse play in v1, so J steps back.",
    repeats: true,
  },
  "shuttle-pause": {
    label: "Pause",
    group: "Transport",
    capcut: SAME,
    repeats: false,
  },
  "shuttle-forward": {
    label: "Play; again while playing, double speed",
    group: "Transport",
    capcut: SAME,
    repeats: false,
  },
  "mark-in": {
    label: "Set the in point of the loop range",
    group: "Transport",
    capcut: `${NONE}. I is the in point in Premiere Pro and DaVinci Resolve.`,
    repeats: false,
  },
  "mark-out": {
    label: "Set the out point of the loop range",
    group: "Transport",
    capcut: `${NONE}. O is the out point in Premiere Pro and DaVinci Resolve.`,
    repeats: false,
  },
  undo: { label: "Undo", group: "Editing", capcut: SAME, repeats: true },
  redo: {
    label: "Redo",
    group: "Editing",
    capcut:
      "Same for Ctrl+Shift+Z. Ctrl+Y is kept as well, because it is Windows' own redo.",
    repeats: true,
  },
  copy: {
    label: "Copy the selected clips",
    group: "Editing",
    capcut: SAME,
    repeats: false,
  },
  paste: {
    label: "Paste at the playhead",
    group: "Editing",
    capcut:
      "Same key. The paste is inserted: later clips on every unlocked track move along, so nothing goes out of sync.",
    repeats: false,
  },
  duplicate: {
    label: "Duplicate the selected clips",
    group: "Editing",
    capcut: SAME,
    repeats: false,
  },
  delete: {
    label: "Delete the selected clips",
    group: "Editing",
    capcut: SAME,
    repeats: false,
  },
  "ripple-delete": {
    label: "Ripple delete: delete and close the gaps",
    group: "Editing",
    capcut: `${NONE}: CapCut's main track closes gaps on its own. Shift+Delete is Premiere Pro's.`,
    repeats: false,
  },
  "select-all": {
    label: "Select every clip",
    group: "Editing",
    capcut: SAME,
    repeats: false,
  },
  split: {
    label: "Split at the playhead",
    group: "Editing",
    capcut: SAME,
    repeats: false,
  },
  "zoom-in": {
    label: "Zoom in",
    group: "Timeline",
    capcut: SAME,
    repeats: true,
  },
  "zoom-out": {
    label: "Zoom out",
    group: "Timeline",
    capcut: SAME,
    repeats: true,
  },
  "zoom-to-fit": {
    label: "Fit the whole timeline in view",
    group: "Timeline",
    capcut: `${NONE}. Shift+Z is DaVinci Resolve's zoom to fit.`,
    repeats: false,
  },
  "toggle-snapping": {
    label: "Turn snapping on or off",
    group: "Timeline",
    capcut: `${NONE}: CapCut has a button. N is DaVinci Resolve's.`,
    repeats: false,
  },
  "new-project": {
    label: "New project",
    group: "Application",
    capcut: SAME,
    repeats: false,
  },
  "open-project": {
    label: "Open a project",
    group: "Application",
    capcut: SAME,
    repeats: false,
  },
  save: { label: "Save", group: "Application", capcut: SAME, repeats: false },
  "save-as": {
    label: "Save as",
    group: "Application",
    capcut: `${NONE}: CapCut saves to its own library. Ctrl+Shift+S is Windows' save as.`,
    repeats: false,
  },
  export: {
    label: "Export",
    group: "Application",
    capcut: SAME,
    repeats: false,
  },
  "show-shortcuts": {
    label: "Show keyboard shortcuts",
    group: "Application",
    capcut: `${NONE}. Ctrl+/ is the reference in most Windows applications.`,
    repeats: false,
  },
};

/** Every binding: chord to action. See the file comment for why it is keyed
 * this way round. */
export const BINDINGS = {
  Space: "play-pause",
  ArrowLeft: "previous-frame",
  ArrowRight: "next-frame",
  Home: "jump-to-start",
  End: "jump-to-end",
  J: "shuttle-back",
  K: "shuttle-pause",
  L: "shuttle-forward",
  I: "mark-in",
  O: "mark-out",
  "Ctrl+Z": "undo",
  "Ctrl+Shift+Z": "redo",
  "Ctrl+Y": "redo",
  "Ctrl+C": "copy",
  "Ctrl+V": "paste",
  "Ctrl+D": "duplicate",
  Delete: "delete",
  Backspace: "delete",
  "Shift+Delete": "ripple-delete",
  "Shift+Backspace": "ripple-delete",
  "Ctrl+A": "select-all",
  "Ctrl+B": "split",
  "Ctrl+=": "zoom-in",
  "Ctrl++": "zoom-in",
  "Ctrl+-": "zoom-out",
  "Shift+Z": "zoom-to-fit",
  N: "toggle-snapping",
  "Ctrl+N": "new-project",
  "Ctrl+O": "open-project",
  "Ctrl+S": "save",
  "Ctrl+Shift+S": "save-as",
  "Ctrl+E": "export",
  "Ctrl+/": "show-shortcuts",
} as const satisfies Partial<Record<Chord, ActionId>>;

/** A chord as it is shown: `Ctrl+Shift+Z`, `←`, `Space`. */
export function displayChord(chord: Chord): string {
  const glyphs: Partial<Record<string, string>> = {
    ArrowLeft: "←",
    ArrowRight: "→",
    ArrowUp: "↑",
    ArrowDown: "↓",
  };
  const parts = chord.endsWith("++")
    ? [...chord.slice(0, -2).split("+").filter(Boolean), "+"]
    : chord.split("+");
  return parts.map((part) => glyphs[part] ?? part).join("+");
}

/** Every chord bound to `action`, in the order they are listed. */
export function chordsFor(action: ActionId): Chord[] {
  return (Object.entries(BINDINGS) as [Chord, ActionId][])
    .filter(([, bound]) => bound === action)
    .map(([chord]) => chord);
}

/** The first chord of `action`, as shown in a tooltip or a menu. */
export function keyFor(action: ActionId): string {
  const [first] = chordsFor(action);
  return first ? displayChord(first) : "";
}

/** The chord a key press is, in canonical form; `null` for a key that is
 * not one Blinkify can bind (a modifier on its own, a dead key). */
export function chordOf(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): string | null {
  const raw = event.key;
  if (raw === "Control" || raw === "Shift" || raw === "Alt" || raw === "Meta")
    return null;
  const key =
    raw === " " ? "Space" : raw.length === 1 ? raw.toUpperCase() : raw;
  // A symbol's Shift is what typed it: Ctrl+Shift+= arrives as "+".
  const symbol = key.length === 1 && !/[A-Z0-9]/.test(key);
  const ctrl = event.ctrlKey || event.metaKey;
  return `${ctrl ? "Ctrl+" : ""}${event.shiftKey && !symbol ? "Shift+" : ""}${event.altKey ? "Alt+" : ""}${key}`;
}

/** The action a key press is bound to, if any. */
export function actionFor(chord: string | null): ActionId | null {
  if (chord === null) return null;
  return (BINDINGS as Partial<Record<string, ActionId>>)[chord] ?? null;
}

/** The reference, as the in-app dialog and the docs list it: grouped, in
 * the order of `ACTIONS`, every chord of each action. */
export interface ReferenceRow {
  readonly action: ActionId;
  readonly label: string;
  readonly keys: readonly string[];
  readonly capcut: string;
}

export function reference(): { group: Group; rows: ReferenceRow[] }[] {
  const groups: Group[] = ["Transport", "Editing", "Timeline", "Application"];
  return groups.map((group) => ({
    group,
    rows: (Object.entries(ACTIONS) as [ActionId, ActionInfo][])
      .filter(([, info]) => info.group === group)
      .map(([action, info]) => ({
        action,
        label: info.label,
        keys: chordsFor(action).map(displayChord),
        capcut: info.capcut,
      })),
  }));
}
