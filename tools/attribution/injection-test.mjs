#!/usr/bin/env node
/**
 * Injection test for the attribution staleness gate (#73).
 *
 * `pnpm attribution:check` passing on today's tree proves only that today's
 * document is current. It does not prove the gate would notice a new
 * dependency — and a gate that cannot is worse than none, because it is the
 * reason nobody else looks.
 *
 * So this builds a throwaway tree in the temp directory — a one-crate cargo
 * workspace and a one-package renderer — writes its attribution document,
 * then adds a dependency to each side in turn and asserts the gate fails
 * **naming the package that was added**, then reverts and asserts it passes
 * again. The added crate is a real one from the local cargo registry, at the
 * version Blinkify's own lockfile pins, resolved with `--offline`; the added
 * renderer package is linked to the one Blinkify already installed. Nothing
 * is fetched and the repository is never modified.
 *
 * Run: pnpm attribution:injection
 */
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ROOT, resolvePackage } from "./attribution.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

/** A crate every Blinkify build resolves, at the version Cargo.lock pins. */
const CRATE = "itoa";
/** A renderer package Blinkify ships. */
const PACKAGE = "zustand";

function lockedVersion(name) {
  const lock = readFileSync(join(ROOT, "Cargo.lock"), "utf8");
  const match = new RegExp(
    `\\[\\[package\\]\\]\\nname = "${name}"\\nversion = "([^"]+)"`,
  ).exec(lock.replace(/\r\n/g, "\n"));
  if (!match) {
    console.error(`attribution:injection: ${name} is not in Cargo.lock.`);
    process.exit(1);
  }
  return match[1];
}

const temp = mkdtempSync(join(tmpdir(), "blinkify-attribution-"));
const workspace = join(temp, "workspace");
const renderer = join(temp, "renderer");
const document = join(temp, "NOTICES.txt");

function writeCrate(withDependency) {
  mkdirSync(join(workspace, "canary", "src"), { recursive: true });
  writeFileSync(
    join(workspace, "Cargo.toml"),
    '[workspace]\nmembers = ["canary"]\nresolver = "2"\n',
  );
  writeFileSync(
    join(workspace, "canary", "Cargo.toml"),
    [
      "[package]",
      'name = "blinkify-attribution-canary"',
      'version = "0.0.0"',
      'edition = "2021"',
      'license = "MIT"',
      "publish = false",
      "",
      "[dependencies]",
      ...(withDependency ? [`${CRATE} = "=${lockedVersion(CRATE)}"`] : []),
      "",
    ].join("\n"),
  );
  writeFileSync(join(workspace, "canary", "src", "lib.rs"), "\n");
  const lock = spawnSync(
    "cargo",
    [
      "generate-lockfile",
      "--offline",
      "--manifest-path",
      join(workspace, "Cargo.toml"),
    ],
    { encoding: "utf8" },
  );
  if (lock.status !== 0) {
    console.error(
      "attribution:injection: could not resolve the canary workspace offline.",
    );
    console.error(lock.stderr);
    console.error(
      "Run `cargo metadata` in the repository once so the registry holds its crates.",
    );
    process.exit(1);
  }
}

function writeRenderer(withDependency) {
  rmSync(renderer, { recursive: true, force: true });
  mkdirSync(join(renderer, "node_modules"), { recursive: true });
  const dependencies = {};
  if (withDependency) {
    const installed = resolvePackage(join(ROOT, "apps", "desktop"), PACKAGE);
    if (installed === null) {
      console.error(`attribution:injection: ${PACKAGE} is not installed.`);
      process.exit(1);
    }
    dependencies[PACKAGE] = "*";
    symlinkSync(installed, join(renderer, "node_modules", PACKAGE), "junction");
  }
  writeFileSync(
    join(renderer, "package.json"),
    JSON.stringify(
      { name: "blinkify-attribution-canary", version: "0.0.0", dependencies },
      null,
      2,
    ),
  );
}

const inputs = [
  "--manifest-path",
  join(workspace, "Cargo.toml"),
  "--npm-root",
  renderer,
  "--components",
  "none",
  "--document",
  document,
  "--offline",
];

function run(script) {
  const result = spawnSync(process.execPath, [join(HERE, script), ...inputs], {
    encoding: "utf8",
  });
  return {
    status: result.status,
    output: `${result.stdout ?? ""}${result.stderr ?? ""}`,
  };
}

let failed = 0;
function expect(what, ok, output) {
  if (ok) {
    console.log(`  ok   ${what}`);
    return;
  }
  failed += 1;
  console.error(`  FAIL ${what}`);
  console.error(
    output
      .trim()
      .split(/\r?\n/)
      .map((line) => `       ${line}`)
      .join("\n"),
  );
}

try {
  writeCrate(false);
  writeRenderer(false);
  const written = run("generate.mjs");
  if (written.status !== 0) {
    console.error("attribution:injection: the baseline document failed.");
    console.error(written.output);
    process.exit(1);
  }

  let check = run("check.mjs");
  expect(
    "the gate passes on an unchanged tree (the control)",
    check.status === 0,
    check.output,
  );

  writeCrate(true);
  check = run("check.mjs");
  expect(
    `adding the crate ${CRATE} without regenerating fails the gate, naming it`,
    check.status !== 0 &&
      new RegExp(`\\+ ${CRATE} \\S+ \\(Rust crate\\)`).test(check.output),
    check.output,
  );

  writeCrate(false);
  check = run("check.mjs");
  expect("reverting the crate passes again", check.status === 0, check.output);

  writeRenderer(true);
  check = run("check.mjs");
  expect(
    `adding the renderer package ${PACKAGE} without regenerating fails the gate, naming it`,
    check.status !== 0 &&
      new RegExp(`\\+ ${PACKAGE} \\S+ \\(npm package\\)`).test(check.output),
    check.output,
  );

  writeRenderer(false);
  check = run("check.mjs");
  expect(
    "reverting the package passes again",
    check.status === 0,
    check.output,
  );
} finally {
  rmSync(temp, { recursive: true, force: true });
}

if (failed > 0) {
  console.error(
    `\nattribution:injection: ${failed} case(s) failed. The staleness gate would let a dependency ship unattributed.`,
  );
  process.exit(1);
}

console.log(
  "\nattribution:injection: the staleness gate fails on a new crate and a new renderer package.",
);
