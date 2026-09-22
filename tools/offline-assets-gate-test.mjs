#!/usr/bin/env node
/**
 * Injection test for the offline-assets gate.
 *
 * `pnpm offline:check` passing today proves the tree holds no remote asset
 * reference today. It does not prove the gate would notice one, and a gate that
 * cannot notice is worse than no gate because it is believed. `CLAUDE.md`
 * section 14: a rule you have not seen fail is a rule you have not tested, and
 * a new boundary rule arrives with its injection case in the same pull request.
 *
 * Each case builds a throwaway tree and points the gate at it through its
 * optional root argument, so the repository is never edited.
 *
 * The cases that earn their place are the acceptances. Rejecting
 * `<link href="https://fonts.googleapis.com/…">` is the case everyone thinks
 * of; accepting a source URL written in a comment is the case that decides
 * whether anyone can keep using the gate, because the font provenance record
 * exists to name where those bytes came from.
 *
 * Run: pnpm offline:check:test
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const GATE = join(HERE, "offline-assets-gate.mjs");

/**
 * @typedef {object} Case
 * @property {string} name
 * @property {string} path
 * @property {string} contents
 * @property {boolean} mustFail
 * @property {string} [rule]
 */

/** @type {Case[]} */
const CASES = [
  {
    // The regression this gate exists for, in the form it actually arrives.
    name: "a font service stylesheet in the document head",
    path: "apps/desktop/index.html",
    contents:
      '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter" />\n',
    mustFail: true,
    rule: "remote-link-element",
  },
  {
    name: "a remote font file in an @font-face",
    path: "packages/ui/src/fonts/fonts.css",
    contents:
      '@font-face {\n  font-family: "Inter";\n  src: url("https://cdn.example.com/inter.woff2");\n}\n',
    mustFail: true,
    rule: "remote-asset-in-stylesheet",
  },
  {
    name: "a remote stylesheet import",
    path: "apps/desktop/src/styles.css",
    contents: '@import "https://cdn.example.com/reset.css";\n',
    mustFail: true,
    rule: "remote-stylesheet-import",
  },
  {
    name: "a remote script in markup",
    path: "apps/desktop/index.html",
    contents: '<script src="https://cdn.example.com/analytics.js"></script>\n',
    mustFail: true,
    rule: "remote-asset-in-markup",
  },
  {
    // Protocol-relative is a remote URL that declines to name its scheme, and
    // it is the form a copied snippet most often carries.
    name: "a protocol-relative asset",
    path: "apps/desktop/src/Panel.tsx",
    contents: 'export const logo = <img src="//cdn.example.com/logo.png" />;\n',
    mustFail: true,
    rule: "remote-asset-in-markup",
  },
  {
    // The acceptance that keeps the gate usable. fonts.css documents where its
    // files came from; without this the provenance record could not be written.
    name: "a source URL inside a comment",
    path: "packages/ui/src/fonts/fonts.css",
    contents:
      "/* Downloaded from https://cdn.jsdelivr.net/npm/@fontsource/inter/x.woff2 */\n" +
      '@font-face {\n  src: url("./inter.woff2");\n}\n',
    mustFail: false,
  },
  {
    name: "a source URL in a TypeScript comment",
    path: "apps/desktop/src/update.store.ts",
    // See https://example.com/spec — a line comment, not a fetch.
    contents:
      "// Specification: https://example.com/spec\nexport const ready = true;\n",
    mustFail: false,
  },
  {
    /* The case that caught a real hole in this gate. A naive line-comment
       stripper removes the `//` of an https URL inside a string too, and the
       gate then scans a file with every URL already gone and calls it clean.
       This case fails against that stripper and passes against the real one. */
    name: "a remote script tag inside a component",
    path: "apps/desktop/src/Panel.tsx",
    contents:
      'export const beacon = <script src="https://cdn.example.com/a.js" />;\n',
    mustFail: true,
    rule: "remote-asset-in-markup",
  },
  {
    // A link a user clicks is a navigation, not an asset. Flagging it would
    // make the gate something people route around.
    name: "an anchor a user clicks",
    path: "apps/desktop/src/Panel.tsx",
    contents:
      'export const notes = <a href="https://example.com/releases">Release notes</a>;\n',
    mustFail: false,
  },
  {
    // The shape everything in Blinkify is supposed to have.
    name: "a bundled asset by relative path",
    path: "packages/ui/src/fonts/fonts.css",
    contents:
      '@font-face {\n  src: url("./inter-latin-wght-normal.woff2");\n}\n',
    mustFail: false,
  },
];

function runGate(root) {
  return spawnSync(process.execPath, [GATE, root], { encoding: "utf8" });
}

let failures = 0;

for (const testCase of CASES) {
  const tree = mkdtempSync(join(tmpdir(), "blinkify-offline-gate-"));

  try {
    const target = join(tree, testCase.path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, testCase.contents);

    const result = runGate(tree);
    const rejected = result.status !== 0;
    const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;

    if (rejected !== testCase.mustFail) {
      failures += 1;
      console.error(
        `FAIL  ${testCase.name}\n` +
          `      expected the gate to ${testCase.mustFail ? "reject" : "accept"} it, ` +
          `and it ${rejected ? "rejected" : "accepted"} it.\n` +
          `      ${output.trim().split("\n").join("\n      ")}`,
      );
      continue;
    }

    if (testCase.rule !== undefined && !output.includes(testCase.rule)) {
      failures += 1;
      console.error(
        `FAIL  ${testCase.name}\n` +
          `      rejected, but not by ${testCase.rule}. A gate that fires for the\n` +
          `      wrong reason will stop firing when that reason is removed.\n` +
          `      ${output.trim().split("\n").join("\n      ")}`,
      );
      continue;
    }

    console.log(
      `ok    ${testCase.name} — ${testCase.mustFail ? "rejected" : "accepted"}`,
    );
  } finally {
    rmSync(tree, { recursive: true, force: true });
  }
}

if (failures > 0) {
  console.error(
    `\noffline-assets-gate-test: ${String(failures)} case(s) failed. The gate does not\n` +
      "prove what it claims, so a remote asset can reach main and Blinkify can ship\n" +
      "an interface that needs a network to appear.",
  );
  process.exit(1);
}

console.log(
  `\noffline-assets-gate-test: ${String(CASES.length)} case(s), all as expected.`,
);
