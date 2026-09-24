# ADR-0007 — Edits are invertible changes, applied by the engine

- **Status**: Accepted
- **Date**: 2026-09-24
- **Context issue**: [#37](https://github.com/ismetcahangirov/blinkify/issues/37)

## Context

Every timeline operation in Epic #5 has to be undoable, and undo followed by
redo has to return the edit graph byte for byte. #37 adds three constraints:

- **Complete.** No mutation may bypass the stack. A change the history did not
  see makes the next undo silently do the wrong thing — worse than no undo.
  The issue asks for this to be enforced by the type model and a boundary
  rule, not by review.
- **Cheap on a large project.** A history of whole-graph snapshots grows with
  the project times the number of edits.
- **Gesture-aware.** A drag or a slider is one entry, bracketed explicitly.

Before this decision the renderer held a copy of the graph, changed it, and
sent the whole thing back (`update_project`). Nothing recorded what changed.

## Decision

**The engine owns the open graph and applies every edit.** The open project
is a `Document` (`blinkify_engine::project::edit`), which holds its `Project`
privately and exposes only `&Project`. The only mutable borrow of an open
graph is inside `Document::apply`, so the compiler, not a convention, routes
every change through the history. The shell holds a `Document`; the
`update_project` command is gone.

**An edit is data, compiled to primitive invertible changes.** The renderer
sends an `Edit` — "move these clips there", "trim this clip to that range" —
with its selection and playhead. The document compiles it against the current
graph into a short list of primitive `Change`s (insert or remove a clip,
replace a clip, insert or remove a track, set a source, the name or the
settings). Applying a primitive change returns its exact inverse. The history
entry stores the inverses — the payload is proportional to what changed —
and undo applies them backwards, which returns the forward changes for redo.

**An edit that breaks the graph is rolled back.** After applying, the
document evaluates the graph (#30). If an edit would make a graph that
evaluated stop evaluating — two clips overlapping, a trim running backwards —
its changes are reverted and the evaluator's reason is returned. The preview
and the export never see an unresolvable graph produced by an edit.

**Gestures are explicit.** `begin_gesture` and `end_gesture` bracket edits
that form one entry. Within a gesture that only replaces clips, the first
inverse per clip is kept and later ones dropped, so a slider dragged through
a hundred values is one entry of one change.

**Selection and playhead travel with the edit.** Each entry keeps the
context before and after; undo returns the before, redo the after, and the
renderer restores both.

**Boundary rule.** `pnpm edits:check` fails on Rust code outside
`project/` that takes `&mut Project` or binds a mutable `Project`, and on any
renderer file other than the project store that sends a graph-changing
command. `pnpm edits:check:test` proves it fires.

The default history depth is **500 entries**, configurable per document
(`Document::with_depth`). The history is per document, so closing a project
drops it.

## Alternatives rejected

- **Snapshots of the whole graph per entry.** Simplest to get right, and
  byte-identical restoration is free. Rejected because the cost is the graph
  size times the history depth: a 200-clip project with 500 entries holds
  100,000 clip copies. #37 names this explicitly.
- **A hand-written inverse per high-level edit.** Each of ripple delete, roll,
  split and detach would need its own undo, and each is a new place for undo
  to be subtly wrong. Composing edits from a handful of primitive changes
  means the inverse logic is written once, per primitive, and property-tested
  over random edit sequences.
- **Keep the graph in the renderer and diff it on each change.** The renderer
  would compute undo and redo; the engine would receive whole graphs. Rejected
  because the engine owns media decisions (`CLAUDE.md` section 2), because
  locking (#36) must be enforced in the command layer where a keyboard
  shortcut cannot route around it, and because two authoritative copies of
  the graph drift.
- **Immer-style patches in the renderer.** A good tool for renderer state,
  but it answers the same question on the wrong side of the IPC boundary and
  adds a dependency to do it.
- **Coalescing by time** (merge edits less than N ms apart). Rejected by #37
  itself: a pause mid-drag splits the entry, and two quick unrelated edits
  merge. The bracket is explicit.

## Consequences

- Every new timeline operation is a new `Edit` variant and a compile step —
  never a new mutation path. Its undo comes from the primitives.
- The renderer receives the evaluated `Timeline` in the `ProjectView` and
  draws from it; it never interprets operations (`pnpm evaluator:check`).
- Each committed edit crosses the IPC boundary once with the whole view.
  That is proportional to project size, not to the edit, and is paid once per
  release of a drag, not per pointer move. If it ever shows in a profile, the
  view can be sent as a diff without changing this decision.
