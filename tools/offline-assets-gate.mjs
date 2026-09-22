#!/usr/bin/env node
/**
 * The offline-assets gate.
 *
 * Issue #16: "the application renders correctly with no network connection on
 * first launch", and "font files bundled with the application, never fetched at
 * runtime".
 *
 * Both are one property: nothing the interface needs in order to appear may
 * come off a network. The failure is quiet in exactly the way that matters — a
 * `<link>` to a font service or an `url(https://…)` in a stylesheet works
 * perfectly on the machine of whoever added it, works in CI, works in every
 * demonstration, and then a user opens Blinkify on a train and the interface
 * has no typeface. `CLAUDE.md` section 20 rule 8 already forbids a network call
 * other than the update check; this is the half of that rule a linter can see.
 *
 * What counts as a violation: a remote URL in a position the browser would
 * fetch before or during first paint.
 *
 *   CSS       `url(https://…)`, `@import "https://…"`
 *   HTML/TSX  `src="https://…"`, `<link … href="https://…">`
 *
 * Protocol-relative `//host/path` counts too: it is a remote URL that merely
 * declines to name its scheme.
 *
 * What is deliberately NOT a violation: a URL in a comment, a URL in prose, or
 * an `<a href>` a user clicks. The first two are documentation — the font
 * provenance record is full of source URLs and must stay that way — and the
 * third is a navigation, not an asset. A gate that flagged them would be routed
 * around within a month, and a gate people route around protects nothing.
 *
 * Usage:
 *   node tools/offline-assets-gate.mjs [root]
 *
 * `root` defaults to the repository. The injection test passes a temporary tree
 * so that proving the gate fires never touches real source.
 *
 * Exit code 0 when clean, 1 when a remote asset reference is found.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = process.argv[2] ?? join(HERE, "..");

/** Directories holding source a person wrote. Nothing else is scanned. */
const SCAN_ROOTS = ["apps", "packages"];

const SCANNED_EXTENSIONS = [".css", ".html", ".ts", ".tsx"];

const SKIPPED_DIRECTORIES = new Set([
  "node_modules",
  "dist",
  "target",
  ".turbo",
  "gen",
]);

/** `https:`, `http:` or a protocol-relative `//host`. */
const REMOTE = String.raw`(?:https?:)?//`;

/**
 * @typedef {{ file: string, line: number, found: string, rule: string }} Violation
 */

/**
 * @typedef {object} Rule
 * @property {string} name
 * @property {RegExp} pattern    must carry the `g` flag
 * @property {(path: string) => boolean} applies
 */

/** @type {Rule[]} */
const RULES = [
  {
    name: "remote-asset-in-stylesheet",
    // url(https://…), url("https://…"), url( '//host/x' )
    pattern: new RegExp(String.raw`url\(\s*['"]?${REMOTE}[^)'"]*`, "gi"),
    applies: (path) => path.endsWith(".css"),
  },
  {
    name: "remote-stylesheet-import",
    pattern: new RegExp(
      String.raw`@import\s+(?:url\(\s*)?['"]${REMOTE}[^'"]*`,
      "gi",
    ),
    applies: (path) => path.endsWith(".css"),
  },
  {
    name: "remote-asset-in-markup",
    // src="https://…" on any element: script, img, video, iframe, audio.
    pattern: new RegExp(String.raw`\bsrc\s*=\s*['"]${REMOTE}[^'"]*`, "gi"),
    applies: (path) => !path.endsWith(".css"),
  },
  {
    name: "remote-link-element",
    // <link … href="https://…">. The font-service regression, exactly.
    pattern: new RegExp(
      String.raw`<link\b[^>]*?href\s*=\s*['"]${REMOTE}[^'"]*`,
      "gis",
    ),
    applies: (path) => !path.endsWith(".css"),
  },
];

/**
 * Strip comments, so a documented source URL is not read as a fetch.
 *
 * `packages/ui/src/fonts/README.md` is not scanned, but `fonts.css` explains
 * where its files came from and `colour-tokens.md` links out to WCAG. Prose
 * about a URL is not a request for it.
 */
function stripComments(source, path) {
  if (path.endsWith(".css")) return source.replace(/\/\*[\s\S]*?\*\//g, " ");
  if (path.endsWith(".html")) return source.replace(/<!--[\s\S]*?-->/g, " ");

  /* The line-comment case has a trap in it, and the injection test found it.
     A stripper that removes every `//` to end of line also removes the `//` of
     `src="//cdn.example.com/x.png"`, and of `src="https://…"` — so the gate
     scanned a file with every URL already deleted and reported it clean.
     Requiring the `//` not to follow a colon, a quote or a backslash keeps a
     real comment strippable and leaves a URL inside a string alone. */
  return source
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/(^|[^:"'`\\])\/\/[^\n]*/gm, "$1 ");
}

/** @returns {Violation[]} */
function scanFile(absolutePath, relativePath) {
  const source = stripComments(
    readFileSync(absolutePath, "utf8"),
    relativePath,
  );

  /** @type {Violation[]} */
  const violations = [];

  for (const rule of RULES) {
    if (!rule.applies(relativePath)) continue;

    for (const match of source.matchAll(rule.pattern)) {
      const line = source.slice(0, match.index).split("\n").length;
      violations.push({
        file: relativePath,
        line,
        found: `${match[0].slice(0, 70).replace(/\s+/g, " ")}…`,
        rule: rule.name,
      });
    }
  }

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
      const relativePath = relative(ROOT, file);
      scanned += 1;
      violations.push(...scanFile(file, relativePath));
    }
  }

  if (violations.length > 0) {
    console.error(
      `offline-assets-gate: ${String(violations.length)} remote asset reference(s)\n`,
    );
    for (const violation of violations) {
      console.error(
        `  ${violation.file.split(sep).join("/")}:${String(violation.line)}  ${violation.found}  [${violation.rule}]`,
      );
    }
    console.error(
      "\nBlinkify must render on a machine with no network. Commit the asset and" +
        "\nreference it by relative path — see packages/ui/src/fonts/README.md for how" +
        "\nthe typefaces do it. CLAUDE.md section 20 rule 8: fix the cause, do not" +
        "\ndisable the gate.",
    );
    process.exit(1);
  }

  console.log(
    `offline-assets-gate: ${String(scanned)} file(s) scanned, no remote asset reference.`,
  );
}

main();
