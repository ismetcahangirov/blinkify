#!/usr/bin/env node
/**
 * Query the Blinkify project graph.
 *
 * Answers, for a given file, the questions CLAUDE.md section 7 step 3 requires
 * before touching existing code:
 *   - what does this import?
 *   - what imports this? (blast radius)
 *   - which tests cover it?
 *   - which layers are affected?
 *
 * Usage:
 *   node tools/project-graph/query.mjs apps/desktop/src/shell.store.ts
 *   node tools/project-graph/query.mjs shell.store          # substring match
 *   node tools/project-graph/query.mjs --untested packages  # files lacking tests
 */
import { readFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const INDEX = join(HERE, "output", "index.json");

if (!existsSync(INDEX)) {
  console.error("No graph found. Run `pnpm graph` first.");
  process.exit(1);
}

const index = JSON.parse(readFileSync(INDEX, "utf8"));
const args = process.argv.slice(2);

if (args.length === 0) {
  console.error("Usage: query.mjs <file-or-substring> | --untested [prefix]");
  process.exit(1);
}

if (args[0] === "--untested") {
  const prefix = args[1] ?? "";
  const rows = Object.entries(index.files)
    .filter(
      ([p, i]) =>
        !i.isTest && i.coveredByTests.length === 0 && p.startsWith(prefix),
    )
    .map(([p]) => p)
    .sort();
  console.log(`Files with no direct test (${rows.length}):`);
  for (const r of rows) console.log("  " + r);
  process.exit(0);
}

const needle = args[0].replace(/\\/g, "/");
const matches = Object.keys(index.files).filter(
  (p) => p === needle || p.includes(needle),
);

if (matches.length === 0) {
  console.error(`No file in the graph matches "${needle}".`);
  console.error("If the file is new, run `pnpm graph` to pick it up.");
  process.exit(1);
}

if (matches.length > 1 && !index.files[needle]) {
  console.log(`${matches.length} matches for "${needle}":`);
  for (const m of matches) console.log("  " + m);
  console.log("\nRe-run with a full path for the impact report.");
  process.exit(0);
}

const target = index.files[needle] ? needle : matches[0];
const info = index.files[target];

const list = (label, arr) => {
  console.log(`\n${label} (${arr.length}):`);
  if (arr.length === 0) console.log("  —");
  else for (const a of [...arr].sort()) console.log("  " + a);
};

console.log(`\n=== ${target} ===`);
console.log(`workspace: ${info.workspace}`);
console.log(`layer:     ${info.layer}`);
console.log(`is test:   ${info.isTest}`);
if (info.hasNoDependents && !info.isTest) {
  console.log(
    "NOTE: nothing imports this file. Either it is an entry point or it is dead code.",
  );
}

list("Depends on", info.dependsOn);
list("Depended on by (blast radius)", info.dependedOnBy);
list("Covered by tests", info.coveredByTests);

const affectedLayers = [
  ...new Set(
    info.dependedOnBy.map((f) => index.files[f]?.layer).filter(Boolean),
  ),
];
list("Affected layers", affectedLayers);

if (!info.isTest && info.coveredByTests.length === 0) {
  console.log(
    "\nNOTE: no test imports this file. Definition of Done requires adding one.",
  );
}

// The graph is static analysis over TypeScript imports. It cannot see the IPC
// edge, which is the one that matters most in this codebase.
if (info.layer === "ipc-contract") {
  console.log(
    "\nNOTE: this is the generated IPC contract. Its real source is the Rust type in\n" +
      "      crates/ — edit that and run `pnpm types:generate`, never this file. The\n" +
      "      graph cannot see that edge; `pnpm boundaries:rust` covers the Rust side.",
  );
} else if (info.layer === "tauri-shell") {
  console.log(
    "\nNOTE: Tauri commands and events are resolved by name at runtime. The graph sees\n" +
      "      no edge from the renderer to this file even when one exists — grep the\n" +
      "      command name in apps/desktop/src/ as well.",
  );
}
