#!/usr/bin/env node
/**
 * Rust crate boundary gate.
 *
 * CLAUDE.md section 2: engine crates never import `tauri`. The reason is not
 * taste. An engine crate that depends on Tauri cannot be built or tested
 * without a window and a WebView, which means it will stop being tested — and
 * the engine is where every decision about what reaches the user's output file
 * is made. `cargo test -p blinkify-engine` has to pass with no Tauri present,
 * and the only way that stays true is if something fails when it stops being
 * true.
 *
 * dependency-cruiser cannot see any of this: it reads TypeScript. CLAUDE.md
 * section 14 is explicit that two languages need two tools, so this is the
 * second one.
 *
 * Checks the whole transitive dependency tree, not just the direct
 * dependencies — a crate that pulls in Tauri through an intermediate still
 * cannot be built without it, so a direct-only check would pass while the
 * property it claims to protect was already broken.
 *
 * Run: pnpm boundaries:rust
 *
 * Takes an optional workspace root so `tools/crate-boundaries-test.mjs` can
 * point it at a throwaway tree that does violate the rule — the only way to
 * know this check still works is to watch it fail.
 */
import { execFileSync } from "node:child_process";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = process.argv[2] ? resolve(process.argv[2]) : join(HERE, "..");

/**
 * Crates that may not reach Tauri, by the directory they live in. Everything
 * under `crates/` is the media engine; `apps/desktop/src-tauri` is the shell
 * and is the only place Tauri belongs.
 */
const ENGINE_DIR = "crates";

/**
 * Forbidden roots, as crate names. `tauri-build` is a build dependency of the
 * shell and equally out of bounds for the engine: a crate that needs it at
 * build time needs the Tauri toolchain present to compile.
 */
const FORBIDDEN = ["tauri", "tauri-build", "tauri-plugin", "wry", "tao"];

const isForbidden = (name) =>
  FORBIDDEN.includes(name) ||
  name.startsWith("tauri-plugin-") ||
  name.startsWith("tauri-utils");

let metadataJson;
try {
  metadataJson = execFileSync(
    "cargo",
    ["metadata", "--format-version", "1", "--all-features"],
    {
      cwd: ROOT,
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
    },
  );
} catch (error) {
  console.error("boundaries:rust: `cargo metadata` failed.\n", error.message);
  process.exit(1);
}

const metadata = JSON.parse(metadataJson);
const packagesById = new Map(metadata.packages.map((p) => [p.id, p]));
const nodesById = new Map(
  (metadata.resolve?.nodes ?? []).map((n) => [n.id, n]),
);

/** Workspace members whose manifest sits under `crates/`. */
const engineMembers = metadata.workspace_members
  .map((id) => packagesById.get(id))
  .filter(Boolean)
  .filter((p) => relative(ROOT, p.manifest_path).split(sep)[0] === ENGINE_DIR)
  .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));

if (engineMembers.length === 0) {
  console.error(
    `boundaries:rust: no workspace crate found under ${ENGINE_DIR}/.`,
  );
  console.error(
    "boundaries:rust: refusing to report success on an empty check.",
  );
  process.exit(1);
}

/**
 * Shortest dependency path from `rootId` to a forbidden crate, or null.
 * Breadth-first, so the path printed is the shortest one and therefore the
 * easiest to act on.
 */
function pathToForbidden(rootId) {
  const seen = new Set([rootId]);
  const queue = [[rootId]];

  while (queue.length > 0) {
    const trail = queue.shift();
    const node = nodesById.get(trail[trail.length - 1]);
    if (!node) continue;

    for (const dep of node.deps ?? []) {
      if (seen.has(dep.pkg)) continue;
      seen.add(dep.pkg);

      const pkg = packagesById.get(dep.pkg);
      const name = pkg?.name ?? dep.name;
      const next = [...trail, dep.pkg];

      if (isForbidden(name)) return next;
      queue.push(next);
    }
  }
  return null;
}

const failures = [];
for (const member of engineMembers) {
  const trail = pathToForbidden(member.id);
  if (trail) {
    failures.push({
      crate: member.name,
      chain: trail.map((id) => packagesById.get(id)?.name ?? id),
    });
  }
}

if (failures.length > 0) {
  console.error("");
  console.error("boundaries:rust: an engine crate depends on Tauri.");
  console.error("");
  for (const f of failures) {
    console.error(`  ${f.crate}`);
    console.error(`    ${f.chain.join(" -> ")}`);
  }
  console.error("");
  console.error(
    "CLAUDE.md section 2: engine crates never import tauri. Move whatever needs the",
  );
  console.error(
    "window into apps/desktop/src-tauri and have it call the engine, not the other",
  );
  console.error(
    "way round. `cargo test -p blinkify-engine` must pass with no Tauri present.",
  );
  console.error("");
  process.exit(1);
}

console.log(
  `boundaries:rust: ${engineMembers.length} engine crate(s) checked, none reaches Tauri ` +
    `(${engineMembers.map((c) => c.name).join(", ")}).`,
);
