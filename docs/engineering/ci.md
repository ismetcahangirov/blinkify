# Continuous integration

Two workflows. The distinction between them is the point of this document:
**one blocks a merge, the other blocks a release.**

| Workflow                                             | Runs on                                | Blocks                                                           |
| ---------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------- |
| [`ci.yml`](../../.github/workflows/ci.yml)           | Every pull request, and push to `main` | A merge                                                          |
| [`heavy.yml`](../../.github/workflows/heavy.yml)     | Weekly, on demand, and on a `v*` tag   | A release                                                        |
| [`release.yml`](../../.github/workflows/release.yml) | A `v*` tag                             | Nothing — it _is_ the release. See [`release.md`](./release.md). |

Both run on `windows-latest`, and only there.

## Why Windows only

Blinkify targets Windows first, deliberately. A Linux pipeline would go green
while the product was broken for every user — and the traps that actually cost
time here are Windows traps a Linux runner cannot see: the MSVC linker, the NSIS
bundler, the WebView2 runtime, path separators in a committed artifact.

macOS and Linux runners arrive when macOS and Linux builds do, which is after
v1.

## The pull-request gate

Nine checks, each its own job so a failure is legible from the pull request
without opening a log.

| Check                       | Runs                                                                      |
| --------------------------- | ------------------------------------------------------------------------- |
| **Format (Prettier)**       | `pnpm format:check`                                                       |
| **Lint (ESLint)**           | `pnpm lint`                                                               |
| **Typecheck (tsc)**         | `pnpm typecheck`, then `pnpm build`                                       |
| **Test (Vitest)**           | `pnpm test:ts`                                                            |
| **Docs links**              | `pnpm check:links`                                                        |
| **Project graph**           | `graph:validate`, `graph:injection`, `graph:determinism`, `graph:check`   |
| **Rust**                    | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` |
| **Boundaries and licences** | `boundaries:rust`, `boundaries:rust:test`, `deny:licenses`, `deny:test`   |
| **Installer**               | `pnpm tauri build`, and uploads the `.exe`                                |

`cargo fmt`, `clippy` and `cargo test` share one job on purpose. Each compiles
the whole Tauri dependency tree; three jobs would compile it three times, and
the wall-clock cost of that is larger than the legibility gained. They are named
steps, so a failure still names itself.

### The order inside the graph job is deliberate

`graph:injection` runs **before** `graph:check`. If a boundary rule has been
disarmed — and
[`architecture-gates.md`](./architecture-gates.md#proving-a-gate-still-works)
lists three ways to do that without touching a rule — then the committed graph
is a faithful record of a check that proves nothing, and `graph:check` goes
green on it. Failing on the disarmed rule is more useful than failing on its
consequence.

`graph:determinism` runs before `graph:check` for the same kind of reason: a
diff can only mean "the tree moved" once the generator is known to be a pure
function of the tree.

## Reproducing a CI failure locally

`pnpm verify` runs every gate in `ci.yml` in the same order, so a green local
verify and a red CI run means an environment difference, not a flake. The usual
causes, in order of likelihood:

1. **An uncommitted file.** CI checks out what you pushed. `git status` first.
2. **`cargo-deny` missing locally.** `cargo install cargo-deny --locked`.
3. **A stale `node_modules`.** CI installs with `--frozen-lockfile`; a local
   tree that drifted from `pnpm-lock.yaml` passes where CI does not.
4. **Running cargo from Git Bash.** See
   [`toolchain.md`](./toolchain.md#windows-build-prerequisites) — coreutils
   ships a `link` that shadows MSVC's `link.exe`, and the error blames rustc.

## Caching

Without it, a Rust build on a Windows runner runs past the 15-minute budget in
`CLAUDE.md` section 13, and a pipeline that routinely overruns gets bypassed.

| Cache                                  | Key                                               |
| -------------------------------------- | ------------------------------------------------- |
| pnpm store                             | `pnpm-lock.yaml`                                  |
| `~/.cargo/registry`, `~/.cargo/git/db` | `Cargo.lock`                                      |
| `target/`                              | `Cargo.lock` + `rust-toolchain.toml`, per profile |
| `cargo-deny` binary                    | `CARGO_DENY_VERSION` in `ci.yml`                  |

Two things there are load-bearing:

- **`target/` is cached per profile.** The `rust` job builds debug, the
  `installer` job builds release. One shared slot means they evict each other
  every run and neither ever gets a warm cache.
- **`~/.cargo/bin` is deliberately _not_ in the registry cache.** A cached
  binary there would shadow a tool this pipeline installs at a pinned version,
  which is how a pipeline ends up running a `cargo-deny` nobody chose.

## Versions come from files, never from the workflow

`.nvmrc` for Node, `packageManager` in `package.json` for pnpm,
`rust-toolchain.toml` for Rust. The workflow names none of them, so CI cannot
drift from what a contributor gets locally — the failure mode `CLAUDE.md`
section 3 pins the toolchain to prevent.

The one exception is `CARGO_DENY_VERSION`, because `cargo-deny` is a developer
tool with no entry in `Cargo.lock`. It is pinned in `ci.yml` and cached on that
key, so bumping it is a deliberate edit rather than a silent `latest`.

## The release gate

`heavy.yml` holds what does not fit in 15 minutes: the losslessness suite and
its media corpus, and the timeline frame-rate benchmark.

**Neither exists yet**, and the workflow says so in its own run summary rather
than reporting a green tick it has not earned:

| Suite                         | Owned by      | Status          |
| ----------------------------- | ------------- | --------------- |
| Losslessness suite + corpus   | #45 (Epic #6) | Not implemented |
| Timeline frame-rate benchmark | #33 (Epic #5) | Not implemented |

It is written now, empty, because the split — what blocks a merge versus what
blocks a release — is a decision from Epic #1 and belongs where the next person
will find it. When #45 adds a `test:lossless` script and #33 adds
`bench:timeline`, the placeholder branch stops being taken and no workflow edit
is needed.

**Until then, a green `heavy.yml` is not evidence of anything.** Do not treat it
as a release sign-off.

The media corpus is never committed (`CLAUDE.md` section 20 rule 12). #45
decides whether it is generated with the bundled FFmpeg at job start or fetched
from a release asset.

## Branch protection

`main` is protected: no direct pushes, linear history, and every check above
required. The settings live in the repository, not in this document — GitHub is
the source of truth and a copy here would be a second version of it that drifts.

## Related

- [`architecture-gates.md`](./architecture-gates.md) — what each gate proves and what it does not
- [`toolchain.md`](./toolchain.md) — the pins and the traps in them
- [`github-workflow.md`](./github-workflow.md) — branches, commits, pull requests
- [`../../CLAUDE.md`](../../CLAUDE.md) sections 8, 13 and 20
