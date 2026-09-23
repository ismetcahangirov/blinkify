# Architecture gates

Seven checks stand between a change and `main`. Each one enforces a boundary
that [`../../CLAUDE.md`](../../CLAUDE.md) states as a rule. This document says
how they work, what each one does **not** cover, and how to prove one still
works after you touch it.

## The gates

| Command                | Enforces                                                   | Rule       |
| ---------------------- | ---------------------------------------------------------- | ---------- |
| `pnpm graph:validate`  | TypeScript import boundaries                               | section 2  |
| `pnpm graph:check`     | The committed project graph matches the source tree        | section 14 |
| `pnpm boundaries:rust` | No engine crate reaches `tauri`, directly or transitively  | section 2  |
| `pnpm deny:licenses`   | No GPL, AGPL or LGPL Rust crate                            | section 10 |
| `pnpm colours:check`   | No colour value outside the design token file              | #15        |
| `pnpm offline:check`   | No remote asset the interface needs in order to appear     | #16        |
| `pnpm tokens:check`    | No hard-coded length, duration or weight in `packages/ui`  | #17        |
| `pnpm sidecar:check`   | The bundled FFmpeg is LGPL-only, ours, and complete        | #21        |
| `pnpm types:check`     | The committed IPC contract is what the Rust types generate | #32        |
| `pnpm evaluator:check` | Only the shared evaluator interprets the edit graph        | #30        |

All ten run in `pnpm verify`, and all ten block a merge — see
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

## The colour gate, and why it is not ESLint

`tools/colour-gate.mjs` enforces one rule: exactly one file in `apps/` and
`packages/` may contain a colour value, and it is
`packages/ui/src/tokens/tokens.css`. Everything else reads a semantic token.

It is a scanner rather than an ESLint rule because ESLint reads TypeScript, and
two thirds of the colours in a React application live in `.css` files. A rule
that covered only the TypeScript half would report green over a stylesheet full
of literals — the same shape of hollow gate the injection tests exist to catch.

What it catches: hex literals, colour functions (`rgb()`, `oklch()`,
`color-mix()`), and — in CSS only, where the word is unambiguous — the CSS
named colours. `var()` references are stripped first, so `var(--brand-azure)`
is not read as the named colour `azure`, and comments are stripped, so a token
documented by its value is not a violation.

What it does **not** catch, and cannot:

- A colour smuggled in through a computed string —
  `` `#${hex}` `` or `` `rgb(${r} ${g} ${b})` ``. Review catches that one.
- A **wrong** token. `--danger` on a save button passes every gate and is still
  wrong. The gate proves a value came from the token file, not that the right
  token was chosen.
- Anything outside `apps/` and `packages/`. `docs/` is full of colour values on
  purpose — that is where the palette is explained.

Test files are exempt, because the contrast machinery has to be able to name a
colour in order to be tested at all. A test styles nothing, so the exemption
cannot reach the interface.

The related test — `tokens.test.ts` in `packages/ui` — is a different gate with
a different job: it measures every documented pair against WCAG AA and fails if
the palette drifts or the table in
[`../design/colour-tokens.md`](../design/colour-tokens.md) stops matching the
tokens it describes.

## The offline-assets gate

`tools/offline-assets-gate.mjs` enforces one property: nothing the interface
needs in order to appear may come off a network.

It exists because the failure is quiet in the way that matters most. A
`<link>` to a font service, or an `url(https://…)` in a stylesheet, works on the
machine of whoever added it, works in CI, works in every demonstration — and
then a user opens Blinkify on a train and the interface has no typeface.
`CLAUDE.md` section 20 rule 8 forbids a network call other than the update
check; this gate is the half of that rule a scanner can see.

What it catches, after stripping comments:

| Where  | Shape                                          |
| ------ | ---------------------------------------------- |
| `.css` | `url(https://…)`, `@import "https://…"`        |
| markup | `src="https://…"`, `<link … href="https://…">` |

Protocol-relative `//host/path` counts, because it is a remote URL that merely
declines to name its scheme — and it is the form a copied snippet usually
carries.

What it deliberately does **not** catch:

- A URL in a comment or in prose. `packages/ui/src/fonts/README.md` records
  exactly where each font file came from, and it has to stay able to. A gate
  that flagged documentation would be routed around inside a month.
- An `<a href>` a user clicks. That is a navigation, not an asset.

What it does **not** catch and cannot:

- A runtime `fetch()`. Nothing in the gate reads control flow, and a request
  fired from a component is caught by review and by the Tauri capability
  allowlist, not here.
- Anything outside `apps/` and `packages/`. The updater endpoint lives in
  `tauri.conf.json` and is the one outbound request Blinkify is allowed.
- A font file that is committed but wrong. `fonts.test.ts` covers that: it
  parses the WOFF2 that actually ships and asserts the timecode digits are all
  one width.

## The design-token gate

`tools/design-token-gate.mjs` is the colour gate's other half. Colour has its
own scanner; this one covers every measurement a component can make — lengths,
durations and font weights — inside `packages/ui`.

The failure it prevents has the same shape and is just as quiet. A component
with `padding: 10px` looks perfectly fine, sits two pixels off the grid every
other control is on, and nobody sees it until two panels end up side by side in
a screenshot.

It scans `packages/ui/src` minus the files whose job is to hold values: the
three token layers in `tokens/`, `fonts/fonts.css` (whose `unicode-range` is a
character range, not a measurement), and test files, which have to be able to
write a number in order to assert on one. **Story files are not exempt** — a
story laid out with `gap: 12px` shows the component in a spacing the product
does not have.

What it accepts, and why each is safe rather than an oversight: zero, which is
zero in every unit; percentages and viewport-relative values inside a token,
because `width: 100%` is a relationship to a parent rather than a size; angles,
because one turn is one turn; and unitless numbers like `flex: 1`.

What it does **not** catch:

- A value assembled at runtime — `` `${n}px` ``. Review catches that one.
- A **wrong** token. `--space-16` where `--space-2` was meant passes every gate.
- The renderer. `apps/desktop` is Tailwind's, and Tailwind's own off-scale steps
  are cleared in `styles.css` so that a utility either maps to a Blinkify token
  or does not exist.

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
about the _shape_ of a message — provided the generated files are current,
which `pnpm types:check` enforces (#32): it regenerates the contract into a
scratch directory and fails on any difference, including a file for a type
that no longer exists. They can still disagree about its _name_.
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

| Script                      | Proves                                                                                                                                                                                                                                                                       |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm graph:injection`      | Each of the seven rules fires on its own violation, **by name**                                                                                                                                                                                                              |
| `pnpm graph:determinism`    | Two generator runs over an unchanged tree are byte-identical                                                                                                                                                                                                                 |
| `pnpm boundaries:rust:test` | The Tauri check catches a direct **and** a transitive dependency                                                                                                                                                                                                             |
| `pnpm deny:test`            | The licence gate rejects GPL, AGPL and LGPL, and accepts MIT                                                                                                                                                                                                                 |
| `pnpm colours:check:test`   | The colour gate catches a hex, an `rgb()` and a named colour, and does not catch a `var()` reference or a comment                                                                                                                                                            |
| `pnpm offline:check:test`   | The offline gate catches a font-service `<link>`, a remote `url()`, a remote `@import` and a remote `src`, and does not catch a URL in a comment or an `<a href>` a user clicks                                                                                              |
| `pnpm sidecar:check:test`   | The sidecar gate rejects a GPL build, a non-free build, a distributor's "LGPL" build carrying `openh264`, a build missing `arnndn` or `hevc_nvenc`, a binary whose hash differs from the lock, and a committed `ffmpeg.exe` — see [`ffmpeg-sidecar.md`](./ffmpeg-sidecar.md) |
| `pnpm evaluator:check:test` | The evaluator boundary catches an `Operation::` taken apart in the shell or the player, and a renderer or design-system file reading `.operations`; it lets through the evaluator itself, tests, `AudioOperation::` and renderer test fixtures                               |
| `pnpm types:check:test`     | The IPC contract gate catches a type changed without regenerating, a new type not committed, and a removed type still committed                                                                                                                                              |
| `pnpm tokens:check:test`    | The design-token gate catches a pixel padding, a rem radius, a millisecond duration, a numeric font weight and a length in an inline style, and does not catch a percentage, a `calc()` over tokens, zero, or the token files themselves                                     |

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
