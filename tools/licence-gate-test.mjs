#!/usr/bin/env node
/**
 * Injection test for the Rust licence gate.
 *
 * `cargo deny check licenses` passing on today's tree proves only that today's
 * tree is clean. It does not prove the gate would catch a GPL crate, and a
 * licence gate that cannot catch a GPL crate is the most expensive kind of
 * green tick this project could own: ADR-0002 and CLAUDE.md section 10 note
 * that a single GPL dependency makes Blinkify GPL and removes the option of a
 * commercial product. That is a one-way door, and a gate in front of a one-way
 * door has to be watched failing.
 *
 * So this builds a throwaway workspace in the temp directory containing one
 * crate that declares `GPL-3.0-only`, points `cargo deny` at it with
 * *Blinkify's own deny.toml*, and asserts the run fails. The repository is
 * never modified — no Cargo.toml is edited, no Cargo.lock is regenerated, and
 * an interrupted run leaves nothing behind but a temp directory.
 *
 * Run: pnpm deny:test
 */
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");
const CONFIG = join(ROOT, "deny.toml");

/**
 * Licences that must be rejected, and the reason each one is a door we cannot
 * walk back through. GPL and AGPL reach the combined work; LGPL reaches it too
 * when statically linked, which is what a Rust crate does — ADR-0002 permits
 * LGPL only across the process boundary the FFmpeg sidecar sits behind.
 */
const MUST_REJECT = [
  { spdx: "GPL-3.0-only", why: "would make the whole application GPL" },
  {
    spdx: "AGPL-3.0-only",
    why: "adds a network-use obligation on top of that",
  },
  {
    spdx: "LGPL-3.0-only",
    why: "a Rust crate is statically linked; ADR-0002 allows LGPL only across a process boundary",
  },
];

/** A licence that must still pass, so a failure here means the gate is simply broken. */
const MUST_ACCEPT = {
  spdx: "MIT",
  why: "the control: the gate must not reject everything",
};

function buildWorkspace(dir, spdx) {
  mkdirSync(join(dir, "canary", "src"), { recursive: true });

  writeFileSync(
    join(dir, "Cargo.toml"),
    [
      "[workspace]",
      'members = ["canary"]',
      'resolver = "2"',
      "",
      "[workspace.package]",
      'edition = "2021"',
      "",
    ].join("\n"),
  );

  writeFileSync(
    join(dir, "canary", "Cargo.toml"),
    [
      "[package]",
      'name = "blinkify-licence-canary"',
      'version = "0.0.0"',
      'edition = "2021"',
      `license = "${spdx}"`,
      "publish = false",
      "",
      "[dependencies]",
      "",
    ].join("\n"),
  );

  writeFileSync(
    join(dir, "canary", "src", "lib.rs"),
    "//! Exists only so `cargo metadata` has a package to report a licence for.\n",
  );
}

/** Run cargo-deny against a throwaway workspace using the project's config. */
function runGate(workspace) {
  const result = spawnSync(
    "cargo",
    [
      "deny",
      "--manifest-path",
      join(workspace, "Cargo.toml"),
      "--config",
      CONFIG,
      // The canary has no dependencies, so there is nothing to fetch; this
      // keeps the test from depending on the network or on a warm registry.
      "--offline",
      "check",
      "licenses",
    ],
    { encoding: "utf8" },
  );

  if (result.error) {
    console.error(
      "deny:test: could not run `cargo deny`. Is cargo-deny installed?",
    );
    console.error("deny:test:   cargo install cargo-deny --locked");
    console.error(`deny:test: ${result.error.message}`);
    process.exit(1);
  }

  return {
    status: result.status,
    output: `${result.stdout ?? ""}${result.stderr ?? ""}`,
  };
}

const workspace = mkdtempSync(join(tmpdir(), "blinkify-licence-gate-"));
let failed = 0;

try {
  for (const { spdx, why } of MUST_REJECT) {
    buildWorkspace(workspace, spdx);
    const { status, output } = runGate(workspace);

    if (status !== 0 && /rejected|not allowed|license/i.test(output)) {
      console.log(`  ok   ${spdx} rejected — ${why}`);
    } else {
      console.error(`  FAIL ${spdx} was NOT rejected (exit ${status}).`);
      console.error(
        "       deny.toml would let a copyleft crate into the tree.",
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

  buildWorkspace(workspace, MUST_ACCEPT.spdx);
  const { status, output } = runGate(workspace);
  if (status === 0) {
    console.log(`  ok   ${MUST_ACCEPT.spdx} accepted — ${MUST_ACCEPT.why}`);
  } else {
    console.error(`  FAIL ${MUST_ACCEPT.spdx} was rejected (exit ${status}).`);
    console.error(
      "       A gate that rejects everything passes the tests above and blocks all work.",
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
} finally {
  rmSync(workspace, { recursive: true, force: true });
}

if (failed > 0) {
  console.error(
    `\ndeny:test: ${failed} case(s) failed. See ADR-0002 before changing deny.toml.`,
  );
  process.exit(1);
}

console.log(
  `\ndeny:test: the licence gate rejects copyleft and accepts permissive.`,
);
