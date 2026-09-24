# Edits and history

From [#37](https://github.com/ismetcahangirov/blinkify/issues/37). Why it is
built this way, and what was rejected, is
[ADR-0007](../decisions/ADR-0007-edits-are-invertible-changes-in-the-engine.md).

## The path of one edit

```
renderer                         shell (Tauri)             engine
────────                         ─────────────             ──────
useProjectStore.edit(edit)
  + { selection, playhead }  ──▶ edit_project ──────────▶ Document::apply
                                                            compile → [Change]
                                                            apply each → [inverse]
                                                            evaluate (#30) or roll back
                                                            push history entry
                                 refresh preview plan ◀──── Ok(context after)
◀── EditOutcome { view, context }
set view, selection; seek if the playhead moved
```

`view` is the whole `ProjectView`: the graph, the evaluated `Timeline` the
timeline draws from, the history list, and the source status.

## Where the graph can change

Only inside `Document`. It owns the `Project` privately and exposes
`project() -> &Project`; the shell holds a `Document`, so it has no way to
reach a `&mut`. The compile-fail doctest at the top of
`crates/blinkify-engine/src/project/edit.rs` keeps it that way, and
`pnpm edits:check` covers what the compiler cannot see (see
[`../engineering/architecture-gates.md`](../engineering/architecture-gates.md)).

On the renderer side the view is typed `DeepReadonly`, and only
`project.store.ts` sends the graph-changing commands.

## Primitive changes

| Change          | Inverse                                     |
| --------------- | ------------------------------------------- |
| `Name`          | `Name` with the previous name               |
| `Settings`      | `Settings` with the previous settings       |
| `Source(id, r)` | `Source(id, previous)` — `None` removes     |
| `InsertTrack`   | `RemoveTrack` by id                         |
| `RemoveTrack`   | `InsertTrack` at the index it was removed   |
| `InsertClip`    | `RemoveClip` by id                          |
| `RemoveClip`    | `InsertClip` at the exact index it occupied |
| `ReplaceClip`   | `ReplaceClip` with the clip it replaced     |

`InsertClip` without an index places the clip in timeline order; the inverse
of a removal always carries the exact index, so a clip list that was not in
order comes back as it was. That is what makes undo byte-identical rather
than merely equivalent.

## Edits

Each `Edit` variant compiles to primitive changes:

- **Move** — every moving clip is removed first, then each is inserted, so
  clips moving past one another never collide half way. A clip may not move
  to a track of another kind.
- **Trim** — one `Trim` operation replaces all of a clip's trims, at the
  position of the first; a changed start re-inserts the clip in order.
- **Speed** — one `Speed` operation replaces all of a clip's speed changes;
  `1/1` removes it.
- **Add clip** — the next clip id, placed in order, and selected.
- **Remove clips** — removed, and taken out of the selection.

After the changes apply, the graph is evaluated. If it evaluated before and
does not now, the changes are reverted and the evaluator's reason — "clips 1
and 2 overlap on track 1" — is the error. A project that opened unevaluable
is not held hostage: edits to it are accepted, and it is refused again only
once it has evaluated.

## History

- An entry is a label, the inverse changes, and the context (selection and
  playhead) before and after.
- A new entry discards everything that could be redone.
- The depth is 500 entries by default (`DEFAULT_DEPTH`); past it the oldest
  entry is forgotten and can no longer be undone.
- `begin_gesture` / `end_gesture` bracket a gesture into one entry. If every
  change in the gesture replaces a clip, only the first inverse per clip is
  kept, so an entry stays the size of one change however long the drag.
- Closing a project drops its `Document`, and with it the history.

## Tests

- `any_sequence_of_edits_fully_undone_restores_the_graph_byte_for_byte` runs
  300 random sequences of up to 40 edits and undos, and checks after every
  step that a refused edit changed nothing, that the graph evaluates, that
  undoing everything returns the original bytes, and that redoing everything
  returns the last state's bytes.
- The gesture, depth, redo-discard, selection-restore and refusal cases each
  have their own test in `edit.rs`.
