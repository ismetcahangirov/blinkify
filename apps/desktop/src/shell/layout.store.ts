import { create } from "zustand";
import {
  clampFraction,
  defaultLayout,
  RESIZABLE_ZONES,
  type ResizableZone,
} from "./layout.js";

/**
 * Where the splitter positions live between launches.
 *
 * ── Why this is not in the project file ─────────────────────────────────────
 *
 * #19: "persist layout state outside the project file — it is a user
 * preference, not part of the document". Panel widths belong to the person and
 * their monitor. Copying a project to another machine must not bring somebody
 * else's panel widths with it, and two people opening the same project should
 * not fight over the inspector.
 *
 * ── Why `localStorage` and not a Tauri command ─────────────────────────────
 *
 * The window's own geometry is persisted in Rust, because the window exists
 * before the renderer does and nothing in JavaScript can be asked where to put
 * it. The splitters are the opposite: they are renderer state, they are read
 * and written only by the renderer, and routing them through IPC would add a
 * command pair and a file format to version in order to store three numbers the
 * WebView already persists per user.
 *
 * The split is along a real line — the operating system's business against the
 * renderer's — rather than an arbitrary one. See `window_state.rs` for the
 * other half.
 *
 * ── Why nothing subscribes to this for rendering ───────────────────────────
 *
 * The store is read once, on mount, to seed the CSS custom properties, and
 * written once per drag, on release. During a drag the DOM is the source of
 * truth and React is not involved at all — which is what makes the acceptance
 * criterion "dragging a splitter does not cause a re-render in the zones being
 * resized" true by construction rather than by memoisation. See `AppShell`.
 */

const STORAGE_KEY = "blinkify.layout.v1";

export interface LayoutState {
  readonly fractions: Record<ResizableZone, number>;
  /** Commit a dragged or stepped position. Called on release, not per frame. */
  readonly setFraction: (zone: ResizableZone, fraction: number) => void;
  /** Back to the documented defaults. */
  readonly reset: () => void;
}

/**
 * Read the persisted layout, falling back to the defaults for anything missing,
 * malformed or out of range.
 *
 * Every failure here is silent and recovers to a working layout on purpose.
 * A corrupted preference must not stop the application opening — the worst
 * outcome it deserves is panels at their default widths, which is exactly what
 * a first launch looks like.
 */
export function readStoredLayout(): Record<ResizableZone, number> {
  const fractions = defaultLayout();

  let raw: string | null;
  try {
    raw = globalThis.localStorage.getItem(STORAGE_KEY);
  } catch {
    // A private window, a cleared profile, storage disabled by policy. All of
    // them mean "no preference stored", which is the default layout.
    return fractions;
  }
  if (raw === null) return fractions;

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return fractions;
  }
  if (typeof parsed !== "object" || parsed === null) return fractions;

  const record = parsed as Record<string, unknown>;
  for (const zone of RESIZABLE_ZONES) {
    const value = record[zone];
    /* Bounds are re-applied on read rather than trusted. The file is on the
       user's disk and a stored 0.99 would leave the player with nothing; a
       stored fraction is a hint, not an instruction. The container size is not
       known here, so only the fractional maximum is enforced — the pixel
       minimum is applied by the CSS `clamp()` once the element has a width. */
    if (typeof value === "number" && Number.isFinite(value) && value > 0) {
      fractions[zone] = clampFraction(zone, value, Number.POSITIVE_INFINITY);
    }
  }

  return fractions;
}

function writeStoredLayout(fractions: Record<ResizableZone, number>): void {
  try {
    globalThis.localStorage.setItem(STORAGE_KEY, JSON.stringify(fractions));
  } catch {
    // Losing a panel width is not worth failing an edit over.
  }
}

export const useLayoutStore = create<LayoutState>((set, get) => ({
  fractions: readStoredLayout(),

  setFraction: (zone, fraction) => {
    const fractions = { ...get().fractions, [zone]: fraction };
    set({ fractions });
    writeStoredLayout(fractions);
  },

  reset: () => {
    const fractions = defaultLayout();
    set({ fractions });
    writeStoredLayout(fractions);
  },
}));

/** The key the layout is stored under. Exported for the persistence test. */
export const LAYOUT_STORAGE_KEY = STORAGE_KEY;
