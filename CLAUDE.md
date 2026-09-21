# Blinkify — Engineering Rulebook

This file is the contract for working on Blinkify. It is binding. A change that
violates a rule here does not get merged, regardless of how well it works.

Read sections 1, 7, 8 and 20 before your first change. Read the rest as you need
them.

---

## 1. Project overview

Blinkify is a Windows desktop video editor whose purpose is to edit video
**without degrading it**.

### The premise, stated as a constraint

Mainstream consumer editors re-encode the entire timeline on every export. A
user who trims four seconds off a 4K phone clip and exports it gets back a file
that is measurably worse than the one they started with, having changed nothing
about the pixels they kept. That loss is not a law of physics. It is a
consequence of an export pipeline that only knows how to decode and re-encode.

Blinkify exists because that loss is avoidable. Therefore:

> **Binding constraint.** No operation in Blinkify may degrade media quality
> that did not have to be degraded. Where the user's edit does not require the
> pixels to change, the pixels must reach the output file bit-identical to the
> source.

This is not an aspiration, a stretch goal, or a marketing line. It is the
product. A feature that cannot be built within it does not ship; it gets
declined, in the UI, with a reason the user can read.

### The three tiers

Every export decision resolves to exactly one of three tiers, per segment, and
the tier is always recorded:

| Tier                   | When it applies                                                    | What happens to the pixels                                                                                                                                |
| ---------------------- | ------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **1 — Stream copy**    | The segment is bounded by keyframes and no filter touches it       | Packets are copied byte-for-byte. Zero generation loss.                                                                                                   |
| **2 — Smart-cut**      | A cut point is not keyframe-aligned                                | Only the partial GOP at the boundary is re-encoded. The interior of the segment is stream-copied. Loss is confined to a fraction of a second at the seam. |
| **3 — Full re-encode** | A filter forces the pixels to change (crop, scale, burned overlay) | The segment is decoded, filtered and re-encoded. The user is told, before export, which segments these are and why.                                       |

The tiers are ordered. The export planner must choose the **lowest tier that
satisfies the edit**. Choosing tier 3 where tier 1 would have worked is a bug of
the most serious class this project has, because it is silent — the output file
plays fine and the user never learns what was taken from them.

Audio is a separate stream and gets its own tier decision. Applying a gain
filter to audio must not drag the video stream into tier 3. See
`docs/decisions/` and Epic #7.

### Non-goals

Blinkify is not a compositor, not a colour grading suite, and not a
motion-graphics tool. It does trims, cuts, speed, audio repair and ordering, and
it does them losslessly. Feature requests that require every frame to be
re-encoded are evaluated against section 1 first and the roadmap second.

---

## 2. Architecture

Three layers, one direction of dependency.

```
┌──────────────────────────────────────────────┐
│  apps/desktop/src        React 19 renderer   │  UI, timeline canvas, state
└───────────────┬──────────────────────────────┘
                │  packages/types (generated IPC contract)
┌───────────────▼──────────────────────────────┐
│  apps/desktop/src-tauri  Tauri 2 shell       │  window, commands, events, fs
└───────────────┬──────────────────────────────┘
                │  plain Rust calls
┌───────────────▼──────────────────────────────┐
│  crates/*                Media engine        │  probe, index, plan, execute
└───────────────┬──────────────────────────────┘
                │  process boundary
┌───────────────▼──────────────────────────────┐
│  FFmpeg sidecar (LGPL build, bundled)        │
└──────────────────────────────────────────────┘
```

Rules that follow from the diagram, and are enforced by tooling, not by good
intentions:

- **The renderer never reaches the engine directly.** It calls Tauri commands
  and consumes Tauri events, both typed by `packages/types`. Enforced by the
  `renderer-not-into-engine` rule in `.dependency-cruiser.cjs`.
- **Engine crates never import `tauri`.** An engine crate that depends on Tauri
  cannot be tested without a window, which means it will stop being tested.
  `cargo test -p blinkify-engine` must pass with no Tauri present. Enforced by a
  crate-boundary check in CI.
- **`packages/types` is generated, not hand-written.** The Rust types are the
  source of truth. A hand-maintained mirror drifts, and a drifting IPC contract
  is a bug class we can design out now for the cost of one build step.
- **The IPC surface is coarse.** Few, chunky commands plus an event channel for
  progress. Tauri 2 IPC serialises across a process boundary; a per-frame or
  per-clip command is a performance bug waiting to be written.
- **The engine owns all media decisions.** The renderer may display a tier, but
  it must never compute one. The planner is the single source of truth for what
  gets copied and what gets re-encoded.

Longer-form architecture notes live in `docs/architecture/`.

---

## 3. Technology stack

| Layer          | Choice                               | Why, in one line                                                         |
| -------------- | ------------------------------------ | ------------------------------------------------------------------------ |
| Shell          | Tauri 2                              | ~10 MB binary, no bundled Node runtime, engine is Rust anyway. ADR-0001. |
| Renderer       | React 19 + TypeScript + Vite         | Familiar, fast HMR, strict types.                                        |
| Renderer state | Zustand                              | Small, no provider tree, no state library in the engine.                 |
| UI primitives  | Radix                                | Unstyled and accessible; styling is ours.                                |
| Timeline       | HTML canvas, hand-rolled             | A DOM node per clip does not survive a long timeline.                    |
| Engine         | Rust, workspace in `crates/`         | Media work is CPU-bound and must not block the UI thread.                |
| Media I/O      | Bundled FFmpeg sidecar, LGPL build   | ADR-0002. Licence boundary is load-bearing.                              |
| Monorepo       | pnpm workspaces + Turborepo          | Mirrors TezUsta.                                                         |
| TS tests       | Vitest                               |                                                                          |
| Rust tests     | `cargo test`                         |                                                                          |
| Packaging      | Tauri NSIS bundler, per-user install | No elevation prompt.                                                     |

Node version is pinned in `.nvmrc`. The Rust toolchain is pinned in
`rust-toolchain.toml`. Neither is optional — an unpinned toolchain makes CI
failures unreproducible locally.

**TypeScript runs strict, with `noUncheckedIndexedAccess`.** **Clippy runs at
`-D warnings`.** Both are gates, not suggestions.

---

## 4. Folder and naming conventions

```
blinkify/
├── apps/
│   └── desktop/
│       ├── src/              React renderer
│       └── src-tauri/        Tauri shell (thin — commands and window only)
├── crates/                   Rust engine workspace (no tauri dependency)
├── packages/
│   ├── ui/                   Design system. May not import from apps/.
│   └── types/                Generated IPC contract types.
├── docs/                     See section 16.
├── tools/
│   └── project-graph/        Graph generator, query and validate.
└── .github/                  Workflows, issue and PR templates, labels.yml.
```

Naming:

- **Rust**: `snake_case` files and functions, `PascalCase` types, crates named
  `blinkify-<area>` (`blinkify-engine`, `blinkify-probe`).
- **TypeScript**: `PascalCase.tsx` for components, `camelCase.ts` for everything
  else, `use*` for hooks, `*.store.ts` for Zustand stores.
- **Tests**: `*.test.ts` beside the source in TypeScript; `#[cfg(test)]` module
  in the same file for Rust units, `crates/<crate>/tests/` for integration.
- **Branches**: `<type>/<issue-number>-<short-slug>`, e.g.
  `feat/40-keyframe-aligned-cut`.

No file is named `utils`, `helpers`, `misc`, `common` or `shared`. Those names
are where architecture goes to die. Name the thing after what it does.

---

## 5. Git rules

- **`main` is protected.** No direct pushes. Every change arrives by pull
  request with CI green.
- **Linear history.** Squash merge. No merge commits on `main`.
- **Conventional Commits**, enforced by convention and reviewed in the PR:
  `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `perf:`, `build:`,
  `ci:`. Scope is the area label where one applies: `feat(export): …`.
- **One issue per branch, one branch per issue.** A pull request that closes two
  issues is two pull requests.
- **The commit body says why, not what.** The diff already says what.
- **Never commit credentials, signing keys, or the media test corpus.** The
  corpus is binary and belongs in a release asset or is generated at job start.
  Git history is permanent; a key pushed once is a key that must be rotated.
- **Never rewrite published history.** No force-push to `main`, ever, and none
  to a shared branch without saying so first.
- **The repository owner is the sole author of every commit.** No assistant,
  tool or model is ever named as an author, co-author or generator — no
  `Co-Authored-By` trailer naming an AI, no "Generated with" line, no
  attribution in a commit message, pull request body, issue comment or release
  note. Authorship metadata states who is accountable for the code, and that is
  a person; a tool that helped write a line is no more an author than the editor
  it was typed into. This matters more here than it looks, because git history
  is permanent and rewriting it is forbidden two bullets up — a trailer added
  once cannot be taken back.

---

## 6. GitHub rules

- **Every change traces to an issue.** No issue, no branch. If the work is worth
  doing it is worth one paragraph of why.
- **Epics do not receive code.** An Epic is a container; work happens in its
  sub-issues and the Epic closes when they all do.
- **Issue templates are mandatory**, blank issues are disabled. Epic,
  Implementation and Bug — see `.github/ISSUE_TEMPLATE/`.
- **Requirements and acceptance criteria are different things.** Requirements
  are what you build; acceptance criteria are how someone else proves you built
  it. See `docs/project-management/issue-rules.md`.
- **Labels are committed** in `.github/labels.yml`. Creating a one-off label in
  the GitHub UI without adding it there is how a label set rots.
- **Pull requests close their issue** with a `Closes #N` line, and carry the
  Definition of Done checklist from `.github/pull_request_template.md`.
- **A bug report about media is unreproducible without the file's
  characteristics.** Container, codec, VFR or CFR, and whether it came off a
  phone. The bug template asks for all four.

Branch naming, Conventional Commits and the Epic-to-sub-issue relationship are
described in full in `docs/engineering/github-workflow.md`.

---

## 7. The per-task workflow

Follow this in order. The steps that look skippable are the ones that catch the
expensive mistakes.

1. **Read the issue in full**, including the technical considerations. They exist
   because someone already hit the trap.
2. **Read what the issue depends on.** If it says "blocked by #N", read #N and
   confirm it is actually done, not just closed.
3. **Establish the blast radius** before touching existing code:
   `node tools/project-graph/query.mjs <file>` prints dependants and covering
   tests. Do this instead of a repository-wide grep. See section 14.
4. **Research before choosing** — see section 9. Media handling is full of
   plausible approaches that are quietly wrong.
5. **Write the test first** where the behaviour is testable. For anything under
   section 1 — anything touching what reaches the output file — the test is not
   optional, and it asserts on bytes, not on "it plays".
6. **Implement the smallest change that satisfies the issue.** Scope creep in a
   pull request is a review burden the reviewer did not agree to.
7. **Run `pnpm verify` locally.** Do not delegate first discovery of a failure to
   CI.
8. **Open the pull request** with `Closes #N` and the Definition of Done
   checklist filled honestly. An unchecked box with a sentence explaining why is
   fine. A checked box that is not true is not.
9. **Update the docs in the same pull request.** See section 16.

---

## 8. Definition of Done

A change is done when **all** of these hold. This is the checklist in the pull
request template.

- [ ] Every requirement in the issue is implemented, or explicitly deferred with
      a linked follow-up issue.
- [ ] Every acceptance criterion in the issue is demonstrably met.
- [ ] Tests cover the new behaviour, including the failure cases, and they fail
      if the behaviour is removed.
- [ ] `pnpm verify` passes locally — format, lint, typecheck, test, build, graph.
- [ ] CI is green on every gate.
- [ ] The project graph is regenerated and committed if the source tree moved.
- [ ] Documentation is updated: ADR for a decision, `docs/` for a concept,
      doc-comments for a public API.
- [ ] No new dependency without section 10 satisfied.
- [ ] **If the change touches the media path**: the tier decision is unchanged or
      the change is recorded, and the losslessness suite still passes.
- [ ] No `TODO`, commented-out code, or debug logging left behind.

---

## 9. Research requirements

Before choosing an approach in an area you have not implemented before, and
always before a decision that is expensive to reverse:

- **Read the upstream source or specification**, not a blog summary of it.
  FFmpeg's behaviour around timestamps, open GOPs and stream copy is not
  accurately described by most secondary sources.
- **Verify against a real file.** A claim about container behaviour is
  unverified until it has been run against an actual sample, including a phone
  capture with variable frame rate.
- **Check the licence before the API.** A library that solves the problem under
  GPL does not solve our problem. See section 10 and ADR-0002.
- **Write down what you rejected.** The next person will consider the same
  alternative. An ADR that lists only the winner is half an ADR.

Where the research changes an architectural decision, it becomes an ADR. Where
it is merely useful, it goes in `docs/architecture/` or `docs/engineering/`.

---

## 10. Dependency policy

A new dependency is a permanent liability. Adding one requires all of:

1. **Licence is compatible.** Permissive (MIT, Apache-2.0, BSD, ISC) or LGPL
   used across a process or dynamic-link boundary. **GPL and AGPL are
   prohibited**, in the renderer and in the engine, without exception. See
   ADR-0002 — linking GPL FFmpeg would make Blinkify GPL and remove the option
   of a commercial product. `cargo deny check licenses` enforces this for Rust.
2. **It is maintained.** A commit in the last year, or it is genuinely finished
   and you can say why.
3. **It earns its weight.** A dependency for something a competent developer
   writes in forty lines is not worth the supply chain risk.
4. **It is added to `THIRD_PARTY.md`** if it ships to the user.
5. **It is pinned.** Lockfiles are committed. `pnpm-lock.yaml` and `Cargo.lock`
   both.

Removing a dependency never requires permission.

---

## 11. Security

- **No credentials in the repository.** Not in code, not in config, not in a
  test fixture, not in a commit message. The updater signing key lives in GitHub
  Actions secrets.
- **The updater verifies signatures.** An update manifest that fails signature
  verification is rejected, and there is a negative test proving it.
- **Treat every media file as hostile input.** A malformed file is the normal
  case, not the edge case. Parsers return errors; they do not panic, and they do
  not index blindly into a buffer whose length came from the file.
- **The FFmpeg sidecar is invoked with an argument vector, never a shell
  string.** A filename containing a quote is not an injection vector.
- **File system access is scoped.** The Tauri allowlist grants the narrowest
  scope that works.
- **Never phone home.** No telemetry, no analytics, no silent network calls. The
  update check is the only outbound request, and it happens on launch and is
  visible.

---

## 12. Performance

Budgets, not vibes. These are asserted in tests where the issue says so.

- **UI stays at 60 fps during timeline interaction.** Drag, trim and zoom on a
  long timeline. Hence the canvas renderer and its virtualisation.
- **No media work on the UI thread.** Ever. Probe, index, decode and export all
  run in the engine, off-thread, and report progress by event.
- **Cancellation is real.** A cancelled export kills the sidecar process and
  cleans up its partial output. A "cancelled" flag that lets the work run to
  completion is a lie to the user.
- **Caches are on disk and keyed by content**, not by path. Moving a file must
  not invalidate its waveform peaks; editing it must.
- **Seek is frame-accurate and keyframe-aware.** Decoding from the start of the
  file to reach 00:41:12 is not an implementation, it is a placeholder.
- **Measure before optimising, and commit the measurement.** An optimisation
  without a before-number is a guess.

---

## 13. Testing

- **Test behaviour, not implementation.** A test that breaks on a rename but not
  on a regression is negative value.
- **Anything under section 1 is tested on bytes.** "The output plays" is not an
  assertion. The losslessness suite compares packet payload hashes against the
  source. Timestamps are outside the hash boundary by design, because rebasing
  them is correct behaviour — see Epic #6 and #45.
- **Every bug fix starts with a failing test** that reproduces the bug. No
  reproduction, no fix — you do not yet know what you are fixing.
- **Failure paths are tested**, including a corrupt file, a missing stream, a
  zero-length file, a file that disappears mid-export, and a full disk.
- **The media corpus is generated or fetched, never committed.** Binary blobs in
  git history are permanent.
- **Rust engine tests run without Tauri.** `cargo test -p blinkify-engine` with
  no window, no WebView2, no renderer.
- **Heavy suites run on their own workflow.** The pull-request gate has a
  15-minute budget; the losslessness suite and the timeline benchmark block a
  release, not a merge. A pipeline that routinely overruns gets bypassed.

---

## 14. Project graph

`tools/project-graph/` answers "what breaks if I change this file" in one
command, instead of a repository-wide search that misses the indirect
dependants.

```bash
pnpm graph                                   # regenerate output/
pnpm graph:check                             # fails if the committed graph is stale
pnpm graph:validate                          # fails on a boundary-rule violation
node tools/project-graph/query.mjs <file>    # dependencies, dependants, covering tests
```

Rules:

- **Run `query.mjs` before modifying existing code.** This is step 3 of the
  workflow and it is not optional.
- **The generator reads no clock and sorts every collection.** Two runs on the
  same tree are byte-identical, so a non-empty diff means the source tree
  actually moved. Do not reintroduce a timestamp; it turns the gate into noise.
- **Regenerate and commit `output/` whenever the tree moves.** `graph:check` is
  a blocking CI gate.
- **`options:` in `.dependency-cruiser.cjs` is dangerous.** `includeOnly`,
  `node_modules` in `exclude`, and unanchored `exclude` patterns all silently
  delete the edges the rules reason about — leaving a gate that passes and
  proves nothing. Any change under `options:` must be proved with an injection
  test: introduce a violation, confirm `graph:validate` fails, revert.
- **Two languages, two tools.** dependency-cruiser covers the renderer; the Rust
  side is covered by a crate-boundary check and `cargo deny`. Do not pretend one
  tool covers both.

---

## 15. Memory system

`claude-mem` is enabled at user level. It is a **convenience layer and nothing
more**.

> Any decision that matters lives in `docs/` or in a GitHub issue. Never only in
> memory.

Memory is not shared with a new contributor, is not reviewable, is not
versioned, and cannot be cited in a pull request. If you find yourself relying
on a remembered decision, that is the signal to write the ADR you skipped.

---

## 16. Documentation rules

`docs/` has six directories, each with a README stating what belongs there:

| Directory             | Contents                                                      |
| --------------------- | ------------------------------------------------------------- |
| `architecture/`       | How the system is put together and why it holds.              |
| `decisions/`          | ADRs. Immutable once accepted.                                |
| `design/`             | Visual language, layout references, component specifications. |
| `engineering/`        | How to work on it: workflow, release, debugging, conventions. |
| `product/`            | What it does for a user and what it deliberately does not.    |
| `project-management/` | Roadmap, issue rules, planning.                               |

Rules:

- **Docs ship in the same pull request as the code.** A documentation debt issue
  is a promise nobody keeps.
- **ADRs are immutable once accepted.** To change a decision, write a new ADR
  that supersedes the old one and mark the old one `Superseded by ADR-NNNN`.
  Never edit an accepted ADR's decision.
- **Every ADR lists the alternatives that were rejected and why.** Without them
  the next person re-litigates the decision from scratch.
- **Link, do not duplicate.** Two copies of a rule become two different rules.
- **Markdown is formatted by Prettier** and the links are checked in CI. Both
  are blocking gates.

---

## 17. Design decisions

- **The layout follows CapCut's desktop structure deliberately**, so a user
  arriving from CapCut is not re-learning where things are. The structure is
  documented zone by zone in `docs/design/`. The visual language is Blinkify's
  own — derived from the logo, not copied.
- **Dark theme first.** Video work happens against dark chrome so the preview is
  what the eye adapts to.
- **The lossless tier is visible in the UI, always.** The export dialog states
  which segments are copied and which are re-encoded, before the export starts,
  and the report states what actually happened after. A user must never have to
  guess whether quality was preserved.
- **Decline honestly.** Where Blinkify cannot do something losslessly and the
  machine offers no way to do it well, it says so and refuses, with the reason.
  It does not silently substitute a worse result. See ADR-0003.
- **A design decision that changes a token, a scale or a layout zone is an ADR
  or a `docs/design/` update**, not a comment in a component file.

---

## 18. Asking questions

Ask when the answer changes what gets built. Do not ask to transfer a decision
you are able to make.

**Ask before proceeding** when:

- Two reasonable readings of the issue produce materially different work.
- The change would weaken section 1, even slightly.
- It requires a new dependency that fails any test in section 10.
- It changes a public IPC contract other code depends on.
- It is destructive or hard to reverse — see section 19.

**Do not ask** about naming, file placement, test structure, or anything the
rulebook already answers. Decide, state the assumption in the pull request, and
move on.

When you ask, bring a recommendation. "A or B?" wastes a round trip; "I propose
A because X, unless you want B for Y" does not.

---

## 19. Never destroy existing work

- **Never delete or rewrite code you did not come to change.** If it looks
  wrong, open an issue.
- **Never `git push --force` to a shared branch.** Never to `main` at all.
- **Never discard a user's media.** No operation overwrites a source file. Every
  export writes to a new path, and the source is opened read-only. This is not a
  preference — a video editor that can eat the only copy of a recording is
  unshippable.
- **Never overwrite an export target without confirmation.**
- **Never mass-rename or mass-reformat outside the change's scope.** It buries
  the real diff and makes review impossible.
- **Before deleting anything, look at it.** The rule applies to files,
  directories, branches, issues and labels alike.

---

## 20. Forbidden behaviours

Absolute. No exceptions, no "just this once", no flag to turn them off.

1. **No operation writes an intermediate media file.** Not a temporary
   transcode, not a "normalised" copy, not a scratch render. Every intermediate
   is a generation of loss and a disk the user did not agree to fill. The
   pipeline is decisions plus one pass to the output file. (Content-addressed
   _caches_ — waveform peaks, thumbnails, keyframe indices, optional preview
   proxies — are not intermediates: they are derived artefacts that never reach
   an output file. The rule is about the export path.)
2. **No re-encode without a recorded reason.** Every tier-3 segment carries the
   reason it could not be copied, the reason is surfaced in the export report,
   and it is stored with the plan. A re-encode nobody can explain is a bug by
   definition.
3. **No silent quality loss.** If the output will differ from the source, the
   user is told before the export starts, not after.
4. **No source file is ever modified or deleted.** Read-only, always.
5. **No shelling out to a command string.** Argument vectors only.
6. **No GPL or AGPL dependency.** See section 10.
7. **No credential, key or token in the repository.**
8. **No network call except the update check.**
9. **No media work on the UI thread.**
10. **No merging with a red gate**, and no disabling a gate to go green. Fix the
    cause or open an issue and mark the Definition of Done honestly.
11. **No editing an accepted ADR's decision.** Supersede it.
12. **No committing the media test corpus.**
13. **No AI or tool attribution anywhere in the project's record.** Not in a
    commit trailer, not in a commit message, not in a pull request body, not in
    an issue, not in a release note. No gate can catch this — it is caught in
    review, which is why it is listed here and not only in section 5.

---

## 21. Commands

```bash
# Setup
pnpm install                   # install workspace dependencies

# Development
pnpm dev                       # renderer only, in the browser
pnpm tauri dev                 # the actual application window

# Verification — run this before every pull request
pnpm verify                    # format + lint + typecheck + test + build + graph

# Individual gates
pnpm format                    # Prettier, write
pnpm format:check              # Prettier, check only
pnpm lint                      # ESLint
pnpm typecheck                 # tsc --noEmit
pnpm test                      # Vitest + cargo test
pnpm build                     # build all packages

# Project graph
pnpm graph                     # regenerate
pnpm graph:check               # fail if the committed graph is stale
pnpm graph:validate            # fail on a boundary violation
node tools/project-graph/query.mjs <file>

# Rust, directly
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test -p blinkify-engine  # must pass with no Tauri present
cargo deny check licenses

# Packaging
pnpm tauri build               # NSIS installer, per-user
```

---

## Where to look next

- `docs/decisions/` — why the load-bearing choices are what they are
- `docs/project-management/roadmap.md` — the Epics and their order
- `docs/engineering/github-workflow.md` — branches, commits, issues
- `THIRD_PARTY.md` — what ships with Blinkify and under which licence
