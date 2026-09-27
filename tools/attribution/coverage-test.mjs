#!/usr/bin/env node
/**
 * Proof that the attribution document covers what it claims to (#73).
 *
 * Four assertions, each against a real input rather than a fixture:
 *
 * 1. **Every crate `cargo metadata` names is in the committed document.**
 *    Computed here with a separate `cargo metadata` call rather than the
 *    generator's own collection, so a bug in that collection cannot hide
 *    behind itself.
 * 2. **A crate whose licence text is not in its metadata is covered.**
 *    `serde` declares `MIT OR Apache-2.0` and no `license-file`: the text
 *    exists only in the `.crate` source the local registry unpacked. The
 *    document must carry that file's text, byte for byte after
 *    normalisation, against `serde`.
 * 3. **A crate that ships no licence file at all is covered**, with the
 *    standard text and a statement that it is one. `ts-rs` ships none.
 * 4. **The tool fails rather than skips.** A crate and a renderer package
 *    whose licence cannot be attributed each make generation fail, naming
 *    the package, and no document is written.
 *
 * Run: pnpm attribution:coverage
 */
import { execFileSync, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { DOCUMENT, ROOT, TARGET, normalise } from "./attribution.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const document = readFileSync(DOCUMENT, "utf8");

let failed = 0;
function expect(what, ok, detail = "") {
  if (ok) {
    console.log(`  ok   ${what}`);
    return;
  }
  failed += 1;
  console.error(`  FAIL ${what}`);
  if (detail)
    console.error(`       ${detail.trim().split(/\r?\n/).join("\n       ")}`);
}

/** Part 2 entries by their heading line, with the text references each cites. */
function entries() {
  const found = new Map();
  const lines = document.split("\n");
  lines.forEach((line, index) => {
    if (!/ \((Rust crate|npm package)\)$/.test(line)) return;
    const block = [];
    for (let i = index + 1; i < lines.length && lines[i] !== ""; i += 1)
      block.push(lines[i]);
    found.set(line, block.join("\n"));
  });
  return found;
}

/** Part 3 texts by reference. */
function texts() {
  const found = new Map();
  const parts = document.split(
    /^-{78}\n\[text ([0-9a-f]{10})\][^\n]*\n[\s\S]*?^-{78}\n\n/m,
  );
  for (let i = 1; i < parts.length; i += 2) {
    // A text runs until the next text's rule, which split() has consumed;
    // the blank line before that rule is layout, not text.
    found.set(parts[i], normalise(parts[i + 1] ?? ""));
  }
  return found;
}

const byHeading = entries();
const byReference = texts();
const refs = (entry) =>
  [...(entry ?? "").matchAll(/\[text ([0-9a-f]{10})\]/g)].map((m) => m[1]);

// 1. Every crate in cargo metadata.
const metadata = JSON.parse(
  execFileSync(
    "cargo",
    [
      "metadata",
      "--format-version",
      "1",
      "--all-features",
      "--locked",
      "--filter-platform",
      TARGET,
    ],
    { cwd: ROOT, encoding: "utf8", maxBuffer: 512 * 1024 * 1024 },
  ),
);
const members = new Set(metadata.workspace_members);
const crates = metadata.packages.filter((pkg) => !members.has(pkg.id));
const missing = crates
  .map((pkg) => `${pkg.name} ${pkg.version} (Rust crate)`)
  .filter((heading) => !byHeading.has(heading));
expect(
  `every one of the ${crates.length} crates cargo metadata resolves is in the document`,
  missing.length === 0,
  `missing: ${missing.join(", ")}`,
);

// 2. serde: the text is in the .crate source, not in the metadata.
const serde = crates.find((pkg) => pkg.name === "serde");
if (!serde) {
  expect("serde is in the tree", false);
} else {
  const heading = `${serde.name} ${serde.version} (Rust crate)`;
  const file = join(dirname(serde.manifest_path), "LICENSE-MIT");
  const own = normalise(readFileSync(file, "utf8"));
  expect(
    "serde declares no licence-file, so its text is only in the unpacked .crate",
    serde.license_file === null && !serde.license.includes("\n"),
    `license_file: ${serde.license_file}`,
  );
  const cited = refs(byHeading.get(heading)).map((id) => byReference.get(id));
  expect(
    `${heading} cites the LICENSE-MIT text from its own source, verbatim`,
    cited.includes(own),
    `cited ${cited.length} text(s); none equals ${file}`,
  );
  expect(
    `${heading} records the choice it was taken under`,
    /Taken under: MIT$/m.test(byHeading.get(heading) ?? ""),
    byHeading.get(heading) ?? "(no entry)",
  );
}

// 3. ts-rs: no licence file at all.
const tsRs = crates.find((pkg) => pkg.name === "ts-rs");
if (!tsRs) {
  expect("ts-rs is in the tree", false);
} else {
  const heading = `${tsRs.name} ${tsRs.version} (Rust crate)`;
  const entry = byHeading.get(heading) ?? "";
  const cited = refs(entry).map((id) => byReference.get(id) ?? "");
  expect(
    `${heading} ships no licence file and is covered by the standard MIT text, marked as such`,
    /standard text; the package ships none/.test(entry) &&
      cited.some((text) =>
        /Permission is hereby granted, free of charge/.test(text),
      ),
    entry || "(no entry)",
  );
}

// 4. Fail, never skip.
const temp = mkdtempSync(join(tmpdir(), "blinkify-attribution-coverage-"));
try {
  const workspace = join(temp, "workspace");
  const orphan = join(temp, "orphan");
  mkdirSync(join(workspace, "canary", "src"), { recursive: true });
  mkdirSync(join(orphan, "src"), { recursive: true });
  writeFileSync(
    join(workspace, "Cargo.toml"),
    '[workspace]\nmembers = ["canary"]\nresolver = "2"\n',
  );
  writeFileSync(
    join(workspace, "canary", "Cargo.toml"),
    '[package]\nname = "blinkify-attribution-canary"\nversion = "0.0.0"\nedition = "2021"\nlicense = "MIT"\npublish = false\n\n[dependencies]\nblinkify-unattributable = { path = "../../orphan" }\n',
  );
  writeFileSync(join(workspace, "canary", "src", "lib.rs"), "\n");
  // Outside the workspace directory, so cargo treats it as a dependency and
  // not a member. A licence with no standard text and no file beside it.
  writeFileSync(
    join(orphan, "Cargo.toml"),
    '[package]\nname = "blinkify-unattributable"\nversion = "0.0.0"\nedition = "2021"\nlicense = "LicenseRef-Blinkify-Unattributable"\npublish = false\n',
  );
  writeFileSync(join(orphan, "src", "lib.rs"), "\n");
  execFileSync(
    "cargo",
    [
      "generate-lockfile",
      "--offline",
      "--manifest-path",
      join(workspace, "Cargo.toml"),
    ],
    {
      stdio: "ignore",
    },
  );

  const renderer = join(temp, "renderer");
  mkdirSync(join(renderer, "node_modules", "blinkify-unlicensed"), {
    recursive: true,
  });
  writeFileSync(
    join(renderer, "package.json"),
    JSON.stringify({
      name: "canary",
      version: "0.0.0",
      dependencies: { "blinkify-unlicensed": "*" },
    }),
  );
  writeFileSync(
    join(renderer, "node_modules", "blinkify-unlicensed", "package.json"),
    JSON.stringify({ name: "blinkify-unlicensed", version: "1.0.0" }),
  );

  const output = join(temp, "NOTICES.txt");
  const result = spawnSync(
    process.execPath,
    [
      join(HERE, "generate.mjs"),
      "--manifest-path",
      join(workspace, "Cargo.toml"),
      "--npm-root",
      renderer,
      "--components",
      "none",
      "--document",
      output,
      "--offline",
    ],
    { encoding: "utf8" },
  );
  const said = `${result.stdout}${result.stderr}`;
  expect(
    "a crate whose licence cannot be attributed fails generation, naming it",
    result.status !== 0 &&
      /blinkify-unattributable 0\.0\.0 \(Rust crate\)/.test(said),
    said,
  );
  expect(
    "a renderer package with no licence fails generation, naming it",
    result.status !== 0 &&
      /blinkify-unlicensed 1\.0\.0 \(npm package\)/.test(said),
    said,
  );
  expect("no document is written when attribution fails", !existsSync(output));
} finally {
  rmSync(temp, { recursive: true, force: true });
}

if (failed > 0) {
  console.error(`\nattribution:coverage: ${failed} case(s) failed.`);
  process.exit(1);
}
console.log(
  "\nattribution:coverage: every crate is covered, texts come from the packages' own sources, and the tool fails rather than skips.",
);
