# ADR-0017 — Generate the attribution document ourselves, from local sources only

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#73](https://github.com/ismetcahangirov/blinkify/issues/73)

## Context

MIT, BSD, ISC and Apache-2.0 each ask for the copyright notice and the licence
text to travel with the binary. The Windows build resolves 330 crates, the
interface bundles 57 npm packages, and the FFmpeg sidecar carries fifteen
libraries of its own. [`THIRD_PARTY.md`](../../THIRD_PARTY.md) records _why_
each direct dependency ships; it is not attribution, and a hand-maintained list
of several hundred packages goes stale on the first `cargo update`.

So the document has to be generated, and the generator has to satisfy four
constraints at once:

- **One document** for the Rust binary, the renderer bundle and the sidecar. A
  user looking for a notice does not know which half a package came from.
- **No network.** CI must not depend on a request per crate, and the result
  must not depend on what a remote service answered that day.
- **Deterministic**, so a committed copy can be gated the way the project graph
  is: a diff means the dependency set moved (`CLAUDE.md` section 14).
- **Fail, never skip.** A partial attribution file reads as complete, which is
  worse than none.

## Decision

A small Node tool in [`tools/attribution/`](../../tools/attribution/) writes
`apps/desktop/src-tauri/THIRD-PARTY-NOTICES.txt`, which is committed, bundled
by the installer as a resource, and shown under **Help ▸ Third-party notices**.

- **Rust**: every package `cargo metadata --all-features --locked
--filter-platform x86_64-pc-windows-msvc` resolves, minus the workspace's own
  crates — build-time and test crates included, because over-attribution costs
  a page and under-attribution is a compliance failure. Each crate's texts are
  read from its own unpacked source in the local cargo registry
  (`manifest_path`), which is where the licence files the registry metadata
  does not expose actually live.
- **Renderer**: the production dependency tree of `apps/desktop`, followed
  through `node_modules` as pnpm laid it out from the lockfile. Workspace
  packages are walked through, not listed; `devDependencies` are not followed.
- **Bundled components** with no package metadata — FFmpeg and the libraries
  built into it, the RNNoise model, the typefaces, the ported smart-cut code —
  are described in `components.json`, with their upstream licence texts
  committed beside it. FFmpeg's version and configure line are read from the
  sidecar's own lock and configure files, so a sidecar bump cannot leave its
  LGPL notice behind.
- **Texts are recognised by their wording**, not by file name. A file that
  matches no licence but is licence-named (a `COPYRIGHT` file, a "dual-licensed
  under…" note) is carried as a notice rather than dropped.
- **Choices**: where a package offers `A OR B`, Blinkify takes the option whose
  text the package itself ships, and among those prefers MIT, then Apache-2.0,
  then the rest of a fixed order written into the document. The option taken
  is recorded against every package.
- **A package that ships no licence file** gets the standard SPDX text,
  committed under `tools/attribution/licences/`, with the authors it declares
  as the copyright line, and is marked as such. A package whose licence has no
  file and no standard text, or that declares no licence at all, fails the
  build, naming the package.

`pnpm attribution:check` fails when the committed document differs from a fresh
generation. See
[`../engineering/architecture-gates.md`](../engineering/architecture-gates.md#the-attribution-gate).

## Alternatives considered

### `cargo-about` — rejected

The obvious candidate, and a sound tool: MIT OR Apache-2.0, maintained (0.9.2,
August 2026), build-time only, so `CLAUDE.md` section 10 would apply to it
lightly. Its licence was checked before its API.

Rejected for three reasons, each sufficient:

- **It covers half the document.** It reads Cargo and nothing else. The
  renderer bundle and the FFmpeg sidecar would still need a second tool and a
  merge step — which is most of the code written here anyway, with a
  handlebars template and a second output format in between.
- **Its accuracy depends on the network.** By default it queries external
  sources (ClearlyDefined) for licence information. Its `--offline` mode
  exists, and its own documentation says that in it "some ambiguous/complicated
  license files might be missed" and it falls back to default licence texts,
  "losing eg. copyright or other unique information". The mode CI needs is the
  mode it describes as degraded.
- **It is one more pinned binary** to `cargo install` and cache in CI, as
  `cargo-deny` already is, for a job that reads files `cargo metadata` has
  already put on disk.

### `pnpm licenses list` or `license-checker` for the renderer — rejected

`pnpm licenses list --prod --json` reports each package's licence and path, but
not its text, and its output format has changed between pnpm majors;
`license-checker` is unmaintained. Walking `node_modules` from the application's
own `package.json` is forty lines and follows exactly what the bundle can reach.

### Keep extending the hand-written tables in `THIRD_PARTY.md` — rejected

That is the failure mode #73 exists to remove. A partially hand-written
attribution file is worse than none, because it reads as complete.

### Fetch missing texts from each crate's repository — rejected

It would turn the handful of crates that ship no licence file into a request
per crate on every build, and make the output depend on a third party's
default branch on the day. The standard SPDX text, marked as such, is the
honest offline answer.

### Bundle the text into the renderer with a Vite `?raw` import — rejected

It would work offline too, but the notices would then exist only inside a
JavaScript bundle. As a resource file they sit in the installation folder,
readable without starting the application, and the dialog shows the very file
that shipped.

## Consequences

### What this makes easy

- Adding a crate or a renderer package fails CI until the document is
  regenerated, and the failure names the package.
- The notice obligation is met by the installed application, offline.
- The FFmpeg notice — LGPL text, source commit, configure line, the statement
  that the sidecar is a separate, replaceable program — travels with the
  binary, as ADR-0002 requires.

### What this makes hard

- A new licence wording the classifier does not recognise is carried as a
  notice rather than recognised; if it was the only text for the licence taken,
  the standard text is used as well. The classifier is extended in
  `attribution.mjs` when that happens.
- The texts of the sidecar's libraries are committed copies of upstream's
  current files, not extracted from the pinned build image. When a library
  changes licence upstream, they are updated by hand.

### What we accept

- A 600 KB generated text file in the repository that changes whenever the
  dependency set does. That diff is the point.
- Build-time and test crates appear in the document although they do not ship.
