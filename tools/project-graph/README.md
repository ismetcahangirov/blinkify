# Project graph

Answers "what breaks if I change this file?" in one command, instead of a
repository-wide search that misses the indirect dependants. CLAUDE.md section 7
step 3 makes running it a required step before modifying existing code.

## Scripts

| Command                  | What it does                                                      |
| ------------------------ | ----------------------------------------------------------------- |
| `pnpm graph`             | Regenerate `output/`                                              |
| `pnpm graph:check`       | Regenerate and fail if the committed `output/` is stale (CI gate) |
| `pnpm graph:validate`    | Fail on a TypeScript boundary violation (CI gate)                 |
| `pnpm graph:injection`   | Prove every rule still fires on its own violation                 |
| `pnpm graph:determinism` | Prove two runs over an unchanged tree are byte-identical          |
| `node query.mjs <path>`  | Impact report for one file                                        |

The Rust half lives one directory up: `tools/crate-boundaries.mjs` and
`deny.toml`. See `.claude/skills/project-graph-query/SKILL.md`.

## Files

| File                   |                                                            |
| ---------------------- | ---------------------------------------------------------- |
| `generate.mjs`         | Runs dependency-cruiser and writes the three artifacts     |
| `query.mjs`            | Reads `output/index.json` and prints a blast-radius report |
| `validate.mjs`         | The boundary gate                                          |
| `check.mjs`            | The staleness gate                                         |
| `injection-test.mjs`   | Writes each forbidden import and asserts its rule fires    |
| `determinism-test.mjs` | Runs the generator twice and compares bytes                |
| `output/`              | Committed, generated. Do not edit.                         |

## Output

| Artifact     |                                                        |
| ------------ | ------------------------------------------------------ |
| `graph.json` | The workspace dependency graph with full edge metadata |
| `index.json` | Condensed forward/reverse index, file-to-test mapping  |
| `GRAPH.md`   | Human-readable summary                                 |

`index.json` is the machine-readable source of truth. Read it rather than
crawling the repository.

## Why `output/` is committed

So the graph can be read without installing anything, and so CI can regenerate
it and fail on a diff. That gate is only worth having because the generator is a
pure function of the source tree: it reads **no clock** and sorts every
collection with a code-unit comparator, never the locale-aware one — an
ICU-backed comparator orders the same file list differently on Windows and
Linux, which produces a diff of identical size that git reports as binary and CI
reports as "0 insertions(+), 0 deletions(-)".

`pnpm graph:determinism` is the standing proof. Do not reintroduce a timestamp
or an unsorted collection.

## The trap worth knowing before you edit the config

Changing anything under `options:` in `.dependency-cruiser.cjs` can disable the
npm rules without touching a rule — `includeOnly`, `node_modules` in `exclude`,
or an unanchored `exclude` pattern each delete the edges those rules reason
about. The gate stays green and proves nothing.

`pnpm gates:prove` is what catches it. Run it after any change to the config.
