#!/usr/bin/env node
/**
 * Injection test for the design-token gate.
 *
 * `pnpm tokens:check` passing today proves the design system holds no hard-coded
 * value today. It does not prove the gate would notice one, and a gate that
 * cannot notice is worse than no gate because it is believed. `CLAUDE.md`
 * section 14: a rule you have not seen fail is a rule you have not tested, and
 * a new boundary rule arrives with its injection case in the same pull request.
 *
 * The rejections are the obvious half. The acceptances are what decide whether
 * the gate is usable at all: a gate that flags `width: 100%` or `flex: 1` is a
 * gate somebody turns off within a week, and then the padding drifts.
 *
 * Run: pnpm tokens:check:test
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const GATE = join(HERE, "design-token-gate.mjs");

const COMPONENT = "packages/ui/src/components/Panel.css";

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
    name: "a padding in pixels",
    path: COMPONENT,
    contents: ".panel {\n  padding: 10px;\n}\n",
    mustFail: true,
    rule: "dimension-outside-token-file",
  },
  {
    name: "a radius in rem",
    path: COMPONENT,
    contents: ".panel {\n  border-radius: 0.375rem;\n}\n",
    mustFail: true,
    rule: "dimension-outside-token-file",
  },
  {
    // The one a spacing-only check walks past, and the one that reads as lag.
    name: "a transition duration in milliseconds",
    path: COMPONENT,
    contents: ".panel {\n  transition: opacity 300ms ease;\n}\n",
    mustFail: true,
    rule: "dimension-outside-token-file",
  },
  {
    name: "a numeric font weight",
    path: COMPONENT,
    contents: ".panel {\n  font-weight: 600;\n}\n",
    mustFail: true,
    rule: "font-weight-outside-token-file",
  },
  {
    // Half of a React component's measurements are in its inline styles, and a
    // CSS-only gate would report green over a file full of them.
    name: "a length inside a component's inline style",
    path: "packages/ui/src/components/Panel.tsx",
    contents: 'export const panel = <div style={{ gap: "12px" }} />;\n',
    mustFail: true,
    rule: "dimension-outside-token-file",
  },
  {
    // A story is reviewed beside the component it shows.
    name: "a length inside a story",
    path: "packages/ui/src/components/Panel.stories.tsx",
    contents: 'export const Row = () => <div style={{ gap: "12px" }} />;\n',
    mustFail: true,
    rule: "dimension-outside-token-file",
  },
  {
    name: "a token reference",
    path: COMPONENT,
    contents:
      ".panel {\n  padding: var(--space-4);\n  border-radius: var(--radius-md);\n}\n",
    mustFail: false,
  },
  {
    // The acceptance that keeps `calc()` usable. A single stripping pass leaves
    // the inner `var()` behind and reports a violation inside correct CSS.
    name: "a calc() over two tokens",
    path: COMPONENT,
    contents:
      ".panel {\n  width: calc(var(--switch-width) - var(--border-width) * 4);\n}\n",
    mustFail: false,
  },
  {
    // Relationships to a parent, not sizes. A token would make them less clear.
    name: "percentages and unitless numbers",
    path: COMPONENT,
    contents:
      ".panel {\n  width: 100%;\n  flex: 1;\n  opacity: 1;\n  transform: translate(-50%, -50%);\n}\n",
    mustFail: false,
  },
  {
    name: "zero, which is zero in every unit",
    path: COMPONENT,
    contents: ".panel {\n  margin: 0;\n  inset: 0;\n}\n",
    mustFail: false,
  },
  {
    name: "one full turn",
    path: COMPONENT,
    contents:
      "@keyframes spin {\n  to {\n    transform: rotate(360deg);\n  }\n}\n",
    mustFail: false,
  },
  {
    // The whole point of the exemption: this is where values live.
    name: "the token files, which hold the values",
    path: "packages/ui/src/tokens/scales.css",
    contents: ":root {\n  --space-4: 1rem;\n  --duration-fast: 80ms;\n}\n",
    mustFail: false,
  },
  {
    // A test has to be able to write a number in order to assert on one.
    name: "a test file",
    path: "packages/ui/src/components/Panel.test.tsx",
    contents: 'expect(style.padding).toBe("16px");\n',
    mustFail: false,
  },
  {
    // `unicode-range` is a character range, not a measurement.
    name: "a font-face unicode range",
    path: "packages/ui/src/fonts/fonts.css",
    contents: "@font-face {\n  unicode-range: U+0000-00FF, U+2000-206F;\n}\n",
    mustFail: false,
  },
  {
    // Prose explaining a token names its value. If the gate read comments, the
    // documentation in every component file would become a violation.
    name: "a value inside a comment",
    path: COMPONENT,
    contents:
      "/* 28px is what a dense toolbar wants */\n.panel {\n  height: var(--control-height-md);\n}\n",
    mustFail: false,
  },
];

function runGate(root) {
  return spawnSync(process.execPath, [GATE, root], { encoding: "utf8" });
}

let failures = 0;

for (const testCase of CASES) {
  const tree = mkdtempSync(join(tmpdir(), "blinkify-token-gate-"));

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
    `\ndesign-token-gate-test: ${String(failures)} case(s) failed. The gate does not\n` +
      "prove what it claims, so a hard-coded value can reach main and the design\n" +
      "system can drift off its own scale unnoticed.",
  );
  process.exit(1);
}

console.log(
  `\ndesign-token-gate-test: ${String(CASES.length)} case(s), all as expected.`,
);
