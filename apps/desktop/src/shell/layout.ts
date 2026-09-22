/**
 * The four-zone layout contract (#19).
 *
 * Every number here comes from `docs/design/capcut-layout-reference.md`, which
 * #18 established and the owner signed off. It is not a second opinion about
 * the sizes — `layout.test.ts` parses that document's table and fails if the
 * two disagree, so the specification and the shell cannot drift apart.
 *
 * ── Why fractions and not pixels ────────────────────────────────────────────
 *
 * A stored pixel width is right for the window it was stored in and wrong for
 * every other one: maximise onto a 4K monitor and the library panel stays the
 * width it was on a laptop, leaving the player with a third of the screen it
 * should have. Fractions scale, and the pixel minimums below are what stops a
 * fraction becoming unusable on a small window.
 *
 * Both limits are applied twice, deliberately:
 *
 *   in JavaScript  while dragging, so the value that gets persisted is sane
 *   in CSS         with `clamp()`, so shrinking the *window* cannot push a zone
 *                  under its minimum without anybody dragging anything
 *
 * One without the other leaves a hole. Clamping only in JavaScript means a
 * window resize breaks the minimum; clamping only in CSS means the stored
 * fraction wanders off and comes back wrong on the next launch.
 */

/** The zones a splitter can resize. The player takes what is left. */
export const RESIZABLE_ZONES = ["library", "inspector", "timeline"] as const;

export type ResizableZone = (typeof RESIZABLE_ZONES)[number];

export interface ZoneBounds {
  /** Fraction of the container's width or height, 0 to 1. */
  readonly defaultFraction: number;
  /** Below this the zone stops being usable. In CSS pixels. */
  readonly minimumPx: number;
  /** Above this the zone is eating the player. As a fraction. */
  readonly maximumFraction: number;
}

/**
 * The documented bounds, zone by zone.
 *
 * The reasons are in the reference document and are not repeated here; what is
 * worth repeating is the one that decides arguments. When the window is small,
 * the space goes to the player. It is the reason the application exists, and
 * every minimum below is set so that it still has 480px at the 1280px window
 * minimum.
 */
export const ZONE_BOUNDS: Record<ResizableZone, ZoneBounds> = {
  library: { defaultFraction: 0.22, minimumPx: 240, maximumFraction: 0.4 },
  inspector: { defaultFraction: 0.2, minimumPx: 260, maximumFraction: 0.4 },
  timeline: { defaultFraction: 0.38, minimumPx: 180, maximumFraction: 0.7 },
};

/** The player's floor. Not resizable directly — it is what the others leave. */
export const PLAYER_MINIMUM_PX = 480;

/** The application bar. Fixed: a bar the user can drag is a bar they can lose. */
export const APP_BAR_HEIGHT_PX = 48;

/** Below this the four-zone layout stops being four zones and becomes four slivers. */
export const WINDOW_MINIMUM = { width: 1280, height: 800 } as const;

/**
 * How far one arrow-key press moves a splitter, in CSS pixels.
 *
 * `--space-4`. A splitter that moves one pixel per press is a control nobody
 * operates from the keyboard twice, and one that moves fifty cannot be aimed.
 */
export const SPLITTER_KEYBOARD_STEP_PX = 16;

/** The CSS custom property a zone's fraction is published as. */
export function fractionVariable(zone: ResizableZone): string {
  return `--layout-${zone}-fraction`;
}

/**
 * Clamp a fraction to the zone's documented bounds, given the space available.
 *
 * `containerPx` is the width for a vertical splitter and the height for a
 * horizontal one. A container smaller than the minimum returns the minimum's
 * fraction — above 1 — rather than silently returning something that fits:
 * the layout is then honestly broken and the CSS `clamp()` shows it, which is
 * better than a zone quietly disappearing. The window minimum exists so this
 * case cannot arise in the application.
 */
export function clampFraction(
  zone: ResizableZone,
  fraction: number,
  containerPx: number,
): number {
  const bounds = ZONE_BOUNDS[zone];
  if (!Number.isFinite(fraction) || containerPx <= 0) {
    return bounds.defaultFraction;
  }

  const minimumFraction = bounds.minimumPx / containerPx;
  return Math.min(bounds.maximumFraction, Math.max(minimumFraction, fraction));
}

/** Every zone at its documented default. The layout on a first launch. */
export function defaultLayout(): Record<ResizableZone, number> {
  return {
    library: ZONE_BOUNDS.library.defaultFraction,
    inspector: ZONE_BOUNDS.inspector.defaultFraction,
    timeline: ZONE_BOUNDS.timeline.defaultFraction,
  };
}
