# Blinkify project graph

<!-- GENERATED FILE — do not edit. Run `pnpm graph` to regenerate. -->

Scanned: apps, packages, tools

TypeScript only. The Rust crate boundaries are checked by
`pnpm boundaries:rust` and the Rust licences by `cargo deny check licenses`.

## Totals

- Files: **182**
- Test files: **21**
- Architecture rule violations: **0**

## Workspaces

| Workspace | Files | Tests | Files with no direct test |
| --- | ---: | ---: | ---: |
| `apps/desktop` | 47 | 15 | 13 |
| `packages/types` | 70 | 0 | 69 |
| `packages/ui` | 39 | 6 | 18 |
| `tools` | 26 | 0 | 26 |

## Architecture rule violations

_None._

## How to query this graph

```bash
pnpm graph                                  # regenerate
node tools/project-graph/query.mjs <path>   # impact of changing a file
```

`output/index.json` is the machine-readable source of truth. For any file it
records `dependsOn`, `dependedOnBy`, `coveredByTests`, and its owning
workspace and layer — read that instead of crawling the repository.
