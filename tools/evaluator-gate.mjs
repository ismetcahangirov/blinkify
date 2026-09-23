#!/usr/bin/env node
/**
 * The evaluator boundary (#30): one piece of code interprets the edit graph.
 *
 * A preview that disagrees with the export is found out after a long render,
 * and two interpretations of the graph will disagree eventually. So the
 * shared evaluator (`crates/blinkify-engine/src/project/evaluate.rs`) is the
 * only reader of a clip's operations, and this gate keeps it that way where
 * the compiler cannot:
 *
 * - **Rust.** `Clip`'s operations are private to `blinkify_engine::project`,
 *   so no other module can read them — the compiler enforces that. What it
 *   cannot stop is code elsewhere taking an `Operation` apart by hand. No
 *   file outside `project/` may name an `Operation::` variant. Tests may:
 *   they build graphs.
 * - **TypeScript.** The renderer receives the graph as data, and could
 *   interpret it — a trim drawn from `clip.operations` is a second
 *   interpretation. No renderer or design-system source may read an
 *   `.operations` property. What the renderer shows about operations comes
 *   from the engine's `operations_at`, already evaluated.
 *
 * `AudioOperation::` — the evaluator's own output — is not an `Operation::`
 * and is not matched.
 *
 * Run: pnpm evaluator:check. Proved by: pnpm evaluator:check:test.
 */
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");

const RUST_ROOTS = ["crates", join("apps", "desktop", "src-tauri", "src")];
const TS_ROOTS = [
  join("apps", "desktop", "src"),
  join("packages", "ui", "src"),
];
const EVALUATOR = "crates/blinkify-engine/src/project/";
const SKIP = new Set(["node_modules", "target", "dist", "generated", ".turbo"]);

/** Every file under `dir` with one of `extensions`, as repository paths. */
function walk(dir, extensions, found = []) {
  let entries;
  try {
    entries = readdirSync(join(ROOT, dir), { withFileTypes: true });
  } catch {
    return found;
  }
  for (const entry of entries) {
    if (SKIP.has(entry.name)) continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path, extensions, found);
    else if (extensions.some((extension) => entry.name.endsWith(extension)))
      found.push(path.split(sep).join("/"));
  }
  return found;
}

const isTest = (path) =>
  /(^|\/)tests\//.test(path) || /\.test\.tsx?$/.test(path);

/**
 * The violations in one file, as `path:line: reason`. Pure, so the injection
 * test can feed it files that do not exist.
 */
export function violations(path, text) {
  if (isTest(path)) return [];
  const rust = path.endsWith(".rs");
  if (rust && path.startsWith(EVALUATOR)) return [];
  const pattern = rust ? /\bOperation::/ : /\.operations\b/;
  const reason = rust
    ? "takes an edit-graph Operation apart outside the evaluator"
    : "reads a clip's operations in the renderer; ask the engine (operations_at)";
  return text
    .split("\n")
    .flatMap((line, index) =>
      pattern.test(line) ? [`${path}:${index + 1}: ${reason}`] : [],
    );
}

function main() {
  const files = [
    ...RUST_ROOTS.flatMap((root) => walk(root, [".rs"])),
    ...TS_ROOTS.flatMap((root) => walk(root, [".ts", ".tsx"])),
  ];
  const found = files.flatMap((path) =>
    violations(path, readFileSync(join(ROOT, path), "utf8")),
  );
  if (found.length > 0) {
    console.error("The edit graph is interpreted outside the evaluator (#30):");
    for (const violation of found) console.error(`  ${violation}`);
    process.exit(1);
  }
  console.log(
    `Only the evaluator interprets the edit graph — ${files.length} file(s) checked.`,
  );
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main();
}
