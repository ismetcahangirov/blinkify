#!/usr/bin/env node
/**
 * Relative-link checker for Blinkify Markdown.
 *
 * Walks every tracked `.md` file, extracts inline links, and fails if a
 * relative target does not exist on disk or a heading anchor does not resolve.
 *
 * Deliberately offline: external URLs are counted and skipped. A link checker
 * that makes network calls is a CI gate that fails for reasons unrelated to the
 * pull request, and a gate that fails randomly is a gate people learn to ignore.
 *
 * Usage:  node tools/check-links.mjs [root]
 * Exit:   0 clean, 1 on any broken link.
 */

import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import { join, dirname, resolve, relative, sep } from "node:path";

const ROOT = resolve(process.argv[2] ?? ".");
const SKIP_DIRS = new Set([
  "node_modules",
  ".git",
  "target",
  "dist",
  "build",
  ".turbo",
]);

/** Every `.md` file under `dir`, excluding build output. */
function markdownFiles(dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (entry.name.startsWith(".") && entry.name !== ".github") continue;
    if (SKIP_DIRS.has(entry.name)) continue;
    const full = join(dir, entry.name);
    if (entry.isDirectory()) found.push(...markdownFiles(full));
    else if (entry.name.endsWith(".md")) found.push(full);
  }
  return found.sort();
}

/**
 * GitHub's heading-to-anchor slug.
 *
 * The last step replaces each whitespace character individually rather than
 * each run of them. That looks like a detail and is not: GitHub does the same,
 * so a heading whose punctuation was stripped from between two spaces — "Zone 4
 * — timeline" — anchors as `zone-4--timeline` with two hyphens. Collapsing the
 * run produces `zone-4-timeline`, which this checker would then accept and
 * GitHub would render as a dead link. A gate that insists on the wrong answer
 * is worse than no gate, because the document gets changed to satisfy it.
 */
function slugify(heading) {
  return heading
    .trim()
    .toLowerCase()
    .replace(/`([^`]*)`/g, "$1")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[^\p{L}\p{N}\s-]/gu, "")
    .replace(/\s/g, "-");
}

/** Anchors a Markdown file exposes, from its ATX headings. */
function anchorsOf(file) {
  const anchors = new Set();
  const counts = new Map();
  let inFence = false;
  for (const line of readFileSync(file, "utf8").split(/\r?\n/)) {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const heading = /^#{1,6}\s+(.*)$/.exec(line);
    if (!heading) continue;
    const base = slugify(heading[1]);
    const seen = counts.get(base) ?? 0;
    counts.set(base, seen + 1);
    anchors.add(seen === 0 ? base : `${base}-${seen}`);
  }
  return anchors;
}

/** Inline links in a file, with the line they sit on, fenced code excluded. */
function linksOf(file) {
  const links = [];
  const lines = readFileSync(file, "utf8").split(/\r?\n/);
  let inFence = false;
  lines.forEach((line, index) => {
    if (/^\s*(```|~~~)/.test(line)) {
      inFence = !inFence;
      return;
    }
    if (inFence) return;
    const withoutCodeSpans = line.replace(/`[^`]*`/g, "");
    for (const match of withoutCodeSpans.matchAll(/\[[^\]]*\]\(([^)\s]+)\)/g)) {
      links.push({ target: match[1], line: index + 1 });
    }
  });
  return links;
}

const files = markdownFiles(ROOT);
const anchorCache = new Map();
const failures = [];
let checked = 0;
let external = 0;

for (const file of files) {
  for (const { target, line } of linksOf(file)) {
    if (/^(https?:|mailto:|#)/.test(target)) {
      if (target.startsWith("#")) {
        checked += 1;
        if (!anchorCache.has(file)) anchorCache.set(file, anchorsOf(file));
        if (!anchorCache.get(file).has(decodeURIComponent(target.slice(1)))) {
          failures.push({ file, line, target, why: "no such heading" });
        }
      } else {
        external += 1;
      }
      continue;
    }

    checked += 1;
    const [path, anchor] = target.split("#");
    const resolved = resolve(dirname(file), decodeURIComponent(path));

    if (!existsSync(resolved)) {
      failures.push({ file, line, target, why: "target does not exist" });
      continue;
    }
    if (!anchor) continue;

    const asFile = statSync(resolved).isDirectory()
      ? join(resolved, "README.md")
      : resolved;
    if (!existsSync(asFile)) {
      failures.push({ file, line, target, why: "no README.md to anchor into" });
      continue;
    }
    if (!anchorCache.has(asFile)) anchorCache.set(asFile, anchorsOf(asFile));
    if (!anchorCache.get(asFile).has(decodeURIComponent(anchor))) {
      failures.push({ file, line, target, why: "no such heading" });
    }
  }
}

const show = (file) => relative(ROOT, file).split(sep).join("/");

if (failures.length > 0) {
  console.error(`\nBroken links (${failures.length}):\n`);
  for (const f of failures) {
    console.error(`  ${show(f.file)}:${f.line}  ->  ${f.target}  (${f.why})`);
  }
  console.error(
    `\n${files.length} files, ${checked} local links checked, ${external} external skipped.\n`,
  );
  process.exit(1);
}

console.log(
  `Links OK — ${files.length} files, ${checked} local links checked, ${external} external skipped.`,
);
