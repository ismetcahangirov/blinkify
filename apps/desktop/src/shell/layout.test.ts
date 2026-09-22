import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  APP_BAR_HEIGHT_PX,
  clampFraction,
  PLAYER_MINIMUM_PX,
  RESIZABLE_ZONES,
  WINDOW_MINIMUM,
  ZONE_BOUNDS,
  type ResizableZone,
} from "./layout.js";

/**
 * The layout contract, checked against the document it comes from (#19).
 *
 * Three copies of the same numbers exist by necessity: the reference document
 * that #18 established, `layout.ts` where the drag handler reads them, and
 * `shell.css` where the grid does. None of them can be derived from the others
 * — a stylesheet cannot import a Markdown table, and a `clamp()` cannot be
 * written in TypeScript.
 *
 * So they are compared instead. Change any one of the three and this fails,
 * which is the only version of "keep them in step" that survives a year.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = join(HERE, "..", "..", "..", "..");

const REFERENCE = readFileSync(
  join(REPOSITORY_ROOT, "docs", "design", "capcut-layout-reference.md"),
  "utf8",
);
const SHELL_CSS = readFileSync(join(HERE, "shell.css"), "utf8");

interface DocumentedZone {
  readonly defaultText: string;
  readonly minimumText: string;
  readonly maximumText: string;
}

/**
 * Read the sizes table out of the reference document.
 *
 * Cells are split and trimmed rather than matched as whole lines, because
 * Prettier pads the columns of a Markdown table and an exact-text assertion
 * would fail on formatting rather than on a wrong number.
 */
function documentedZones(markdown: string): Map<string, DocumentedZone> {
  const zones = new Map<string, DocumentedZone>();

  for (const line of markdown.split("\n")) {
    if (!line.trimStart().startsWith("| `")) continue;

    const cells = line
      .split("|")
      .slice(1, -1)
      .map((cell) => cell.trim().replace(/`/g, ""));

    const [zone, defaultText, minimumText, maximumText] = cells;
    if (
      zone === undefined ||
      defaultText === undefined ||
      minimumText === undefined ||
      maximumText === undefined
    ) {
      continue;
    }

    zones.set(zone, { defaultText, minimumText, maximumText });
  }

  return zones;
}

const documented = documentedZones(REFERENCE);

/** The first number in a cell: "22% of width" is 22, "240px" is 240. */
function firstNumber(text: string): number {
  const match = /-?\d+(?:\.\d+)?/.exec(text);
  expect(match, `no number in ${JSON.stringify(text)}`).not.toBeNull();
  return Number.parseFloat(match?.[0] ?? "NaN");
}

/** A custom property's value from `shell.css`, as written. */
function cssValue(name: string): string {
  const match = new RegExp(`${name}:\\s*([^;]+);`).exec(SHELL_CSS);
  expect(match, `${name} is not declared in shell.css`).not.toBeNull();
  return (match?.[1] ?? "").trim();
}

describe("the documented sizes", () => {
  it("covers every resizable zone", () => {
    /* If the table stops being parseable — a column added, the backticks
       dropped — every assertion below would pass over an empty map and prove
       nothing. */
    for (const zone of RESIZABLE_ZONES) {
      expect(
        documented.has(zone),
        `${zone} is missing from the reference`,
      ).toBe(true);
    }
    expect(documented.has("player")).toBe(true);
    expect(documented.has("appBar")).toBe(true);
  });

  it.each(RESIZABLE_ZONES)("%s matches the reference document", (zone) => {
    const row = documented.get(zone);
    expect(row).toBeDefined();
    const bounds = ZONE_BOUNDS[zone];

    expect(firstNumber(row?.defaultText ?? "") / 100).toBeCloseTo(
      bounds.defaultFraction,
      5,
    );
    expect(firstNumber(row?.minimumText ?? "")).toBe(bounds.minimumPx);
    expect(firstNumber(row?.maximumText ?? "") / 100).toBeCloseTo(
      bounds.maximumFraction,
      5,
    );
  });

  it("matches the reference for the player and the bar", () => {
    expect(firstNumber(documented.get("player")?.minimumText ?? "")).toBe(
      PLAYER_MINIMUM_PX,
    );
    expect(firstNumber(documented.get("appBar")?.defaultText ?? "")).toBe(
      APP_BAR_HEIGHT_PX,
    );
  });

  it("states the window minimum the zone minimums were derived from", () => {
    /* 1280 × 800 is not decoration: every minimum above was chosen so the
       player still has 480px at that width. A change to the window minimum that
       did not revisit the zones would make the layout unsatisfiable. */
    expect(REFERENCE).toContain(
      `${String(WINDOW_MINIMUM.width)} × ${String(WINDOW_MINIMUM.height)}`,
    );

    const splitterCount = 2;
    const consumed =
      ZONE_BOUNDS.library.minimumPx +
      ZONE_BOUNDS.inspector.minimumPx +
      PLAYER_MINIMUM_PX +
      splitterCount;
    expect(consumed).toBeLessThanOrEqual(WINDOW_MINIMUM.width);
  });
});

describe("the stylesheet", () => {
  it.each(RESIZABLE_ZONES)("clamps %s to the documented bounds", (zone) => {
    const bounds = ZONE_BOUNDS[zone];
    expect(cssValue(`--zone-${zone}-min`)).toBe(
      `${String(bounds.minimumPx)}px`,
    );
    expect(cssValue(`--zone-${zone}-max`)).toBe(
      `${String(bounds.maximumFraction * 100)}%`,
    );
  });

  it("gives the player its floor and the bar its height", () => {
    expect(cssValue("--zone-player-min")).toBe(
      `${String(PLAYER_MINIMUM_PX)}px`,
    );
    expect(cssValue("--shell-bar-height")).toBe(
      `${String(APP_BAR_HEIGHT_PX)}px`,
    );
  });

  it("seeds each zone at its documented default", () => {
    /* The fallback for a first launch or a cleared preference. If these drift
       from `layout.ts`, a fresh install and a reset layout would differ — and
       the one nobody tests is the fresh install. */
    for (const zone of RESIZABLE_ZONES) {
      expect(cssValue(`--layout-${zone}-fraction`)).toBe(
        String(ZONE_BOUNDS[zone].defaultFraction),
      );
    }
  });

  it("applies the clamp to both axes", () => {
    /* The JavaScript clamp protects the value being persisted; this one
       protects the layout when the window itself shrinks, which no pointer
       handler ever sees. Losing it is invisible until somebody resizes. */
    expect(SHELL_CSS).toMatch(/grid-template-columns:[\s\S]*?clamp\(/);
    expect(SHELL_CSS).toMatch(/grid-template-rows:[\s\S]*?clamp\(/);
  });
});

describe("clampFraction", () => {
  const width = 1280;

  it("stops a zone at its pixel minimum rather than collapsing it", () => {
    /* The acceptance criterion: "a splitter stops rather than collapsing a zone
       to nothing". A collapsed zone is a zone the user then has to work out how
       to get back. */
    for (const zone of RESIZABLE_ZONES) {
      const clamped = clampFraction(zone, 0, width);
      expect(clamped * width).toBeCloseTo(ZONE_BOUNDS[zone].minimumPx, 5);
    }
  });

  it("stops a zone at its maximum rather than eating the player", () => {
    for (const zone of RESIZABLE_ZONES) {
      expect(clampFraction(zone, 1, width)).toBe(
        ZONE_BOUNDS[zone].maximumFraction,
      );
    }
  });

  it("leaves a value inside the bounds alone", () => {
    expect(clampFraction("library", 0.3, width)).toBe(0.3);
  });

  it("returns the default for a value that is not a number", () => {
    // A corrupted preference must not produce a NaN grid track, which renders
    // as a zone of zero width and no error anywhere.
    expect(clampFraction("library", Number.NaN, width)).toBe(
      ZONE_BOUNDS.library.defaultFraction,
    );
  });

  it("returns the default when the container has not been measured yet", () => {
    // Before layout, `clientWidth` is 0. Dividing by it would produce Infinity
    // and a zone that fills the window.
    expect(clampFraction("library", 0.3, 0)).toBe(
      ZONE_BOUNDS.library.defaultFraction,
    );
  });

  it("keeps the player above its floor at the window minimum", () => {
    /* The arithmetic the whole set of minimums exists to satisfy, asserted at
       the worst case: both side panels dragged as wide as they go on the
       smallest window we support. */
    const library = clampFraction(
      "library",
      ZONE_BOUNDS.library.maximumFraction,
      WINDOW_MINIMUM.width,
    );
    const inspector = clampFraction(
      "inspector",
      ZONE_BOUNDS.inspector.maximumFraction,
      WINDOW_MINIMUM.width,
    );

    const remaining = WINDOW_MINIMUM.width * (1 - library - inspector) - 2;

    /* At the maximums the player is squeezed below its floor, which is why the
       stylesheet gives it `minmax(--zone-player-min, 1fr)`: the grid refuses to
       shrink it further and the side panels give the space back. This asserts
       the situation the CSS is there to handle, so that removing the `minmax`
       is not silent. */
    expect(remaining).toBeLessThan(PLAYER_MINIMUM_PX);
    expect(SHELL_CSS).toContain("minmax(var(--zone-player-min), 1fr)");
  });
});

describe("at the window minimum", () => {
  /*
   * Found by the shell's own tests rather than reasoned about in advance, and
   * left in because it is the layout behaving correctly at the edge of its
   * range rather than a defect.
   *
   * At 1280px the inspector's documented 20% default is 256px, which is under
   * its documented 260px minimum. So on the smallest supported window the
   * inspector opens at its floor rather than at its default, and gives the
   * extra four pixels back to the player — which is the right way round, since
   * the player is what the window is for.
   *
   * The alternative readings are both worse. Lowering the minimum would make
   * the panel too narrow for a labelled control with a numeric field; lowering
   * the default would make every other window size slightly wrong to fix the
   * one at the very bottom of the range.
   */
  it("clamps the inspector to its floor rather than its default", () => {
    const clamped = clampFraction(
      "inspector",
      ZONE_BOUNDS.inspector.defaultFraction,
      WINDOW_MINIMUM.width,
    );

    expect(clamped * WINDOW_MINIMUM.width).toBeCloseTo(
      ZONE_BOUNDS.inspector.minimumPx,
      5,
    );
    expect(clamped).toBeGreaterThan(ZONE_BOUNDS.inspector.defaultFraction);
  });

  it("opens the library at its default, which is above its floor", () => {
    // The library has room to spare at the minimum width, so it is not clamped
    // — which is what makes the inspector's case a property of that zone rather
    // than of the window.
    const clamped = clampFraction(
      "library",
      ZONE_BOUNDS.library.defaultFraction,
      WINDOW_MINIMUM.width,
    );
    expect(clamped).toBe(ZONE_BOUNDS.library.defaultFraction);
  });
});

/** Every zone the CSS knows about is a zone the contract knows about. */
describe("the two lists agree", () => {
  it("declares no stylesheet zone the contract has not heard of", () => {
    const inCss = [...SHELL_CSS.matchAll(/--zone-([a-z]+)-min\b/g)].map(
      (match) => match[1],
    );
    const known = new Set<string>([...RESIZABLE_ZONES, "player"]);

    for (const zone of inCss) {
      expect(known.has(zone ?? ""), `${String(zone)} is only in the CSS`).toBe(
        true,
      );
    }
  });

  it("names the same zones the shell renders", () => {
    const zones: ResizableZone[] = [...RESIZABLE_ZONES];
    expect(zones).toEqual(["library", "inspector", "timeline"]);
  });
});
