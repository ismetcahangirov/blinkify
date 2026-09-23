#!/usr/bin/env node
/**
 * Injection test for the IPC contract gate.
 *
 * `pnpm types:check` passing proves the contract matches today. It does not
 * prove the gate would notice a drift — CLAUDE.md section 14: a rule you have
 * not seen fail is a rule you have not tested. Each case starts from a copy
 * of the committed contract, which must pass, breaks it one way, and asserts
 * the gate names the file.
 *
 * Run: pnpm types:check:test
 */
import {
  cpSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  unlinkSync,
  writeFileSync,
  appendFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { COMMITTED, compare } from "./types-gate.mjs";

const some = readdirSync(COMMITTED).find(
  (name) => name.endsWith(".ts") && name !== "index.ts",
);
if (!some) throw new Error("no generated types to test against");

const cases = [
  ["a clean copy passes", () => {}, null],
  [
    "a Rust change not regenerated",
    (dir) => appendFileSync(join(dir, some), "// edited\n"),
    `${some} differs`,
  ],
  [
    "a new type not committed",
    (dir) => writeFileSync(join(dir, "Brand.ts"), "export type Brand = 1;\n"),
    "Brand.ts is generated but not committed",
  ],
  [
    "a removed type still committed",
    (dir) => unlinkSync(join(dir, some)),
    `${some} is committed but no Rust type generates it`,
  ],
];

let failed = 0;
for (const [label, breakIt, expected] of cases) {
  const root = mkdtempSync(join(tmpdir(), "blinkify-types-test-"));
  try {
    const generated = join(root, "generated");
    cpSync(COMMITTED, generated, { recursive: true });
    breakIt(generated);
    const problems = compare(generated, COMMITTED);
    const ok =
      expected === null
        ? problems.length === 0
        : problems.some((problem) => problem.includes(expected));
    if (!ok) {
      failed += 1;
      console.error(`FAIL ${label}: ${JSON.stringify(problems)}`);
    } else {
      console.log(`ok   ${label}`);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

if (failed > 0) process.exit(1);
