#!/usr/bin/env node
/**
 * Staleness gate for the committed attribution document (#73).
 *
 * Regenerates the document in memory and fails if it differs from the file
 * the installer bundles. Adding a crate or a renderer package and not running
 * `pnpm attribution` ships a binary whose notices leave that package out —
 * a compliance failure that nothing else would notice, because the
 * application builds, installs and runs exactly the same.
 *
 * A diff can mean only "the dependency set moved" because the generator is a
 * pure function of the tree; `pnpm attribution:determinism` is the standing
 * proof. The file on disk is never modified here: the gate reports, and the
 * contributor regenerates and reviews.
 *
 * Run: pnpm attribution:check
 */
import { existsSync, readFileSync } from "node:fs";
import { relative } from "node:path";
import {
  AttributionError,
  ROOT,
  generate,
  optionsFromArgs,
} from "./attribution.mjs";

const options = optionsFromArgs(process.argv.slice(2));
const shown = relative(ROOT, options.document).replaceAll("\\", "/");

let fresh;
try {
  fresh = generate(options);
} catch (error) {
  console.error(
    error instanceof AttributionError
      ? `attribution:check: ${error.message}`
      : `attribution:check: generation failed.\n${error.message}`,
  );
  process.exit(1);
}

const committed = existsSync(options.document)
  ? readFileSync(options.document, "utf8")
  : null;

if (committed === fresh) {
  console.log(
    `attribution:check: ${shown} matches the dependency tree (${Buffer.byteLength(fresh)} bytes).`,
  );
  process.exit(0);
}

/** The Part 2 entry lines — one per package — for a readable summary. */
const entries = (text) =>
  new Set(
    (text ?? "")
      .split("\n")
      .filter((line) => / \((Rust crate|npm package)\)$/.test(line)),
  );
const before = entries(committed);
const after = entries(fresh);
const added = [...after].filter((line) => !before.has(line));
const removed = [...before].filter((line) => !after.has(line));

console.error("");
console.error(`attribution:check: ${shown} is stale.`);
console.error("");
if (committed === null) console.error("  The document does not exist.");
for (const line of added) console.error(`  + ${line}`);
for (const line of removed) console.error(`  - ${line}`);
if (committed !== null && added.length === 0 && removed.length === 0)
  console.error(
    "  The same packages, but a licence, a text or a bundled component changed.",
  );
console.error("");
console.error(
  "The dependency set moved and the attribution document was not regenerated.",
);
console.error("Regenerate it, review the difference and commit it:");
console.error("");
console.error("  pnpm attribution");
console.error(`  git add ${shown}`);
console.error("");
process.exit(1);
