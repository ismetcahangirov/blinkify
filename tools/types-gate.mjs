#!/usr/bin/env node
/**
 * The IPC contract gate: the committed TypeScript types are exactly what the
 * Rust types generate.
 *
 * `packages/types` is generated, never hand-written (CLAUDE.md section 2).
 * That only holds if someone runs `pnpm types:generate` after changing a Rust
 * type — and #32 requires that the two "cannot drift", which needs a check,
 * not a habit. So this regenerates the whole contract into a scratch
 * directory and compares it with the committed one, file by file:
 *
 * - a changed type whose `.ts` was not regenerated,
 * - a new type whose `.ts` was not committed,
 * - a removed type whose `.ts` is still there (ts-rs never deletes a file,
 *   so a plain `git status` after `cargo test` would miss this one),
 * - a barrel that does not list exactly the generated modules.
 *
 * ts-rs reads `TS_RS_EXPORT_DIR` when the tests run, and `.cargo/config.toml`
 * does not force it, so the environment redirects the output without
 * touching the working tree.
 *
 * Run: pnpm types:check. Proved by: pnpm types:check:test.
 */
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");
export const COMMITTED = join(ROOT, "packages", "types", "src", "generated");

const read = (dir) =>
  new Map(
    (existsSync(dir) ? readdirSync(dir) : [])
      .filter((name) => name.endsWith(".ts"))
      .sort()
      // Line endings are git's business (.gitattributes), not the contract's.
      .map((name) => [
        name,
        readFileSync(join(dir, name), "utf8").replaceAll("\r\n", "\n"),
      ]),
  );

/**
 * Every difference between the generated and the committed contract, as a
 * sentence naming the file. Empty when they agree.
 */
export function compare(generatedDir, committedDir) {
  const generated = read(generatedDir);
  const committed = read(committedDir);
  const problems = [];
  for (const [name, text] of generated) {
    if (!committed.has(name)) {
      problems.push(`${name} is generated but not committed`);
    } else if (committed.get(name) !== text) {
      problems.push(`${name} differs from what the Rust types generate`);
    }
  }
  for (const name of committed.keys()) {
    if (!generated.has(name)) {
      problems.push(`${name} is committed but no Rust type generates it`);
    }
  }
  return problems;
}

function run(command, args, env) {
  const result = spawnSync(command, args, {
    cwd: ROOT,
    env: { ...process.env, ...env },
    stdio: ["ignore", "inherit", "inherit"],
    shell: false,
  });
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed (${result.status})`);
  }
}

function main() {
  const scratch = mkdtempSync(join(tmpdir(), "blinkify-types-"));
  try {
    run("cargo", ["test", "--workspace", "export_bindings", "--quiet"], {
      TS_RS_EXPORT_DIR: scratch,
    });
    run(process.execPath, [join(HERE, "generate-types-index.mjs"), scratch]);
    const problems = compare(scratch, COMMITTED);
    if (problems.length > 0) {
      console.error(
        "The committed IPC contract has drifted from the Rust types:",
      );
      for (const problem of problems) console.error(`  - ${problem}`);
      console.error(
        "Run `pnpm types:generate` and commit packages/types/src/generated.",
      );
      process.exit(1);
    }
    console.log(
      `IPC contract matches the Rust types — ${read(scratch).size} file(s).`,
    );
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main();
}
