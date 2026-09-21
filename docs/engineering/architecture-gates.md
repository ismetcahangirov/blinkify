# Architecture gates

Four checks stand between a change and `main`. Each one enforces a boundary
that [`../../CLAUDE.md`](../../CLAUDE.md) states as a rule. This document says
how they work, what each one does **not** cover, and how to prove one still
works after you touch it.

## The gates

| Command                | Enforces                                                  | Rule       |
| ---------------------- | --------------------------------------------------------- | ---------- |
| `pnpm graph:validate`  | TypeScript import boundaries                              | section 2  |
| `pnpm graph:check`     | The committed project graph matches the source tree       | section 14 |
| `pnpm boundaries:rust` | No engine crate reaches `tauri`, directly or transitively | section 2  |
| `pnpm deny:licenses`   | No GPL, AGPL or LGPL Rust crate                           | section 10 |

All four run in `pnpm verify`, and all four block a merge — see
[`ci.md`](./ci.md) for how they are wired into the pipeline and in what order.

## Two languages, two tools — and why that is not pedantry

`dependency-cruiser` reads TypeScript. It resolves `import` statements through
Node and pnpm's workspace links. It has no idea `crates/` exists.

So a green `pnpm graph:validate` says nothing at all about the engine. It cannot
tell you whether `blinkify-engine` has grown a `tauri` dependency, and that is
the boundary with the most riding on it: an engine crate that needs Tauri cannot
be built or tested without a window and a WebView, which means it stops being
tested — and the engine is where every decision about what reaches the user's
output file is made.

`tools/crate-boundaries.mjs` is the other tool. It reads `cargo metadata` and
walks the **transitive** dependency graph of every workspace member under
`crates/`. Transitive, not direct: a crate that pulls Tauri in through an
intermediate still cannot be built without it, so a direct-only check would
report success while the property it claims to protect was already gone.

## What no gate covers: the IPC edge

The most important coupling in this codebase is invisible to static analysis in
either language. A Tauri command is resolved **by name at runtime**:

```
apps/desktop/src/…        invoke("probe_media", { path })
apps/desktop/src-tauri/…  #[tauri::command] fn probe_media(path: String) -> …
```

To `dependency-cruiser` those are unrelated files. To `cargo`, the renderer does
not exist. Rename the command on one side and the application breaks while every
gate stays green.

`packages/types` is the mitigation, not the graph: the contract is generated
from the Rust types by `pnpm types:generate`, so the two sides cannot disagree
about the _shape_ of a message. They can still disagree about its _name_.
`query.mjs` prints a reminder when you ask it about a file on either side of
that edge. Grep the command name as well.

## Proving a gate still works

A gate can be disarmed without a rule being removed. The npm rules
(`not-to-dev-dep`, `no-non-package-json`, `no-deprecated-core`) all reason about
edges into `node_modules`, and three separate things under `options:` in
`.dependency-cruiser.cjs` delete those edges before the rule engine sees them:

- `includeOnly`, which drops every module outside its pattern;
- `node_modules` in `exclude` rather than in `doNotFollow` — `doNotFollow`
  records the typed edge and stops there, `exclude` deletes it;
- an unanchored `exclude` pattern such as `(^|/)dist/`, which also matches
  `node_modules/vite/dist/index.js`.

In every case the gate still passes. It simply proves nothing, and nobody finds
out, because a gate's output looks the same whether it is working or hollow.

So CLAUDE.md section 14 requires an injection test for any change under
`options:`. That test is a command:

```bash
pnpm gates:prove
```

| Script                      | Proves                                                           |
| --------------------------- | ---------------------------------------------------------------- |
| `pnpm graph:injection`      | Each of the seven rules fires on its own violation, **by name**  |
| `pnpm graph:determinism`    | Two generator runs over an unchanged tree are byte-identical     |
| `pnpm boundaries:rust:test` | The Tauri check catches a direct **and** a transitive dependency |
| `pnpm deny:test`            | The licence gate rejects GPL, AGPL and LGPL, and accepts MIT     |

Asserting on the rule _name_ rather than on a non-zero exit code is the point.
Exit 1 could come from any rule, or from the tool failing to start. Only the
name proves the specific boundary caught it.

None of these modify the repository. The TypeScript cases write a file, run the
gate, and delete it; the Rust cases build a throwaway workspace in the temp
directory and point the checker at that, so no `Cargo.toml` is edited and no
`Cargo.lock` is regenerated.

### Adding a boundary

Add the rule **and its injection case in the same change.** A rule with no case
in `tools/project-graph/injection-test.mjs` is a rule nobody has watched fail,
which is indistinguishable from a rule that does not work.

## Why the graph is committed

`tools/project-graph/output/` is generated and committed, so it can be read
without installing anything and so CI can regenerate it and fail on a diff.

That only works because the generator is a pure function of the source tree. It
reads no clock, and it sorts every collection with a code-unit comparator rather
than the locale-aware one. The locale-aware comparator is ICU-backed and its
weights differ between ICU builds, so the same file list orders differently on
Windows and on a Linux runner. The resulting files are the same size, git calls
them binary, and CI prints "0 insertions(+), 0 deletions(-)" — which reads as a
broken gate rather than as a stale graph, and costs an afternoon.

`pnpm graph:determinism` is the standing proof. Do not reintroduce a timestamp.

## Licences

`deny.toml` carries the reasoning for every allowed licence. Two things there
are load-bearing and easy to get wrong:

- **LGPL is not on the allow list**, even though
  [ADR-0002](../decisions/ADR-0002-lgpl-ffmpeg-sidecar.md) is built on LGPL.
  ADR-0002 permits it _across a process boundary_ — the FFmpeg sidecar is a
  separate process invoked by argument vector. A Rust crate is statically linked
  into the binary, which is the case ADR-0002 rules out.
- **`all-features = true`.** A GPL crate behind an off-by-default feature is
  still in the tree, and a future `--features` flag turns it on without anyone
  editing `deny.toml`.

`cargo deny check advisories` is a separate gate on purpose. A new advisory can
turn a green build red without anybody changing a line, and that must not be
confused with a contributor having introduced a GPL dependency.

## Related

- [`../../CLAUDE.md`](../../CLAUDE.md) sections 2, 10 and 14 — the rules
- [`../../tools/project-graph/README.md`](../../tools/project-graph/README.md) — the tooling
- [`../decisions/ADR-0002-lgpl-ffmpeg-sidecar.md`](../decisions/ADR-0002-lgpl-ffmpeg-sidecar.md) — the licence boundary
- [`toolchain.md`](./toolchain.md) — pinned versions and the traps in them
- [`ci.md`](./ci.md) — where these gates run, and what does not fit in the
  pull-request budget
