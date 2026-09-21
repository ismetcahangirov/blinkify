---
name: project-graph-query
description: Use BEFORE editing existing Blinkify code to find what depends on it, what it depends on, and which tests cover it — and after structural changes to regenerate the graph. Triggers on "what uses this", "blast radius", "what will break", "refactor", "rename", "delete", or modifying any shared code, any Tauri command, or anything in packages/types.
---

# Query the project graph before you edit

CLAUDE.md section 7 step 3 requires establishing the blast radius before
touching existing code. The graph answers that in one command instead of a
repository-wide grep that misses the indirect dependants.

## Impact report for a file

```bash
node tools/project-graph/query.mjs apps/desktop/src/App.tsx
```

```
=== apps/desktop/src/App.tsx ===
workspace: apps/desktop
layer:     renderer                  ← which CLAUDE.md §2 layer this sits in

Depends on (1):                      ← what this file needs
Depended on by (blast radius) (2):   ← what breaks if you change it
Covered by tests (1):                ← run these after editing
Affected layers (1):                 ← what else to re-verify
```

A substring works too:

```bash
node tools/project-graph/query.mjs shell.store
```

## Find files with no test

```bash
node tools/project-graph/query.mjs --untested packages
```

**Know what this means.** It shows which files no test file _imports_ — not
which lines are exercised. Use it to find files with **no test at all**; use a
coverage report for anything finer.

## Regenerate

```bash
pnpm graph        # regenerate output/
pnpm graph:check  # regenerate and fail if the committed output moved
```

Required after a feature, a refactor, a new module, a dependency change, or an
architecture change (CLAUDE.md section 14).

`graph:check` is a CI gate. It is meaningful because the generator reads **no
clock** and sorts every collection, so two runs on the same tree are
byte-identical and a non-empty diff means the source tree actually moved.
`pnpm graph:determinism` is the proof of that, and it runs in CI. Do not
reintroduce a timestamp or an unordered collection — it would make every run
produce a diff, and a signal that always fires is a signal nobody reads.

## Two languages, two tools

CLAUDE.md section 14 is explicit about this and it is the thing people get
wrong. dependency-cruiser reads TypeScript. It knows nothing about `crates/`.

| Gate                   | Covers                                     |
| ---------------------- | ------------------------------------------ |
| `pnpm graph:validate`  | TypeScript boundaries (dependency-cruiser) |
| `pnpm boundaries:rust` | Engine crates must not reach `tauri`       |
| `pnpm deny:licenses`   | Licence of every Rust crate (ADR-0002)     |

A green `graph:validate` says **nothing** about the engine. Run all three.

## Enforce the boundaries

```bash
pnpm graph:validate
```

Rules in `.dependency-cruiser.cjs`:

| Rule                           |                                                                                         |
| ------------------------------ | --------------------------------------------------------------------------------------- |
| `no-circular`                  | Anywhere                                                                                |
| `not-to-dev-dep`               | A devDependency is absent from the bundle the installer carries                         |
| `no-non-package-json`          | Phantom dependencies — which a hoisted node_modules otherwise permits                   |
| `no-deprecated-core`           | A deprecated Node core module disappears in a future runtime                            |
| `renderer-not-into-engine`     | `apps/desktop/src/**` may not reach `crates/` or `src-tauri/` — go via `packages/types` |
| `ui-package-stays-shared`      | `packages/*` importing `apps/*` inverts the dependency direction                        |
| `engine-contract-is-generated` | `packages/types` may not import an app or the design system                             |

**Add a rule whenever you establish a boundary.** One entry now is cheaper than
a refactor later — and add its case to `tools/project-graph/injection-test.mjs`
in the same change, or you have added a rule nobody has seen fail.

### The npm rules are easy to silence by accident

`not-to-dev-dep`, `no-non-package-json` and `no-deprecated-core` all reason
about edges into `node_modules`. All three can be disabled without touching a
single rule, by a change under `options:`:

- **`includeOnly`** drops every module outside its pattern — including every npm
  package. Do not reintroduce one.
- **`node_modules` in `exclude`** (rather than `doNotFollow`) does the same.
  `doNotFollow` records the typed edge and stops there; `exclude` deletes it.
- **An unanchored `exclude` pattern** such as `(^|/)dist/` also matches
  `node_modules/vite/dist/index.js`, silently dropping any package whose entry
  point sits in a `dist/` folder. Every exclude is anchored to
  `^(apps|packages|tools)/` for exactly this reason.

The failure mode is invisible: the gate still passes, and it now proves nothing.

**So any change under `options:` must be proved with an injection test.**

```bash
pnpm gates:prove
```

That runs, in order:

| Script                      | Proves                                                           |
| --------------------------- | ---------------------------------------------------------------- |
| `pnpm graph:injection`      | Every rule above fires on its own violation, by name             |
| `pnpm graph:determinism`    | Two runs over an unchanged tree are byte-identical               |
| `pnpm boundaries:rust:test` | The Tauri check catches a direct **and** a transitive dependency |
| `pnpm deny:test`            | The licence gate rejects GPL, AGPL and LGPL, and accepts MIT     |

Each writes its violation into a throwaway location, asserts the gate fails,
and removes it. The repository is never modified — no `Cargo.toml` is edited
and no `Cargo.lock` is regenerated. **A rule you have not seen fail is a rule
you have not tested.**

## Reading the raw index directly

`tools/project-graph/output/index.json` is the machine-readable source of truth
and is committed, so it can be read without installing anything:

```json
{
  "files": {
    "<path>": {
      "workspace": "apps/desktop",
      "layer": "renderer",
      "isTest": false,
      "dependsOn": [],
      "dependedOnBy": [],
      "coveredByTests": [],
      "hasNoDependents": false
    }
  }
}
```

Prefer this over crawling the repository.

## Workflow when changing shared code

```
1. node tools/project-graph/query.mjs <file>     # blast radius
2. Read the dependents — will the change break them?
3. Make the change
4. Run the tests listed under "Covered by tests"
5. pnpm graph          (if structure changed — commit the regenerated output)
6. pnpm graph:validate && pnpm boundaries:rust && pnpm deny:licenses
7. pnpm graph:check    (what CI runs; fails if the committed graph is stale)
```

## Limitation — know it, it matters more here than in a web app

This is **static analysis over TypeScript imports**. The two most important
edges in Blinkify are invisible to it:

- **The IPC edge.** A Tauri command is resolved by name at runtime. The
  renderer calling `invoke("probe_media")` and the Rust `#[tauri::command]` that
  answers it are, to this graph, unrelated files. Renaming a command breaks the
  application and moves nothing in the graph. Grep the command name.
- **The Rust graph.** `crates/` contains no TypeScript, so it does not appear
  here at all. `cargo tree` is the equivalent, and `pnpm boundaries:rust` is the
  gate.

`query.mjs` prints a reminder when the file you asked about sits on either side
of one of those edges. Believe the reminder over the file list.
