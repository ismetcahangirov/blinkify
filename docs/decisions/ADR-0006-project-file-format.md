# ADR-0006 — The project file is versioned, deterministic JSON over integer time

- **Status**: Accepted
- **Date**: 2026-09-23
- **Context issue**: [#32](https://github.com/ismetcahangirov/blinkify/issues/32)

## Context

The project file is the one artefact of Blinkify that users keep. Media is
theirs already; caches can be rebuilt; the edit — what was cut, where it was
placed, what was done to it — exists nowhere else. Two failures are worse than
any other an editor can ship:

- **A later build cannot open an earlier project.** The user's work is gone,
  and the fix ships after they have lost it.
- **The file says one thing and plays another.** A cut placed on a frame and
  stored in floating-point seconds comes back one frame off after enough
  arithmetic. Nothing reports it.

Three further requirements come from #32: saving an unchanged project changes
no byte (no spurious git diff, no "unsaved changes" prompt), the TypeScript
side is generated from the Rust model, and nothing in the file can refer to a
proxy, a render or a temporary file.

## Decision

The project file (`.blinkify`) is **pretty-printed JSON** with a
`schemaVersion` field from the first version, written from the Rust model in
`blinkify_engine::project`:

- **Time is integer ticks of a rational time base.** A trim is in the source
  stream's own time base; a position on the timeline in the sequence's, one
  tick per frame. Conversions go through `crate::time::rescale` in 128-bit
  integers with the rounding direction stated. Speed is a rational, and a
  sped-up clip's ticks are rescaled through the source time base multiplied
  by the speed, so no conversion goes through a float.
- **Deterministic output.** Maps are `BTreeMap`s (ordered keys), fields are
  written in declaration order, the file ends with one newline, and
  `serde_json`'s `float_roundtrip` parser is enabled so a gain or loudness
  target reads back as the same double. A property test over 2,000 generated
  graphs asserts save → load → save is byte-identical; without
  `float_roundtrip` it fails on the first case.
- **Migrations run on JSON, in order.** `project::migrate` holds one
  `fn(Value) -> Result<Value>` per schema step. A file is migrated from its
  version to the current one before it is typed. A file from a newer build is
  refused with a message naming both versions — never read on a best-effort
  basis. Every released schema version has a committed fixture under
  `crates/blinkify-engine/tests/fixtures/projects/`, and the test suite fails
  if one is missing.
- **Sources are fingerprinted, not just named.** A source reference stores its
  path, size, modification time, and a SHA-256 over the size and the first
  and last mebibyte (the same sampling as the cache key, #24). Identity is
  size and hash; the modification time is recorded but not compared, because
  copying a file to another disk changes it and nothing else.
- **Only originals.** A `SourceRef` can only be built from an `ExportSource`,
  which only ever holds an original file (#26). Its fields are private. A
  `compile_fail` doc test proves a proxy cannot be passed.

## Alternatives considered

### Floating-point seconds — rejected

What most hobby editors store, and the reason they drift. `0.1 + 0.2` is not
`0.3`; a clip trimmed, moved and sped up accumulates the error of each step,
and a 29.97 fps frame is not representable in binary at all. The issue names
this as the standard reason editors drift by a frame.

### A binary format, or SQLite — rejected

Smaller and faster to load, and neither matters at the size of an edit graph
— thousands of clips are kilobytes of JSON. What would be lost: a user can
read and diff a JSON project, put it in git, and recover it by hand when
something goes wrong. SQLite also stores pages, not a canonical text, so "two
saves are byte-identical" stops being something a test can assert.

### Migrations on the typed Rust structs — rejected

Keeping `ProjectV1`, `ProjectV2`, … as types and converting between them
means every old version of every nested type lives in the code forever, and a
field renamed in a shared type changes the meaning of an old version
silently. On `serde_json::Value`, a migration is a function of the file as it
was written, and the old types can be deleted.

### Identity by path, or by path and modification time — rejected

A file replaced at the same path is a different source, and every cache keyed
on it has to know (#32). A path cannot tell, and a modification time is
preserved by some copy tools and not others. Hashing the whole file would be
exact and would read a two-hour recording end to end on every open.

### Serde's `#[serde(default)]` as the migration mechanism — rejected

Adding a field with a default reads old files without a migration, and is
fine for that one case. It cannot rename, restructure or change a unit, and
the first time it is not enough there would be no registry to put the real
migration in. The registry exists from version 1, empty.

### A property-testing crate (`proptest`, `quickcheck`) — rejected for now

The generator the round-trip test needs is about forty lines over a seeded
xorshift, and a fixed seed reproduces a failure exactly. `CLAUDE.md`
section 10: a dependency for forty lines is not worth the supply chain.
Shrinking would be the reason to revisit.

## Consequences

### What this makes easy

- Opening a project from any earlier build, and proving it: add the fixture,
  the test opens it.
- A project under version control diffs as the edit changed, and only then.
- The evaluator (#30) and the export planner (Epic #6) read one model with
  exact time.

### What this makes hard

- Every change to a serialised type is a schema change: bump
  `SCHEMA_VERSION`, write the migration, commit the fixture. That is the
  point, and it is also work.
- Integer time needs care at every conversion — which way to round is a
  decision at each call site, not a default.

### What we accept

- A false "same source" needs a file edited without its size or either end
  changing. The cache key already accepts this trade.
- A hand-edited project file that migrates and validates is trusted as the
  user's intent, including a path to a proxy typed in by hand. The type model
  stops the application from writing one; it cannot stop a text editor.
