#!/usr/bin/env node
/**
 * Injection test for the architecture gates.
 *
 * CLAUDE.md section 14 states the failure mode this file exists to prevent:
 * `includeOnly`, `node_modules` in `exclude`, and unanchored `exclude`
 * patterns all silently delete the edges the rules reason about, leaving a
 * gate that passes and proves nothing. The gate looks identical either way —
 * green — so the only way to know a rule still works is to watch it fail.
 *
 * So: for every rule, write the violation it forbids into the working tree,
 * run the gate, assert it fails *with that rule's name*, and remove the file
 * again. A rule that does not fire here is either gone or disarmed.
 *
 * Asserting on the rule name, not merely on a non-zero exit, is the point. An
 * exit code of 1 could come from any of the eight rules or from
 * dependency-cruiser refusing to start; only the name proves that the specific
 * boundary is the thing that caught it.
 *
 * Run: pnpm graph:injection
 */
import { spawnSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const DEPCRUISE = join(
  ROOT,
  "node_modules",
  "dependency-cruiser",
  "bin",
  "dependency-cruiser.mjs",
);

/**
 * Each case names the rule it must trigger and the file that triggers it.
 * `cleanup` lists every path to remove afterwards, deepest first, so a
 * directory created for the test does not survive it.
 */
const CASES = [
  {
    rule: "renderer-not-into-engine",
    why: "the renderer must reach the engine only through the generated IPC contract",
    files: {
      // A hand-written mirror of the contract, parked in the Tauri shell. This
      // is the realistic version of the violation: not a renderer importing a
      // .rs file, which would not compile anyway, but a second copy of the
      // contract drifting away from the Rust source of truth.
      "apps/desktop/src-tauri/injected-contract.ts":
        "export const INJECTED_TIER = 1;\n",
      "apps/desktop/src/injected-renderer-reach.ts":
        'import { INJECTED_TIER } from "../src-tauri/injected-contract.js";\n\n' +
        "export const tier = INJECTED_TIER;\n",
    },
    cleanup: [
      "apps/desktop/src/injected-renderer-reach.ts",
      "apps/desktop/src-tauri/injected-contract.ts",
    ],
  },
  {
    rule: "ui-package-stays-shared",
    why: "the design system must not reach back into an application",
    files: {
      "packages/ui/src/injected-app-reach.ts":
        'import { useShellStore } from "../../../apps/desktop/src/shell.store.js";\n\n' +
        "export const store = useShellStore;\n",
    },
    cleanup: ["packages/ui/src/injected-app-reach.ts"],
  },
  {
    rule: "engine-contract-is-generated",
    why: "the IPC contract must be readable by anything that speaks to the engine",
    files: {
      // Relative, not `@blinkify/ui`. The bare specifier is undeclared in
      // packages/types/package.json, so it never resolves and
      // `no-non-package-json` catches it first — which proves that rule, not
      // this one. A relative path resolves, so the edge reaches the rule
      // engine as `packages/types/... -> packages/ui/...` and the boundary
      // rule is the thing under test.
      "packages/types/src/injected-ui-reach.ts":
        'import { DESIGN_SYSTEM_VERSION } from "../../ui/src/index.js";\n\n' +
        "export const version = DESIGN_SYSTEM_VERSION;\n",
    },
    cleanup: ["packages/types/src/injected-ui-reach.ts"],
  },
  {
    rule: "no-circular",
    why: "a cycle makes both modules impossible to reason about or test in isolation",
    files: {
      "apps/desktop/src/injected-cycle-a.ts":
        'import { b } from "./injected-cycle-b.js";\n\nexport const a = b;\n',
      "apps/desktop/src/injected-cycle-b.ts":
        'import { a } from "./injected-cycle-a.js";\n\nexport const b = a;\n',
    },
    cleanup: [
      "apps/desktop/src/injected-cycle-a.ts",
      "apps/desktop/src/injected-cycle-b.ts",
    ],
  },
  {
    rule: "not-to-dev-dep",
    why: "a devDependency is absent from the bundle the installer carries",
    files: {
      // `vite` is a devDependency of @blinkify/desktop. Importing it from a
      // non-test renderer file is the shape of the bug: it works in `pnpm dev`
      // and is missing from the shipped build.
      "apps/desktop/src/injected-dev-dep.ts":
        'import { defineConfig } from "vite";\n\nexport const config = defineConfig;\n',
    },
    cleanup: ["apps/desktop/src/injected-dev-dep.ts"],
  },
  {
    rule: "no-non-package-json",
    why: "a phantom dependency resolves on this machine and breaks on a clean install",
    files: {
      "apps/desktop/src/injected-phantom.ts":
        'import { thing } from "@blinkify/engine";\n\nexport const value = thing;\n',
    },
    cleanup: ["apps/desktop/src/injected-phantom.ts"],
  },
  {
    rule: "no-deprecated-core",
    why: "a deprecated Node core module disappears in a future runtime",
    files: {
      "tools/injected-deprecated-core.mjs":
        'import punycode from "punycode";\n\nexport const p = punycode;\n',
    },
    cleanup: ["tools/injected-deprecated-core.mjs"],
  },
];

function write(files) {
  for (const [rel, contents] of Object.entries(files)) {
    const abs = join(ROOT, rel);
    mkdirSync(dirname(abs), { recursive: true });
    writeFileSync(abs, contents);
  }
}

function remove(paths) {
  for (const rel of paths) {
    rmSync(join(ROOT, rel), { force: true });
  }
}

/** Run the gate and return its combined output, whatever the exit code. */
function runGate() {
  const result = spawnSync(
    process.execPath,
    [
      DEPCRUISE,
      "--config",
      ".dependency-cruiser.cjs",
      "--output-type",
      "json",
      "apps",
      "packages",
      "tools",
    ],
    { cwd: ROOT, encoding: "utf8", maxBuffer: 128 * 1024 * 1024 },
  );
  const stdout = result.stdout ?? "";
  if (stdout.trim() === "") {
    return { failedToRun: true, message: result.stderr ?? "(no output)" };
  }
  const cruise = JSON.parse(stdout);
  return {
    failedToRun: false,
    rulesFired: new Set(
      (cruise.summary?.violations ?? []).map((v) => v.rule.name),
    ),
    errorCount: cruise.summary?.error ?? 0,
  };
}

// A dirty baseline would make every result meaningless: a rule already firing
// before anything is injected proves nothing when it fires afterwards.
console.log("injection: checking the baseline is clean ...");
const baseline = runGate();
if (baseline.failedToRun) {
  console.error(
    "injection: dependency-cruiser would not run.\n",
    baseline.message,
  );
  process.exit(1);
}
if (baseline.rulesFired.size > 0) {
  console.error(
    `injection: the tree already violates ${[...baseline.rulesFired].join(", ")}. ` +
      "Fix that first — an injection test needs a clean baseline to mean anything.",
  );
  process.exit(1);
}

// A leftover file from an interrupted run would be indistinguishable from a
// real violation on the next run, and the message would be baffling.
const stale = CASES.flatMap((c) => c.cleanup).filter((p) =>
  existsSync(join(ROOT, p)),
);
if (stale.length > 0) {
  console.error(
    "injection: leftover files from an earlier run — delete them and retry:",
  );
  for (const s of stale) console.error("  " + s);
  process.exit(1);
}

let failed = 0;
for (const testCase of CASES) {
  write(testCase.files);
  let outcome;
  try {
    outcome = runGate();
  } finally {
    remove(testCase.cleanup);
  }

  if (outcome.failedToRun) {
    console.error(`  FAIL ${testCase.rule}: the gate would not run.`);
    failed += 1;
    continue;
  }

  if (outcome.rulesFired.has(testCase.rule)) {
    console.log(`  ok   ${testCase.rule} — fired, as it must: ${testCase.why}`);
  } else {
    const instead =
      outcome.rulesFired.size > 0
        ? [...outcome.rulesFired].sort().join(", ")
        : "nothing";
    console.error(
      `  FAIL ${testCase.rule} did not fire. ${instead} fired instead.`,
    );
    console.error(
      `       The violation written was: ${Object.keys(testCase.files).join(", ")}`,
    );
    console.error(
      "       Either the rule is gone, or something under `options:` in " +
        ".dependency-cruiser.cjs\n       is deleting the edge before the rule engine sees it.",
    );
    failed += 1;
  }
}

// The tree must be exactly as it was found. A test that leaves a violation
// behind poisons every gate that runs after it.
const after = runGate();
if (after.failedToRun || after.rulesFired.size > 0) {
  console.error(
    "injection: the tree is not clean after the run. Check for leftover files.",
  );
  process.exit(1);
}

if (failed > 0) {
  console.error(
    `\ninjection: ${failed} of ${CASES.length} rules did not fire.`,
  );
  process.exit(1);
}

console.log(
  `\ninjection: all ${CASES.length} rules fired on their own violation.`,
);
