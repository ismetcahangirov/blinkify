#!/usr/bin/env node
/**
 * Blinkify project graph generator.
 *
 * Purpose (CLAUDE.md section 14): answer "what breaks if I change this file?"
 * without reading the whole repository.
 *
 * Emits into tools/project-graph/output/:
 *   graph.json  — the workspace dependency graph, full edge metadata
 *   index.json  — condensed forward/reverse index, file to test mapping
 *   GRAPH.md    — human- and agent-readable summary
 *
 * TypeScript only. The Rust crate graph is a separate tool
 * (tools/crate-boundaries.mjs) because it is a separate language with a
 * separate resolver, and pretending otherwise produces a gate that proves less
 * than it claims.
 *
 * Run: pnpm graph
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const OUT = join(HERE, "output");

const DEPCRUISE_BIN = join(
  ROOT,
  "node_modules",
  "dependency-cruiser",
  "bin",
  "dependency-cruiser.mjs",
);

const SCAN_TARGETS = ["apps", "packages", "tools"].filter((d) =>
  existsSync(join(ROOT, d)),
);

if (SCAN_TARGETS.length === 0) {
  console.error(
    "project-graph: nothing to scan (no apps/, packages/ or tools/).",
  );
  process.exit(1);
}

function isTest(p) {
  return /(\.(test|spec)\.[cm]?[jt]sx?$)|(^|\/)(__tests__|test|e2e)\//.test(p);
}

/**
 * Workspace a file belongs to, e.g. apps/desktop or packages/types.
 *
 * `tools/` is one bucket rather than one per subdirectory: it is not a pnpm
 * workspace, it is a pile of CLI entry points, and splitting it would report
 * `tools/apply-labels.mjs` and `tools/project-graph/query.mjs` as belonging to
 * different things when they belong to the same thing.
 */
function workspaceOf(p) {
  const m = /^((?:apps|packages)\/[^/]+)\//.exec(p);
  if (m) return m[1];
  if (p.startsWith("tools/")) return "tools";
  return "(root)";
}

/**
 * Layer in the CLAUDE.md section 2 diagram, which is the unit the boundary
 * rules reason about. The renderer and the Tauri shell live in the same
 * workspace but are different layers, and the blast-radius report is far more
 * useful when it says which one moved.
 */
function layerOf(p) {
  if (/^apps\/desktop\/src\//.test(p)) return "renderer";
  if (/^apps\/desktop\/src-tauri\//.test(p)) return "tauri-shell";
  if (/^packages\/types\//.test(p)) return "ipc-contract";
  if (/^packages\/ui\//.test(p)) return "design-system";
  if (/^tools\//.test(p)) return "tooling";
  return workspaceOf(p);
}

console.log(`project-graph: scanning ${SCAN_TARGETS.join(", ")} ...`);

let raw;
try {
  raw = execFileSync(
    process.execPath,
    [
      DEPCRUISE_BIN,
      "--config",
      ".dependency-cruiser.cjs",
      "--output-type",
      "json",
      ...SCAN_TARGETS,
    ],
    { cwd: ROOT, encoding: "utf8", maxBuffer: 128 * 1024 * 1024 },
  );
} catch (error) {
  // depcruise exits non-zero when a *rule* is violated, but still prints JSON.
  if (!error.stdout) {
    console.error(
      "project-graph: dependency-cruiser failed to run.\n",
      error.message,
    );
    process.exit(1);
  }
  raw = error.stdout;
}

const cruise = JSON.parse(raw);
const modules = cruise.modules ?? [];

mkdirSync(OUT, { recursive: true });

const isLocal = (p) => !p.includes("node_modules");

/**
 * Code-unit ordering, deliberately not the locale-aware comparator.
 *
 * Every sort here feeds a committed artifact whose whole value is the claim in
 * CLAUDE.md section 14 — that it is a pure function of the source tree, so CI
 * can regenerate it and fail on any diff. The locale-aware comparator breaks
 * that claim: it is ICU-backed, it weights punctuation differently from a
 * code-unit comparison, and those weights differ between ICU builds. A graph
 * sorted on Windows and regenerated on a Linux runner comes out in a different
 * order, and because ONLY the order changes both files are byte-for-byte the
 * same size — git reports them as binary and CI prints "0 insertions(+), 0
 * deletions(-)", which reads as a broken gate rather than as a stale graph.
 *
 * Human-friendly ordering is not worth a host-dependent artifact. If this ever
 * needs to read more naturally, sort at the point of display — never here.
 */
const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

const local = modules
  .filter((m) => !m.coreModule && isLocal(m.source))
  .sort((a, b) => byCodeUnit(a.source, b.source));

// --- graph.json ------------------------------------------------------------
// The committed graph must be a pure function of the source tree, because CI
// regenerates it and fails on a non-empty diff. Raw cruise output is not:
//
//   * `summary.environment` carries the OS string and the Node version;
//   * `summary.optionsUsed.baseDir` is an absolute path;
//   * the node_modules leaves differ by platform — optional native packages,
//     case-sensitive resolution, and symlink realpaths all vary between a
//     contributor's laptop and the CI runner.
//
// So graph.json records the workspace graph only, sorted, with edge metadata
// (`dependencyTypes`, `dynamic`, `circular`, …) preserved. The npm edges still
// reach the rule engine during `graph:validate`, which is where they are
// enforced; they are simply not part of the committed artifact, because their
// content is a property of the machine.
const portableModules = local.map((m) => ({
  ...m,
  dependencies: (m.dependencies ?? [])
    .filter((d) => !d.coreModule && isLocal(d.resolved))
    .sort((a, b) => byCodeUnit(a.resolved, b.resolved)),
  dependents: (m.dependents ?? []).filter(isLocal).sort(byCodeUnit),
}));

const graph = {
  ...cruise,
  modules: portableModules,
  summary: {
    ...cruise.summary,
    // The raw totals count the node_modules leaves, whose number depends on
    // the platform. Report the totals for what this file actually contains.
    totalCruised: portableModules.length,
    totalDependenciesCruised: portableModules.reduce(
      (n, m) => n + m.dependencies.length,
      0,
    ),
    environment: undefined,
    optionsUsed: { ...cruise.summary?.optionsUsed, baseDir: undefined },
  },
};

const graphJson = JSON.stringify(graph, null, 2);

// Fail loudly rather than committing an artifact that will make every pull
// request's drift check fail for a reason nobody can see in a diff. Only path
// values are checked: the literal string "node_modules" is expected inside
// `optionsUsed.doNotFollow`, where it is configuration rather than a path.
const pathsEmitted = portableModules.flatMap((m) => [
  m.source,
  ...m.dependencies.map((d) => d.resolved),
  ...m.dependents,
]);

for (const [what, offends] of [
  ["a node_modules path", (p) => p.includes("node_modules")],
  ["an absolute path", (p) => /^([A-Za-z]:|\/)/.test(p)],
  ["a Windows path separator", (p) => p.includes("\\")],
]) {
  const bad = pathsEmitted.find(offends);
  if (bad !== undefined) {
    console.error(
      `project-graph: refusing to write graph.json — ${bad} is ${what}.`,
    );
    console.error(
      "project-graph: the committed graph must not depend on the machine.",
    );
    process.exit(1);
  }
}

writeFileSync(join(OUT, "graph.json"), graphJson);

// --- Condensed index -------------------------------------------------------

const dependsOn = new Map(); // file -> [files it imports]
const dependedOnBy = new Map(); // file -> [files that import it]

for (const m of local) {
  // Keep local edges only: core modules and node_modules are noise for a
  // blast-radius report. Dynamic imports are kept — they are still couplings.
  const deps = (m.dependencies ?? [])
    .filter((d) => !d.coreModule && !d.resolved.includes("node_modules"))
    .map((d) => d.resolved)
    .sort(byCodeUnit);
  dependsOn.set(m.source, deps);
  for (const d of deps) {
    if (!dependedOnBy.has(d)) dependedOnBy.set(d, []);
    dependedOnBy.get(d).push(m.source);
  }
}

const files = {};
for (const m of local) {
  const src = m.source;
  const reverse = (dependedOnBy.get(src) ?? []).slice().sort(byCodeUnit);
  files[src] = {
    workspace: workspaceOf(src),
    layer: layerOf(src),
    isTest: isTest(src),
    dependsOn: dependsOn.get(src) ?? [],
    dependedOnBy: reverse,
    // Which test files (one hop) exercise this file.
    coveredByTests: reverse.filter(isTest),
    // Derived from the graph rather than from a dependency-cruiser `orphan`
    // rule, on purpose. An entry point has no dependents and never will — a
    // rule that flags it produces a permanent violation in the committed
    // graph, and a gate that is always red is a gate nobody reads. This is
    // reported, not enforced.
    hasNoDependents: reverse.length === 0,
  };
}

const unsortedByWorkspace = {};
for (const [src, info] of Object.entries(files)) {
  unsortedByWorkspace[info.workspace] ??= { files: 0, tests: 0, untested: [] };
  unsortedByWorkspace[info.workspace].files += 1;
  if (info.isTest) unsortedByWorkspace[info.workspace].tests += 1;
  else if (info.coveredByTests.length === 0)
    unsortedByWorkspace[info.workspace].untested.push(src);
}

const byWorkspace = Object.fromEntries(
  Object.entries(unsortedByWorkspace)
    .sort(([a], [b]) => byCodeUnit(a, b))
    .map(([ws, s]) => [
      ws,
      { ...s, untested: s.untested.slice().sort(byCodeUnit) },
    ]),
);

const violations = (cruise.summary?.violations ?? [])
  .map((v) => ({
    rule: v.rule.name,
    severity: v.rule.severity,
    from: v.from,
    to: v.to,
  }))
  .sort((a, b) =>
    byCodeUnit(`${a.rule}${a.from}${a.to}`, `${b.rule}${b.from}${b.to}`),
  );

// No timestamp, and every collection is sorted. The output is committed, so it
// must be a pure function of the source tree: CI regenerates it and fails on a
// non-empty `git diff`. A clock reading here would make that check fire on
// every run and therefore mean nothing.
const index = {
  scanned: SCAN_TARGETS,
  totals: {
    files: local.length,
    tests: local.filter((m) => isTest(m.source)).length,
    violations: violations.length,
  },
  byWorkspace,
  violations,
  files,
};

writeFileSync(join(OUT, "index.json"), JSON.stringify(index, null, 2));

// --- Markdown summary ------------------------------------------------------
const wsRows = Object.entries(byWorkspace)
  .sort(([a], [b]) => byCodeUnit(a, b))
  .map(
    ([ws, s]) =>
      `| \`${ws}\` | ${s.files} | ${s.tests} | ${s.untested.length} |`,
  )
  .join("\n");

const md = `# Blinkify project graph

<!-- GENERATED FILE — do not edit. Run \`pnpm graph\` to regenerate. -->

Scanned: ${SCAN_TARGETS.join(", ")}

TypeScript only. The Rust crate boundaries are checked by
\`pnpm boundaries:rust\` and the Rust licences by \`cargo deny check licenses\`.

## Totals

- Files: **${index.totals.files}**
- Test files: **${index.totals.tests}**
- Architecture rule violations: **${index.totals.violations}**

## Workspaces

| Workspace | Files | Tests | Files with no direct test |
| --- | ---: | ---: | ---: |
${wsRows || "| _(empty — no source yet)_ | 0 | 0 | 0 |"}

## Architecture rule violations

${
  violations.length === 0
    ? "_None._"
    : violations
        .map(
          (v) => `- **${v.rule}** (${v.severity}): \`${v.from}\` → \`${v.to}\``,
        )
        .join("\n")
}

## How to query this graph

\`\`\`bash
pnpm graph                                  # regenerate
node tools/project-graph/query.mjs <path>   # impact of changing a file
\`\`\`

\`output/index.json\` is the machine-readable source of truth. For any file it
records \`dependsOn\`, \`dependedOnBy\`, \`coveredByTests\`, and its owning
workspace and layer — read that instead of crawling the repository.
`;

writeFileSync(join(OUT, "GRAPH.md"), md);

console.log(
  `project-graph: ${index.totals.files} files, ${index.totals.tests} tests, ${index.totals.violations} violations`,
);
console.log(
  `project-graph: wrote ${relative(ROOT, OUT)}/{graph.json,index.json,GRAPH.md}`,
);
