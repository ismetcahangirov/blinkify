#!/usr/bin/env node
/**
 * Apply `.github/labels.yml` to the repository, or check GitHub against it.
 *
 * `labels.yml` is only the source of truth if something reconciles it. Without
 * a command, it is a file that describes what the labels used to be.
 *
 *   node tools/apply-labels.mjs           apply (idempotent — re-running is a no-op)
 *   node tools/apply-labels.mjs --check   fail if GitHub has drifted; change nothing
 *
 * Applying runs, once per entry:
 *   gh label create "<name>" --color "<color>" --description "<desc>" --force
 *
 * Requires the `gh` CLI, authenticated. Deliberately no YAML dependency: the
 * file's shape is fixed and forty lines of parser is cheaper than a dependency
 * that ships forever (CLAUDE.md section 10, rule 3).
 */

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const LABELS_FILE = join(HERE, "..", ".github", "labels.yml");
const CHECK_ONLY = process.argv.includes("--check");

/**
 * Parse the fixed `- name: / color: / description:` shape in labels.yml.
 * Anything else in the file is a syntax error here, on purpose — a silently
 * ignored line is a label that silently stops being managed.
 */
function parseLabels(text) {
  const labels = [];
  let current = null;
  text.split(/\r?\n/).forEach((raw, index) => {
    const line = raw.replace(/\s+$/, "");
    if (line === "" || line.trimStart().startsWith("#")) return;

    const quoted = (value) => {
      const m = /^"((?:[^"\\]|\\.)*)"$/.exec(value);
      if (!m) throw new Error(`labels.yml:${index + 1} value must be quoted`);
      return m[1].replace(/\\"/g, '"').replace(/\\\\/g, "\\");
    };

    let m;
    if ((m = /^- name:\s*(.+)$/.exec(line))) {
      current = { name: quoted(m[1]) };
      labels.push(current);
    } else if ((m = /^ {2}color:\s*(.+)$/.exec(line))) {
      if (!current)
        throw new Error(`labels.yml:${index + 1} color before name`);
      current.color = quoted(m[1]).toUpperCase();
    } else if ((m = /^ {2}description:\s*(.+)$/.exec(line))) {
      if (!current) throw new Error(`labels.yml:${index + 1} desc before name`);
      current.description = quoted(m[1]);
    } else {
      throw new Error(`labels.yml:${index + 1} unrecognised line: ${line}`);
    }
  });

  for (const label of labels) {
    if (label.color === undefined || label.description === undefined) {
      throw new Error(
        `labels.yml: "${label.name}" is missing color or description`,
      );
    }
  }
  return labels;
}

function gh(args) {
  return execFileSync("gh", args, {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

const wanted = parseLabels(readFileSync(LABELS_FILE, "utf8"));
const live = JSON.parse(
  gh(["label", "list", "--limit", "200", "--json", "name,color,description"]),
);
const liveByName = new Map(
  live.map((l) => [
    l.name,
    { color: l.color.toUpperCase(), description: l.description ?? "" },
  ]),
);

const missing = [];
const differing = [];
for (const label of wanted) {
  const current = liveByName.get(label.name);
  if (!current) missing.push(label);
  else if (
    current.color !== label.color ||
    current.description !== label.description
  ) {
    differing.push({ label, current });
  }
}
const unmanaged = live
  .map((l) => l.name)
  .filter((name) => !wanted.some((l) => l.name === name))
  .sort();

if (CHECK_ONLY) {
  const problems = missing.length + differing.length + unmanaged.length;
  for (const l of missing) console.error(`missing on GitHub:  ${l.name}`);
  for (const { label, current } of differing) {
    console.error(
      `differs:             ${label.name}  (GitHub: ${current.color} "${current.description}")`,
    );
  }
  for (const name of unmanaged)
    console.error(`on GitHub, not in labels.yml:  ${name}`);
  if (problems > 0) {
    console.error(
      `\n${problems} label(s) out of sync. Run \`node tools/apply-labels.mjs\`, or add the` +
        ` unmanaged ones to .github/labels.yml.\n`,
    );
    process.exit(1);
  }
  console.log(`Labels in sync — ${wanted.length} managed, none unmanaged.`);
  process.exit(0);
}

let applied = 0;
for (const label of wanted) {
  gh([
    "label",
    "create",
    label.name,
    "--color",
    label.color,
    "--description",
    label.description,
    "--force",
  ]);
  applied += 1;
}

console.log(`Applied ${applied} label(s) from .github/labels.yml.`);
if (unmanaged.length > 0) {
  console.log(
    `\nNot managed by labels.yml (left untouched): ${unmanaged.join(", ")}\n` +
      `Add them to .github/labels.yml or delete them — an unmanaged label is how the set rots.`,
  );
}
