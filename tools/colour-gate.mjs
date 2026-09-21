#!/usr/bin/env node
/**
 * The colour gate.
 *
 * Issue #15: "a lint rule fails on a hard-coded colour outside the token file".
 *
 * One file in the repository may contain a colour value —
 * `packages/ui/src/tokens/tokens.css`. Everywhere else reads a semantic token
 * from it. The rule exists because the failure it prevents is invisible: a
 * component with `#1e1e1e` hard-coded looks perfectly fine until the day the
 * palette moves, and then it is the one panel that did not, in a dark interface
 * where nobody notices a surface being one step wrong.
 *
 * It also protects the contrast table. A colour that is not a token is a colour
 * nothing measured.
 *
 * What counts as a colour: a hex literal, a colour function (`rgb`, `hsl`,
 * `oklch`, `color-mix`, …) or — in CSS, where it is unambiguous — one of the
 * CSS named colours. `var(--token)` references are stripped before the check,
 * so `var(--brand-azure)` is not read as the named colour `azure`.
 *
 * Why not ESLint: two thirds of the colours in a React application live in
 * `.css` files, which ESLint does not read. A gate that covered only the
 * TypeScript half would report green over a stylesheet full of literals.
 *
 * Usage:
 *   node tools/colour-gate.mjs [root]
 *
 * `root` defaults to the repository. The injection test passes a temporary
 * tree so that proving the gate fires never touches real source.
 *
 * Exit code 0 when clean, 1 when a colour is found outside the token file.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = process.argv[2] ?? join(HERE, "..");

/** Directories that hold source a person wrote. Nothing else is scanned. */
const SCAN_ROOTS = ["apps", "packages"];

const SCANNED_EXTENSIONS = [".css", ".ts", ".tsx"];

const SKIPPED_DIRECTORIES = new Set([
  "node_modules",
  "dist",
  "target",
  ".turbo",
  "gen",
]);

/**
 * The one file permitted to hold colour values, and the test files that read
 * them back.
 *
 * Tests are exempt because the machinery that measures contrast has to be able
 * to name a colour in order to be tested at all — `contrastRatio("#ffffff",
 * "#000000")` is 21:1 and a test that could not write that is a test that
 * cannot check the maths. A test styles nothing, so the exemption cannot leak
 * into the interface.
 */
const TOKEN_FILE = join("packages", "ui", "src", "tokens", "tokens.css");
const isExempt = (relativePath) =>
  relativePath === TOKEN_FILE || /\.test\.tsx?$/.test(relativePath);

/* A hex colour: 3, 4, 6 or 8 digits. */
const HEX = /#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})\b/gi;

/* Any CSS colour function. `color-mix` included: mixing two tokens is fine,
   mixing in a literal is not, and the literal is caught by HEX anyway — but a
   bare `rgb(30 30 30)` would otherwise walk straight through. */
const COLOUR_FUNCTION =
  /\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color|color-mix)\s*\(/gi;

/* The CSS named colours. Checked in stylesheets only: in TypeScript, `tan`,
   `plum` and `linen` are ordinary words and flagging them would make the gate
   something people route around. */
const NAMED_COLOURS = new Set([
  "aliceblue",
  "antiquewhite",
  "aqua",
  "aquamarine",
  "azure",
  "beige",
  "bisque",
  "black",
  "blanchedalmond",
  "blue",
  "blueviolet",
  "brown",
  "burlywood",
  "cadetblue",
  "chartreuse",
  "chocolate",
  "coral",
  "cornflowerblue",
  "cornsilk",
  "crimson",
  "cyan",
  "darkblue",
  "darkcyan",
  "darkgoldenrod",
  "darkgray",
  "darkgreen",
  "darkgrey",
  "darkkhaki",
  "darkmagenta",
  "darkolivegreen",
  "darkorange",
  "darkorchid",
  "darkred",
  "darksalmon",
  "darkseagreen",
  "darkslateblue",
  "darkslategray",
  "darkslategrey",
  "darkturquoise",
  "darkviolet",
  "deeppink",
  "deepskyblue",
  "dimgray",
  "dimgrey",
  "dodgerblue",
  "firebrick",
  "floralwhite",
  "forestgreen",
  "fuchsia",
  "gainsboro",
  "ghostwhite",
  "gold",
  "goldenrod",
  "gray",
  "green",
  "greenyellow",
  "grey",
  "honeydew",
  "hotpink",
  "indianred",
  "indigo",
  "ivory",
  "khaki",
  "lavender",
  "lavenderblush",
  "lawngreen",
  "lemonchiffon",
  "lightblue",
  "lightcoral",
  "lightcyan",
  "lightgoldenrodyellow",
  "lightgray",
  "lightgreen",
  "lightgrey",
  "lightpink",
  "lightsalmon",
  "lightseagreen",
  "lightskyblue",
  "lightslategray",
  "lightslategrey",
  "lightsteelblue",
  "lightyellow",
  "lime",
  "limegreen",
  "linen",
  "magenta",
  "maroon",
  "mediumaquamarine",
  "mediumblue",
  "mediumorchid",
  "mediumpurple",
  "mediumseagreen",
  "mediumslateblue",
  "mediumspringgreen",
  "mediumturquoise",
  "mediumvioletred",
  "midnightblue",
  "mintcream",
  "mistyrose",
  "moccasin",
  "navajowhite",
  "navy",
  "oldlace",
  "olive",
  "olivedrab",
  "orange",
  "orangered",
  "orchid",
  "palegoldenrod",
  "palegreen",
  "paleturquoise",
  "palevioletred",
  "papayawhip",
  "peachpuff",
  "peru",
  "pink",
  "plum",
  "powderblue",
  "purple",
  "rebeccapurple",
  "red",
  "rosybrown",
  "royalblue",
  "saddlebrown",
  "salmon",
  "sandybrown",
  "seagreen",
  "seashell",
  "sienna",
  "silver",
  "skyblue",
  "slateblue",
  "slategray",
  "slategrey",
  "snow",
  "springgreen",
  "steelblue",
  "tan",
  "teal",
  "thistle",
  "tomato",
  "turquoise",
  "violet",
  "wheat",
  "white",
  "whitesmoke",
  "yellow",
  "yellowgreen",
]);

/** Strip comments, so an explanation of a token is not read as a violation. */
function stripComments(source, isCss) {
  const withoutBlocks = source.replace(/\/\*[\s\S]*?\*\//g, " ");
  return isCss ? withoutBlocks : withoutBlocks.replace(/\/\/[^\n]*/g, " ");
}

/** Replace `var(--token)` and `var(--token, fallback)` with whitespace. */
function stripVarReferences(source) {
  return source.replace(/var\(\s*--[\w-]+\s*(?:,[^()]*)?\)/g, " ");
}

/**
 * @typedef {{ file: string, line: number, found: string, rule: string }} Violation
 */

/** @returns {Violation[]} */
function scanFile(absolutePath, relativePath) {
  const isCss = relativePath.endsWith(".css");
  const raw = readFileSync(absolutePath, "utf8");
  const source = stripVarReferences(stripComments(raw, isCss));

  /** @type {Violation[]} */
  const violations = [];
  const lines = source.split("\n");

  lines.forEach((line, index) => {
    const lineNumber = index + 1;

    for (const match of line.matchAll(HEX)) {
      violations.push({
        file: relativePath,
        line: lineNumber,
        found: match[0],
        rule: "hex-colour-outside-token-file",
      });
    }

    for (const match of line.matchAll(COLOUR_FUNCTION)) {
      violations.push({
        file: relativePath,
        line: lineNumber,
        found: `${match[0]}…)`,
        rule: "colour-function-outside-token-file",
      });
    }

    if (!isCss) return;

    // Named colours, in the value half of a declaration only. A property named
    // `--brand-violet` is a token name, not a use of `violet`.
    const separator = line.indexOf(":");
    if (separator === -1) return;
    const value = line.slice(separator + 1);

    for (const word of value.toLowerCase().match(/[a-z]+/g) ?? []) {
      if (!NAMED_COLOURS.has(word)) continue;
      violations.push({
        file: relativePath,
        line: lineNumber,
        found: word,
        rule: "named-colour-outside-token-file",
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
  /** @type {Violation[]} */
  const violations = [];
  let scanned = 0;

  for (const scanRoot of SCAN_ROOTS) {
    const absolute = join(ROOT, scanRoot);
    let exists = true;
    try {
      statSync(absolute);
    } catch {
      exists = false;
    }
    if (!exists) continue;

    for (const file of collectFiles(absolute)) {
      // Normalised to forward slashes so the message reads the same on Windows
      // and in a Linux container, and so the exemption compares equal on both.
      const relativePath = relative(ROOT, file);
      if (isExempt(relativePath)) continue;
      scanned += 1;
      violations.push(...scanFile(file, relativePath));
    }
  }

  if (violations.length > 0) {
    console.error(
      `colour-gate: ${String(violations.length)} colour value(s) outside ${TOKEN_FILE.split(sep).join("/")}\n`,
    );
    for (const violation of violations) {
      console.error(
        `  ${violation.file.split(sep).join("/")}:${String(violation.line)}  ${violation.found}  [${violation.rule}]`,
      );
    }
    console.error(
      "\nAdd a semantic token to packages/ui/src/tokens/tokens.css and read it" +
        "\ninstead. See docs/design/colour-tokens.md. CLAUDE.md section 20 rule 10:" +
        "\nfix the cause, do not disable the gate.",
    );
    process.exit(1);
  }

  console.log(
    `colour-gate: ${String(scanned)} file(s) scanned, no colour outside the token file.`,
  );
}

main();
