#!/usr/bin/env node
/**
 * TypeScript architecture boundary gate.
 *
 * Wraps dependency-cruiser so the gate does not fail merely because a
 * workspace directory does not exist yet. Only genuine rule violations should
 * fail this job.
 *
 * The Rust half of the boundary story is `tools/crate-boundaries.mjs`. Run
 * both — `pnpm graph:validate` proves nothing about the engine.
 *
 * Run: pnpm graph:validate
 */
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const BIN = join(
  ROOT,
  "node_modules",
  "dependency-cruiser",
  "bin",
  "dependency-cruiser.mjs",
);

const targets = ["apps", "packages", "tools"].filter((d) =>
  existsSync(join(ROOT, d)),
);

if (targets.length === 0) {
  console.log(
    "graph:validate: no source directories present yet — nothing to check.",
  );
  process.exit(0);
}

const result = spawnSync(
  process.execPath,
  [BIN, "--config", ".dependency-cruiser.cjs", ...targets],
  { cwd: ROOT, stdio: "inherit" },
);

process.exit(result.status ?? 1);
