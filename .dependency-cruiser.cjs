/**
 * Blinkify architecture rules and dependency graph configuration.
 *
 * Two jobs:
 *   1. `pnpm graph`          — emit the machine-readable graph (tools/project-graph).
 *   2. `pnpm graph:validate` — fail CI when an architectural boundary is crossed.
 *
 * The rules here encode the layering in CLAUDE.md section 2. This file covers
 * the TypeScript side only. The Rust side is covered by
 * `tools/crate-boundaries.mjs` and `cargo deny`; CLAUDE.md section 14 is
 * explicit that one tool does not cover both languages, so do not add a rule
 * here that pretends to.
 */

/** @type {import('dependency-cruiser').IConfiguration} */
module.exports = {
  forbidden: [
    {
      name: "no-circular",
      severity: "error",
      comment:
        "Circular dependencies make modules impossible to reason about or test in isolation.",
      from: {},
      to: { circular: true },
    },
    {
      name: "no-deprecated-core",
      severity: "error",
      comment:
        "Deprecated Node core modules will be removed in a future runtime.",
      from: {},
      to: {
        dependencyTypes: ["core"],
        path: ["^(punycode|domain|sys|constants)$"],
      },
    },
    {
      name: "not-to-dev-dep",
      severity: "error",
      comment:
        "Shipped code must not import a devDependency — it will be absent from the bundle " +
        "the installer carries.",
      from: {
        path: "^(apps|packages)",
        // Tests and build/tooling config are tooling: they never reach the
        // renderer bundle, so importing a devDependency from them is correct.
        pathNot:
          "\\.(test|spec)\\.(ts|tsx)$|/__tests__/|(^|/)(eslint|vite|vitest)\\.config\\.(ts|mts|mjs)$",
      },
      to: { dependencyTypes: ["npm-dev"] },
    },
    {
      name: "no-non-package-json",
      severity: "error",
      comment:
        "Dependency used but not declared in package.json — breaks on a clean install.",
      from: {},
      to: {
        dependencyTypes: [
          "unknown",
          "undetermined",
          "npm-no-pkg",
          "npm-unknown",
        ],
        // A workspace package resolves to a path inside the repo. pnpm links
        // those through a symlink and a subpath export, which dependency-cruiser
        // reports as `undetermined` even though package.json declares the
        // dependency. Cross-workspace edges are governed by the boundary rules
        // below, so excluding them here loses no coverage.
        //
        // `crates/` is deliberately absent from this list. A renderer file that
        // names an engine crate as a bare specifier — `@blinkify/engine`, say —
        // resolves to nothing, and this is the rule that catches it;
        // `renderer-not-into-engine` only matches once the path resolves.
        pathNot: "^(apps|packages|tools)/",
      },
    },

    // --- Blinkify boundaries -----------------------------------------------
    {
      name: "renderer-not-into-engine",
      severity: "error",
      comment:
        "CLAUDE.md section 2: the renderer never reaches the engine directly. It calls Tauri " +
        "commands and consumes Tauri events, both typed by packages/types, which is generated " +
        "from the Rust definitions. A renderer file importing from crates/ or src-tauri/ either " +
        "will not build or is a second, hand-maintained copy of the IPC contract — and a " +
        "drifting IPC contract is the bug class packages/types exists to design out.",
      from: { path: "^apps/desktop/src/" },
      to: { path: "^(crates/|apps/desktop/src-tauri/)" },
    },
    {
      name: "ui-package-stays-shared",
      severity: "error",
      comment:
        "packages/* are leaf libraries. A design system that reaches back into an application " +
        "is not a design system — it is that application with extra steps, and it cannot be " +
        "used anywhere else. Named for packages/ui, which CLAUDE.md section 4 calls out by " +
        "name; scoped to every shared package because the reasoning does not stop at ui.",
      from: { path: "^packages/" },
      to: { path: "^apps/" },
    },
    {
      name: "engine-contract-is-generated",
      severity: "error",
      comment:
        "packages/types is generated from the Rust types by `pnpm types:generate`. Nothing in " +
        "it may import application or design-system code: the contract has to be readable by " +
        "anything that speaks to the engine, and an import out of it inverts that.",
      from: { path: "^packages/types/" },
      to: { path: "^(apps/|packages/ui/)" },
    },
  ],

  options: {
    // `doNotFollow` records the edge into node_modules and stops there: the
    // dependency is typed (npm / npm-dev / npm-no-pkg) without crawling the
    // package's own tree.
    //
    // node_modules is deliberately NOT in `exclude`, and there is deliberately
    // no `includeOnly`. Either one drops every npm edge before the rule engine
    // sees it, which silently disables `not-to-dev-dep`, `no-non-package-json`
    // and `no-deprecated-core` — and the gate still passes, so nobody finds
    // out. CLAUDE.md section 14 requires an injection test for any change under
    // this key; `pnpm graph:injection` is that test.
    // tools/project-graph/generate.mjs filters node_modules out of the
    // condensed index, so the blast-radius report stays workspace-only.
    doNotFollow: { path: ["node_modules"] },
    // Every exclude is anchored to the workspace tree. An unanchored pattern
    // such as `(^|/)dist/` also matches `node_modules/vite/dist/index.js`,
    // which drops that npm edge before `not-to-dev-dep` can see it.
    exclude: {
      path: [
        "^(apps|packages|tools)/.*/(dist|build|coverage)/",
        "^(apps|packages|tools)/.*/\\.turbo/",
        // Tauri writes these on every build and .gitignore covers them, so
        // including them would make the committed graph a function of whether
        // the contributor has ever run the app rather than of the source tree.
        "^apps/desktop/src-tauri/(gen|target)/",
        "^tools/project-graph/output/",
      ],
    },
    // Follow `import type` edges too — a type-only import is still a coupling
    // that the blast-radius report must show, and `renderer-not-into-engine`
    // is above all a rule about types.
    tsPreCompilationDeps: true,
    // Deliberately no `tsConfig` here. tsconfig.base.json is a base to extend,
    // not a buildable project, and pointing tsc at it fails with TS18003.
    // Module resolution goes through node + pnpm workspace links, which is how
    // the renderer actually resolves `@blinkify/*`.
    enhancedResolveOptions: {
      exportsFields: ["exports"],
      conditionNames: ["import", "require", "node", "default", "types"],
      extensions: [
        ".js",
        ".jsx",
        ".ts",
        ".tsx",
        ".mjs",
        ".cjs",
        ".json",
        ".d.ts",
      ],
      mainFields: ["module", "main", "types", "typings"],
    },
    reporterOptions: {
      dot: { collapsePattern: "node_modules/(?:@[^/]+/[^/]+|[^/]+)" },
      archi: {
        collapsePattern:
          "^(?:packages|apps|tools)/[^/]+|node_modules/(?:@[^/]+/[^/]+|[^/]+)",
      },
    },
  },
};
