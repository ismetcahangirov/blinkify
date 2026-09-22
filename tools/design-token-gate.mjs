#!/usr/bin/env node
/**
 * The design-token gate.
 *
 * Issue #17: "a lint rule fails the build on a hard-coded value inside
 * `packages/ui`". Epic #2: "no component contains a hard-coded colour, radius,
 * duration or spacing value".
 *
 * Colour already has a gate of its own — `tools/colour-gate.mjs`. This one
 * covers everything else a component can measure with: lengths, durations and
 * font weights. The failure it prevents is the same shape and just as quiet. A
 * component with `padding: 10px` looks perfectly fine, sits two pixels off the
 * grid every other control is on, and nobody sees it until the day somebody
 * screenshots two panels side by side.
 *
 * ── Where it applies ────────────────────────────────────────────────────────
 *
 * `packages/ui/src`, minus the files whose job is to hold values:
 *
 *   tokens/*.css     the three token layers. This is where numbers live.
 *   fonts/fonts.css  `@font-face`, whose `unicode-range` is not a measurement.
 *   *.test.ts(x)     a test has to be able to write a number to assert on one.
 *
 * Story files are NOT exempt. They are reviewed alongside the components and a
 * story laid out with `gap: 12px` is a story showing the component in a
 * spacing the product does not have.
 *
 * Everything else — `components/*.css`, `components/*.tsx`, `base.css` — reads
 * `var(--token)`.
 *
 * ── What counts as a value ──────────────────────────────────────────────────
 *
 * A number carrying a length or time unit: `12px`, `0.5rem`, `150ms`, `2s`.
 * And a bare numeric `font-weight`, because `font-weight: 600` is a scale step
 * spelled as a literal.
 *
 * ── What does not, and why each one is safe ────────────────────────────────
 *
 *   0                 zero is zero in every unit; there is nothing to tokenise.
 *   percentages       `width: 100%`, `translate(-50%, -50%)`. These are
 *                     relationships to a parent, not sizes — a token would make
 *                     them less clear, not more.
 *   angles            `rotate(360deg)`. One turn is one turn.
 *   unitless numbers  `flex: 1`, `opacity`, `z-index`, `scale(…)`. Not lengths.
 *   `var(…)`, `calc(…)` over tokens — the whole point.
 *
 * Usage:
 *   node tools/design-token-gate.mjs [root]
 *
 * `root` defaults to the repository. The injection test passes a temporary tree
 * so that proving the gate fires never touches real source.
 *
 * Exit code 0 when clean, 1 when a literal is found.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = process.argv[2] ?? join(HERE, "..");

/** The design system, and nothing else. The renderer is Tailwind's problem. */
const SCAN_ROOT = join("packages", "ui", "src");

const SCANNED_EXTENSIONS = [".css", ".ts", ".tsx"];

const SKIPPED_DIRECTORIES = new Set(["node_modules", "dist", ".turbo"]);

/** Files whose job is to hold values. */
function isExempt(relativePath) {
  const path = relativePath.split(sep).join("/");
  return (
    path.includes("/src/tokens/") ||
    path.endsWith("/fonts/fonts.css") ||
    /\.test\.tsx?$/.test(path)
  );
}

/**
 * A number with a length or time unit.
 *
 * `%` and `deg` are deliberately absent — see the header. The negative lookahead
 * on a preceding word character keeps `translate3d` and `grid-column: 1 / 3`
 * out of it, and the one on a following letter keeps a unit from matching the
 * front of a longer word.
 */
const DIMENSION =
  /(?<![\w#-])(-?\d*\.?\d+)(px|rem|em|ch|ex|vh|vw|vmin|vmax|pt|pc|in|cm|mm|ms|s)(?![\w-])/gi;

/** `font-weight: 600`, in CSS. The scale has a token for every one of these. */
const NUMERIC_FONT_WEIGHT = /font-weight\s*:\s*\d+/gi;

/**
 * Zero needs no unit and no token.
 *
 * Parsed rather than pattern-matched. The injection test caught the pattern
 * version accepting `0.375rem`, because a regex anchored at the front is
 * perfectly happy to match the leading zero of a number that is not zero —
 * which is a gate reporting clean over the exact literal it exists to catch.
 */
const isZero = (number) => Number.parseFloat(number) === 0;

function stripComments(source, isCss) {
  const withoutBlocks = source.replace(/\/\*[\s\S]*?\*\//g, " ");
  return isCss
    ? withoutBlocks
    : withoutBlocks.replace(/(^|[^:"'`\\])\/\/[^\n]*/gm, "$1 ");
}

/** Remove token references and the `calc()` expressions built out of them. */
function stripTokenReferences(source) {
  let previous;
  let current = source;
  /* Repeated, because a `calc()` can hold a `var()` that holds a fallback that
     holds another `var()`. One pass leaves the inner one behind and the gate
     then reports a violation inside a perfectly correct expression. */
  do {
    previous = current;
    current = current.replace(/var\(\s*--[\w-]+\s*(?:,[^()]*)?\)/g, " ");
  } while (current !== previous);
  return current;
}

/**
 * @typedef {{ file: string, line: number, found: string, rule: string }} Violation
 */

/** @returns {Violation[]} */
function scanFile(absolutePath, relativePath) {
  const isCss = relativePath.endsWith(".css");
  const source = stripTokenReferences(
    stripComments(readFileSync(absolutePath, "utf8"), isCss),
  );

  /** @type {Violation[]} */
  const violations = [];

  source.split("\n").forEach((line, index) => {
    const lineNumber = index + 1;

    for (const match of line.matchAll(DIMENSION)) {
      if (isZero(match[1] ?? "")) continue;
      violations.push({
        file: relativePath,
        line: lineNumber,
        found: match[0],
        rule: "dimension-outside-token-file",
      });
    }

    if (!isCss) return;

    for (const match of line.matchAll(NUMERIC_FONT_WEIGHT)) {
      violations.push({
        file: relativePath,
        line: lineNumber,
        found: match[0],
        rule: "font-weight-outside-token-file",
      });
    }
  });

  return violations;
}

/** @returns {string[]} absolute paths */
function collectFiles(directory) {
  /** @type {string[]} */
  const found = [];

  for (const entry of readdirSync(directory)) {
    const absolute = join(directory, entry);

    if (statSync(absolute).isDirectory()) {
      if (SKIPPED_DIRECTORIES.has(entry)) continue;
      found.push(...collectFiles(absolute));
      continue;
    }

    if (SCANNED_EXTENSIONS.some((extension) => entry.endsWith(extension))) {
      found.push(absolute);
    }
  }

  return found;
}

function main() {
  const absoluteRoot = join(ROOT, SCAN_ROOT);

  let exists = true;
  try {
    statSync(absoluteRoot);
  } catch {
    exists = false;
  }
  if (!exists) {
    console.log(
      "design-token-gate: packages/ui/src is absent, nothing to scan.",
    );
    return;
  }

  /** @type {Violation[]} */
  const violations = [];
  let scanned = 0;

  for (const file of collectFiles(absoluteRoot)) {
    const relativePath = relative(ROOT, file);
    if (isExempt(relativePath)) continue;
    scanned += 1;
    violations.push(...scanFile(file, relativePath));
  }

  if (violations.length > 0) {
    console.error(
      `design-token-gate: ${String(violations.length)} hard-coded value(s) in packages/ui\n`,
    );
    for (const violation of violations) {
      console.error(
        `  ${violation.file.split(sep).join("/")}:${String(violation.line)}  ${violation.found}  [${violation.rule}]`,
      );
    }
    console.error(
      "\nAdd a token to packages/ui/src/tokens/ and read it instead. The scales" +
        "\nare documented in docs/design/scales.md; a measurement that belongs to one" +
        "\ncomponent goes in tokens/component-tokens.css. CLAUDE.md section 20 rule 10:" +
        "\nfix the cause, do not disable the gate.",
    );
    process.exit(1);
  }

  console.log(
    `design-token-gate: ${String(scanned)} file(s) scanned, no hard-coded value.`,
  );
}

main();
