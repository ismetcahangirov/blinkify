#!/usr/bin/env node
/**
 * The edit boundary (#37): every change to the open graph is an edit, and
 * goes through the history.
 *
 * An undo stack that some mutation bypasses undoes the wrong thing, silently
 * — worse than no undo. The engine's `Document` owns the open project and
 * hands out only a shared reference, so the compiler already stops the shell
 * from changing the document's graph. This gate closes what the compiler
 * cannot see:
 *
 * - **Rust.** A function outside `crates/blinkify-engine/src/project/` that
 *   takes `&mut Project`, or binds a `mut` `Project`, is building a graph
 *   change outside the edit layer. Tests may: they build graphs.
 * - **TypeScript.** Only `apps/desktop/src/project/project.store.ts` sends
 *   the commands that change the graph (`edit_project`, `undo_edit`,
 *   `redo_edit`, `begin_gesture`, `end_gesture`). A component that invokes
 *   one directly skips the store's selection and playhead context, so its
 *   undo would not restore them.
 *
 * Run: pnpm edits:check. Proved by: pnpm edits:check:test.
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
const EDIT_LAYER = "crates/blinkify-engine/src/project/";
const STORE = "apps/desktop/src/project/project.store.ts";
const SKIP = new Set(["node_modules", "target", "dist", "generated", ".turbo"]);

const RUST_PATTERN = /&\s*mut\s+Project\b|\bmut\s+\w+\s*:\s*Project\b/;
const TS_PATTERN =
  /["'`](edit_project|undo_edit|redo_edit|begin_gesture|end_gesture)["'`]/;

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

/** Whether `line` is inside a Rust `#[cfg(test)]` module of `lines`. */
function inRustTests(lines, index) {
  return lines.slice(0, index).some((line) => /#\[cfg\(test\)\]/.test(line));
}

/**
 * The violations in one file, as `path:line: reason`. Pure, so the injection
 * test can feed it files that do not exist.
 */
export function violations(path, text) {
  if (isTest(path)) return [];
  const rust = path.endsWith(".rs");
  if (rust && path.startsWith(EDIT_LAYER)) return [];
  if (!rust && path === STORE) return [];
  const pattern = rust ? RUST_PATTERN : TS_PATTERN;
  const reason = rust
    ? "changes a Project outside the edit layer; make it an Edit (project::edit)"
    : "sends a graph edit around the project store; call useProjectStore";
  const lines = text.split("\n");
  return lines.flatMap((line, index) =>
    pattern.test(line) && !(rust && inRustTests(lines, index))
      ? [`${path}:${index + 1}: ${reason}`]
      : [],
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
    console.error("The edit graph is changed outside the history (#37):");
    for (const violation of found) console.error(`  ${violation}`);
    process.exit(1);
  }
  console.log(
    `Every graph change is an edit — ${files.length} file(s) checked.`,
  );
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main();
}
