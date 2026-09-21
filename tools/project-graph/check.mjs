#!/usr/bin/env node
/**
 * Staleness gate for the committed project graph.
 *
 * Regenerates the graph and fails if the result differs from what is committed.
 * This is meaningful only because the generator reads no clock and sorts every
 * collection (CLAUDE.md section 14): two runs on the same tree are
 * byte-identical, so a non-empty diff means the source tree actually moved and
 * the contributor forgot to run `pnpm graph`.
 *
 * `git status --porcelain` rather than `git diff`, deliberately: a diff misses
 * an output file that was never added to the index, which is exactly what
 * happens the first time a new artifact is emitted.
 *
 * Run: pnpm graph:check
 */
import { execFileSync, spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const OUTPUT_PATH = "tools/project-graph/output";

const generated = spawnSync(process.execPath, [join(HERE, "generate.mjs")], {
  cwd: ROOT,
  stdio: "inherit",
});

if (generated.status !== 0) {
  console.error("graph:check: the generator failed — see above.");
  process.exit(generated.status ?? 1);
}

let status;
try {
  status = execFileSync("git", ["status", "--porcelain", "--", OUTPUT_PATH], {
    cwd: ROOT,
    encoding: "utf8",
  });
} catch (error) {
  console.error(
    "graph:check: could not ask git about the output directory.\n",
    error.message,
  );
  process.exit(1);
}

if (status.trim() === "") {
  console.log("graph:check: committed graph matches the source tree.");
  process.exit(0);
}

console.error("");
console.error("graph:check: the committed project graph is stale.");
console.error("");
for (const line of status.trimEnd().split(/\r?\n/)) {
  console.error("  " + line.trim());
}
console.error("");
console.error(
  "The source tree moved and the graph was not regenerated. The working copy now",
);
console.error("holds the correct graph — review it and commit it:");
console.error("");
console.error(`  git add ${OUTPUT_PATH}`);
console.error("");
process.exit(1);
