#!/usr/bin/env node
/**
 * Injection test for the Rust crate boundary gate.
 *
 * `pnpm boundaries:rust` passing today proves the engine does not reach Tauri
 * today. It does not prove the check would notice if it did — and a check that
 * cannot notice is worse than no check, because it is believed.
 *
 * Three throwaway workspaces, each pointed at by `tools/crate-boundaries.mjs`
 * through its optional root argument:
 *
 *   1. an engine crate that depends on `tauri` directly       — must fail
 *   2. an engine crate that reaches it through an intermediate — must fail
 *   3. an engine crate that depends on neither                — must pass
 *
 * Case 2 is the one worth having. A direct-dependency check passes it while the
 * property it claims to protect — `cargo test -p blinkify-engine` building with
 * no Tauri present — is already broken.
 *
 * The real repository is never touched: no Cargo.toml is edited and no
 * Cargo.lock is regenerated. `tauri` here is a local stub crate with the right
 * name, so nothing is fetched.
 *
 * Run: pnpm boundaries:rust:test
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const CHECK = join(HERE, "crate-boundaries.mjs");

/**
 * @param {string} dir      workspace root to write into
 * @param {string[]} engineDeps  dependencies of the engine crate, by crate name
 */
function buildWorkspace(dir, engineDeps) {
  // Every stub the tree might need. Unused members cost nothing and keep the
  // manifest writing in one place.
  const stubs = ["tauri", "relay"];

  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, "Cargo.toml"),
    [
      "[workspace]",
      `members = ["crates/blinkify-engine", ${stubs.map((s) => `"vendor/${s}"`).join(", ")}]`,
      'resolver = "2"',
      "",
    ].join("\n"),
  );

  const crate = (path, name, deps) => {
    mkdirSync(join(dir, path, "src"), { recursive: true });
    writeFileSync(
      join(dir, path, "Cargo.toml"),
      [
        "[package]",
        `name = "${name}"`,
        'version = "0.0.0"',
        'edition = "2021"',
        'license = "MIT"',
        "publish = false",
        "",
        "[dependencies]",
        ...deps.map(
          (d) =>
            `${d} = { path = "${"../".repeat(path.split("/").length)}vendor/${d}" }`,
        ),
        "",
      ].join("\n"),
    );
    writeFileSync(join(dir, path, "src", "lib.rs"), "//! stub\n");
  };

  crate("crates/blinkify-engine", "blinkify-engine", engineDeps);
  crate("vendor/tauri", "tauri", []);
  // The intermediate. Nothing about its name says "Tauri", which is exactly
  // how this arrives in a real tree.
  crate("vendor/relay", "relay", ["tauri"]);
}

function runCheck(workspace) {
  const result = spawnSync(process.execPath, [CHECK, workspace], {
    encoding: "utf8",
    // `cargo metadata` needs a lockfile it is allowed to write; the workspace
    // is a temp directory, so let it.
    env: process.env,
  });
  return {
    status: result.status,
    output: `${result.stdout ?? ""}${result.stderr ?? ""}`,
  };
}

const CASES = [
  {
    name: "engine depends on tauri directly",
    deps: ["tauri"],
    mustPass: false,
    why: "the obvious violation",
  },
  {
    name: "engine reaches tauri through an intermediate",
    deps: ["relay"],
    mustPass: false,
    why: "the violation a direct-dependency check would miss",
  },
  {
    name: "engine depends on neither",
    deps: [],
    mustPass: true,
    why: "the control: the check must not fail on a clean tree",
  },
];

const workspace = mkdtempSync(join(tmpdir(), "blinkify-crate-boundaries-"));
let failed = 0;

try {
  for (const testCase of CASES) {
    buildWorkspace(workspace, testCase.deps);
    const { status, output } = runCheck(workspace);
    const passed = status === 0;

    if (passed === testCase.mustPass) {
      console.log(`  ok   ${testCase.name} — ${testCase.why}`);
    } else {
      console.error(
        `  FAIL ${testCase.name}: expected the check to ${testCase.mustPass ? "pass" : "fail"}, ` +
          `it ${passed ? "passed" : "failed"}.`,
      );
      console.error(
        output
          .trim()
          .split(/\r?\n/)
          .map((l) => "       " + l)
          .join("\n"),
      );
      failed += 1;
    }
  }
} finally {
  rmSync(workspace, { recursive: true, force: true });
}

if (failed > 0) {
  console.error(
    `\nboundaries:rust:test: ${failed} of ${CASES.length} cases failed.`,
  );
  process.exit(1);
}

console.log(
  "\nboundaries:rust:test: the check catches Tauri, direct and transitive.",
);
