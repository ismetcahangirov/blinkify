#!/usr/bin/env node
/**
 * The version gate.
 *
 * A desktop application's version is not decoration: it is what an installed
 * build reports, what the updater compares against the release manifest, and
 * what a bug report names. Blinkify carries it in five places across two
 * package managers, and nothing about editing one of them reminds you about the
 * other four.
 *
 * The failure that costs real time is not a mismatched number in a file. It is
 * an installed build that reports 0.2.0 while the manifest offers 0.2.0, so the
 * updater decides there is nothing to do — and a user sits on a broken version
 * forever, with every gate green.
 *
 * So: `package.json` at the repository root is the source of truth, and
 * everything else is either derived from it or checked against it.
 *
 *   node tools/version.mjs            # check. Non-zero on any drift.
 *   node tools/version.mjs --set 0.2.0
 *   node tools/version.mjs --against-tag v0.2.0
 *
 * `--against-tag` is for the release workflow: a tag that disagrees with the
 * tree would publish an installer whose version is not the version anybody
 * asked for.
 */
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");

/** The source of truth. Everything below is measured against this file. */
const SOURCE = "package.json";

/**
 * Files that carry a copy of the version, and how to read and write it.
 *
 * The workspace `package.json` files are private and never published, so their
 * version changes nothing at runtime — they are here because a reader who sees
 * `0.1.0` at the root and `0.3.0` in a package has to stop and work out which
 * one is lying, and `--set` keeps them aligned for free.
 */
const MIRRORS = [
  jsonVersion("apps/desktop/package.json"),
  jsonVersion("packages/types/package.json"),
  jsonVersion("packages/ui/package.json"),
  {
    path: "Cargo.toml",
    what: "[workspace.package] version",
    // Anchored to the [workspace.package] table. An unanchored match would hit
    // `serde = { version = "1.0.229" }` further down the file and rewrite a
    // dependency pin, which is a far worse outcome than the drift it was
    // fixing.
    read: (text) => matchCargo(text)?.[2] ?? null,
    write: (text, next) => {
      const m = matchCargo(text);
      if (!m)
        throw new Error("Cargo.toml has no version under [workspace.package].");
      return text.replace(m[0], `${m[1]}"${next}"`);
    },
  },
];

function jsonVersion(path) {
  return {
    path,
    what: "version",
    read: (text) => JSON.parse(text).version ?? null,
    write: (text, next) => {
      // Textual, not JSON.parse/stringify: rewriting the whole file would
      // reorder nothing but would drop the formatting Prettier just checked,
      // and the format gate would fail on a file this tool wrote.
      const m = /("version"\s*:\s*)"[^"]*"/.exec(text);
      if (!m) throw new Error(`${path} has no "version" key.`);
      return text.replace(m[0], `${m[1]}"${next}"`);
    },
  };
}

function matchCargo(text) {
  return /(\[workspace\.package\][\s\S]*?\bversion\s*=\s*)"([^"]*)"/.exec(text);
}

const read = (path) => readFileSync(join(ROOT, path), "utf8");
const write = (path, text) => writeFileSync(join(ROOT, path), text);

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

const args = process.argv.slice(2);
const setIndex = args.indexOf("--set");
const tagIndex = args.indexOf("--against-tag");

// --- tauri.conf.json must stay derived, not copied ------------------------
// Tauri reads `"version": "../package.json"` and takes the version from there.
// Someone replacing that with a literal would break the single source of truth
// in a way no version comparison can see, because the literal would be correct
// on the day it was written.
function checkTauriDerivation(problems) {
  const path = "apps/desktop/src-tauri/tauri.conf.json";
  const version = JSON.parse(read(path)).version;
  if (version !== "../package.json") {
    problems.push(
      `${path}: "version" is ${JSON.stringify(version)}, not "../package.json".\n` +
        "    Tauri can read the version straight from package.json. A literal here is a\n" +
        "    fifth copy that will be right today and wrong at the next release.",
    );
  }
}

// --- --set -----------------------------------------------------------------
if (setIndex !== -1) {
  const next = args[setIndex + 1];
  if (!next || !SEMVER.test(next)) {
    console.error(
      `version: "${next ?? ""}" is not a semver version (e.g. 0.2.0, 1.0.0-rc.1).`,
    );
    process.exit(1);
  }

  write(SOURCE, jsonVersion(SOURCE).write(read(SOURCE), next));
  console.log(`version: ${SOURCE} -> ${next}`);
  for (const m of MIRRORS) {
    write(m.path, m.write(read(m.path), next));
    console.log(`version: ${m.path} -> ${next}`);
  }

  // Cargo.lock records the workspace members' versions. Left stale, the next
  // `cargo build --locked` fails, and the error names the lockfile rather than
  // this change.
  try {
    execFileSync("cargo", ["metadata", "--format-version", "1", "--offline"], {
      cwd: ROOT,
      stdio: "ignore",
    });
    console.log("version: Cargo.lock refreshed");
  } catch {
    console.warn(
      "version: could not refresh Cargo.lock — run `cargo metadata` before committing.",
    );
  }

  console.log(
    "\nversion: commit every file above together. A partial bump is the drift this",
  );
  console.log(
    "version: tool exists to catch, and it would catch it on the next run.",
  );
  process.exit(0);
}

// --- check -----------------------------------------------------------------
const source = jsonVersion(SOURCE).read(read(SOURCE));
const problems = [];

if (!source || !SEMVER.test(source)) {
  problems.push(`${SOURCE}: version ${JSON.stringify(source)} is not semver.`);
}

for (const m of MIRRORS) {
  const found = m.read(read(m.path));
  if (found !== source) {
    problems.push(
      `${m.path}: ${m.what} is ${JSON.stringify(found)}, but ${SOURCE} says ${JSON.stringify(source)}.`,
    );
  }
}

checkTauriDerivation(problems);

if (tagIndex !== -1) {
  const tag = args[tagIndex + 1] ?? "";
  const tagged = tag.replace(/^refs\/tags\//, "").replace(/^v/, "");
  if (tagged !== source) {
    problems.push(
      `tag ${JSON.stringify(tag)} resolves to ${JSON.stringify(tagged)}, but ${SOURCE} says ${JSON.stringify(source)}.\n` +
        "    Releasing this would ship an installer whose version is not the one the tag\n" +
        "    names, and the updater compares versions, not tags.",
    );
  }
}

if (problems.length > 0) {
  console.error("");
  console.error(
    "version: the version is not consistent across the repository.",
  );
  console.error("");
  for (const p of problems) console.error("  " + p);
  console.error("");
  console.error(
    `Fix every copy at once:  node tools/version.mjs --set ${source ?? "<version>"}`,
  );
  console.error("");
  process.exit(1);
}

console.log(
  `version: ${source}, consistent across ${MIRRORS.length + 2} files.`,
);
