#!/usr/bin/env node
/**
 * Injection test for the colour gate.
 *
 * `pnpm colours:check` passing today proves there is no hard-coded colour in
 * the tree today. It does not prove the gate would notice one — and a gate that
 * cannot notice is worse than no gate, because it is believed. `CLAUDE.md`
 * section 14: a rule you have not seen fail is a rule you have not tested.
 *
 * Each case builds a throwaway tree and points `tools/colour-gate.mjs` at it
 * through its optional root argument. The repository is never edited, so an
 * interrupted run cannot leave a violation behind in real source.
 *
 * The cases that earn their place are the last four. The first three would be
 * caught by any regex; the rest are the ways a colour actually arrives:
 * spelled as a word, wrapped in a function, or sitting in a file the gate has
 * to decide whether to read at all.
 *
 * Run: pnpm colours:check:test
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const GATE = join(HERE, "colour-gate.mjs");

/**
 * @typedef {object} Case
 * @property {string} name        what the case is proving
 * @property {string} path        file to write, relative to the tree root
 * @property {string} contents    what to write into it
 * @property {boolean} mustFail   whether the gate is required to reject it
 * @property {string} [rule]      the rule expected to name the violation
 */

/** @type {Case[]} */
const CASES = [
  {
    name: "a hex colour in a stylesheet",
    path: "apps/desktop/src/panel.css",
    contents: ".panel {\n  background: #1e1e1e;\n}\n",
    mustFail: true,
    rule: "hex-colour-outside-token-file",
  },
  {
    name: "a hex colour in a component",
    path: "apps/desktop/src/Panel.tsx",
    contents: "export const tint = { color: '#ff0000' };\n",
    mustFail: true,
    rule: "hex-colour-outside-token-file",
  },
  {
    name: "a three-digit hex colour",
    path: "apps/desktop/src/panel.css",
    contents: ".panel {\n  color: #fff;\n}\n",
    mustFail: true,
    rule: "hex-colour-outside-token-file",
  },
  {
    // The one a hex-only check walks straight past.
    name: "an rgb() function with no hex in it",
    path: "apps/desktop/src/panel.css",
    contents: ".panel {\n  background: rgb(30 30 30);\n}\n",
    mustFail: true,
    rule: "colour-function-outside-token-file",
  },
  {
    // And the one a function-and-hex check walks past.
    name: "a CSS named colour",
    path: "apps/desktop/src/panel.css",
    contents: ".panel {\n  border-color: rebeccapurple;\n}\n",
    mustFail: true,
    rule: "named-colour-outside-token-file",
  },
  {
    // The gate strips `var()` before looking for names. Without that step the
    // token set fails its own gate, since half of it references `--brand-azure`
    // and `--brand-violet`.
    name: "a var() reference whose token name contains a colour word",
    path: "apps/desktop/src/panel.css",
    contents:
      ".panel {\n  color: var(--brand-violet);\n  border: 1px solid var(--border);\n}\n",
    mustFail: false,
  },
  {
    // Comments explain tokens by naming their values. If the gate read them,
    // every honest comment in tokens.css would become a violation elsewhere.
    name: "a hex colour inside a comment",
    path: "apps/desktop/src/panel.css",
    contents:
      "/* azure is #22a3fd */\n.panel {\n  color: var(--text-accent);\n}\n",
    mustFail: false,
  },
  {
    // The exemption has to hold, or the test suite for the colour machinery
    // cannot be written.
    name: "a hex colour in a test file",
    path: "packages/ui/src/tokens/contrast.test.ts",
    contents: "expect(contrastRatio('#ffffff', '#000000')).toBe(21);\n",
    mustFail: false,
  },
  {
    // And the token file itself, which is the whole point of the exemption.
    name: "the token file, which is where colours live",
    path: "packages/ui/src/tokens/tokens.css",
    contents: ":root {\n  --brand-azure: #22a3fd;\n  --surface: #12161c;\n}\n",
    mustFail: false,
  },
];

function runGate(root) {
  return spawnSync(process.execPath, [GATE, root], { encoding: "utf8" });
}

let failures = 0;

for (const testCase of CASES) {
  const tree = mkdtempSync(join(tmpdir(), "blinkify-colour-gate-"));

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
    `\ncolour-gate-test: ${String(failures)} case(s) failed. The colour gate does not\n` +
      "prove what it claims, so a hard-coded colour can reach main unseen.",
  );
  process.exit(1);
}

console.log(
  `\ncolour-gate-test: ${String(CASES.length)} case(s), all as expected.`,
);
