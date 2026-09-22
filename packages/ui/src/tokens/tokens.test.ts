import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  contrastRatio,
  formatRatio,
  parseTokens,
  resolveToken,
} from "./contrast.js";
import {
  CONTRAST_PAIRS,
  DUTY_THRESHOLD,
  type ContrastPair,
} from "./contrastPairs.js";

/**
 * The token set, asserted rather than trusted (#15).
 *
 * Three properties are under test, and each of them fails silently without it:
 *
 *  1. The tokens the issue requires exist and are spelled the way components
 *     will spell them.
 *  2. The two layers stay separate — semantic tokens reference primitives and
 *     never carry a value of their own, which is the whole mechanism by which a
 *     light theme becomes a value swap rather than a rewrite.
 *  3. Every documented pair still measures what the documentation says, and
 *     still clears the threshold for the job it does.
 *
 * Property 3 is the one that earns the file. A dark interface that has drifted
 * below AA looks exactly like one that has not.
 */

/* The directory is derived in two steps rather than with
   `new URL("./tokens.css", import.meta.url)`, because Vite rewrites that exact
   pattern into an asset reference at transform time and the test then reads an
   http URL that no filesystem call can open. */
const HERE = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = join(HERE, "..", "..", "..", "..");

const TOKENS_CSS = readFileSync(join(HERE, "tokens.css"), "utf8");
const CONTRAST_DOC = readFileSync(
  join(REPOSITORY_ROOT, "docs", "design", "colour-tokens.md"),
  "utf8",
);

const tokens = parseTokens(TOKENS_CSS);

/** Semantic tokens: everything a component is allowed to name. */
const SEMANTIC_TOKENS = [
  "--background",
  "--surface-sunken",
  "--surface",
  "--surface-raised",
  "--surface-overlay",
  "--surface-hover",
  "--surface-active",
  "--border",
  "--border-strong",
  "--text-primary",
  "--text-secondary",
  "--text-disabled",
  "--text-accent",
  "--accent",
  "--accent-hover",
  "--accent-pressed",
  "--accent-foreground",
  "--focus-ring",
  "--scrim",
  "--success",
  "--warning",
  "--danger",
  "--lossless",
  "--timeline-track",
  "--timeline-clip-video",
  "--timeline-clip-video-border",
  "--timeline-clip-audio",
  "--timeline-clip-audio-border",
  "--timeline-clip-label",
  "--timeline-clip-selected-border",
  "--timeline-playhead",
  "--timeline-ruler-text",
] as const;

/** One row of the table in `docs/design/colour-tokens.md`. */
interface DocumentedRow {
  readonly foreground: string;
  readonly background: string;
  readonly ratio: string;
  readonly required: string;
}

/**
 * Read the contrast table out of the documentation.
 *
 * Cells are split and trimmed rather than matched as whole lines, because
 * Prettier pads the columns of a Markdown table and an exact-text assertion
 * would fail on formatting instead of on a wrong number.
 */
function documentedRows(markdown: string): DocumentedRow[] {
  const rows: DocumentedRow[] = [];

  for (const line of markdown.split("\n")) {
    if (!line.trimStart().startsWith("| `--")) continue;

    const cells = line
      .split("|")
      .slice(1, -1)
      .map((cell) => cell.trim().replace(/`/g, ""));

    const [foreground, background, , , ratio, required] = cells;
    if (
      foreground === undefined ||
      background === undefined ||
      ratio === undefined ||
      required === undefined
    ) {
      continue;
    }

    rows.push({ foreground, background, ratio, required });
  }

  return rows;
}

function measure(pair: ContrastPair): number {
  return contrastRatio(
    resolveToken(tokens, pair.foreground),
    resolveToken(tokens, pair.background),
  );
}

describe("the token file declares what #15 requires", () => {
  it("carries the three brand values measured from the logo", () => {
    expect(tokens.get("--brand-azure")).toBe("#22a3fd");
    expect(tokens.get("--brand-indigo")).toBe("#4e52fc");
    expect(tokens.get("--brand-violet")).toBe("#8b46fb");
  });

  it("carries the brand gradient at the logo's angle", () => {
    const gradient = tokens.get("--brand-gradient");
    expect(gradient).toContain("135deg");
    expect(gradient).toContain("#22a3fd 0%");
    expect(gradient).toContain("#4e52fc 50%");
    expect(gradient).toContain("#8b46fb 100%");
  });

  it("carries a neutral surface ramp of at least six steps", () => {
    // The issue's floor is six, from the application background to the highest
    // raised surface. Steps 1 to 5 are surfaces; the assertion covers the whole
    // ramp because a missing step higher up breaks borders and text instead.
    for (let step = 1; step <= 13; step += 1) {
      expect(tokens.has(`--neutral-${String(step)}`)).toBe(true);
    }
  });

  it("carries every semantic role a component may name", () => {
    for (const token of SEMANTIC_TOKENS) {
      expect(tokens.has(token)).toBe(true);
    }
  });

  it("keeps the focus ring off the control it focuses", () => {
    // Load-bearing, not cosmetic. Azure on the accent fill measures 1.96:1;
    // the ring only clears §1.4.11 because the offset puts it on the surface
    // behind the control. A zero offset silently reintroduces the failure that
    // `contrastPairs.ts` records as fixed by geometry.
    const offset = tokens.get("--focus-ring-offset");
    expect(offset).toBeDefined();
    expect(Number.parseFloat(offset ?? "0")).toBeGreaterThan(0);
  });

  it("gives the lossless indicator a colour of its own", () => {
    // Not a second green. Success and lossless answer different questions, and
    // an export that finished is not the same claim as an export that changed
    // no pixels — CLAUDE.md section 17.
    expect(resolveToken(tokens, "--lossless")).not.toBe(
      resolveToken(tokens, "--success"),
    );
  });
});

describe("the two layers stay separate", () => {
  it("defines every semantic token as a reference, never as a value", () => {
    // This is the mechanism behind "swapping the token file changes the whole
    // application with no component edits". A semantic token holding a literal
    // is a value that a light theme would have to chase into layer 2.
    for (const token of SEMANTIC_TOKENS) {
      expect({ token, value: tokens.get(token) }).toEqual({
        token,
        value: expect.stringMatching(/^var\(--[\w-]+\)$/) as unknown,
      });
    }
  });

  it("resolves every semantic token to a literal colour", () => {
    for (const token of SEMANTIC_TOKENS) {
      expect(() => resolveToken(tokens, token)).not.toThrow();
    }
  });
});

describe("every documented pair meets its threshold", () => {
  it.each(CONTRAST_PAIRS)(
    "$foreground on $background — $usage",
    (pair: ContrastPair) => {
      const ratio = measure(pair);
      const threshold = DUTY_THRESHOLD[pair.duty];

      // Reported as an object so a failure names the pair rather than printing
      // "expected 4.31 to be greater than 4.5" with nothing to act on.
      expect({
        pair: `${pair.foreground} on ${pair.background}`,
        ratio: formatRatio(ratio),
        meets: ratio >= threshold,
      }).toEqual({
        pair: `${pair.foreground} on ${pair.background}`,
        ratio: formatRatio(ratio),
        meets: true,
      });
    },
  );
});

describe("the documented contrast table matches the tokens", () => {
  const rows = documentedRows(CONTRAST_DOC);

  it("documents exactly the pairs that carry a duty", () => {
    const documented = rows
      .map((row) => `${row.foreground} on ${row.background}`)
      .sort();
    const required = CONTRAST_PAIRS.map(
      (pair) => `${pair.foreground} on ${pair.background}`,
    ).sort();

    expect(documented).toEqual(required);
  });

  it.each(CONTRAST_PAIRS)(
    "states the measured ratio for $foreground on $background",
    (pair: ContrastPair) => {
      const row = rows.find(
        (candidate) =>
          candidate.foreground === pair.foreground &&
          candidate.background === pair.background,
      );

      expect(row).toBeDefined();
      expect(row?.ratio).toBe(`${formatRatio(measure(pair))}:1`);
      expect(row?.required).toBe(`${DUTY_THRESHOLD[pair.duty].toFixed(1)}:1`);
    },
  );
});
