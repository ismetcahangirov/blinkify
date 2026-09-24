# The project lifecycle

From [#54](https://github.com/ismetcahangirov/blinkify/issues/54). Code:
`crates/blinkify-engine/src/project/session.rs` and `recent.rs` (the rules),
`apps/desktop/src-tauri/src/lifecycle.rs` (the commands and the window), and
`apps/desktop/src/project/ProjectLifecycle.tsx` (the dialogs and keys).

## What the user can do

| Action      | From                                | What happens                                                                  |
| ----------- | ----------------------------------- | ----------------------------------------------------------------------------- |
| New project | File menu, Ctrl+N                   | Name, and either _match the first clip_ (the default, #57) or chosen settings |
| Open        | File menu, Ctrl+O, Recent, Explorer | One path for all of them; a newer schema is refused whole, with its reason    |
| Save        | File menu, Ctrl+S                   | Asks where if the project was never saved                                     |
| Save as     | File menu, Ctrl+Shift+S             | The new file becomes the project's                                            |
| Close       | File menu, or closing the window    | Asks _Save / Don't save / Cancel_ when there is unsaved work                  |
| Recover     | At launch                           | Offers unsaved work from a session that did not end cleanly, with both times  |

At launch Blinkify opens the project it was started with (a double-click in
Explorer); failing that it offers any unsaved work to restore; failing that it
starts a new untitled project, so there is always somewhere to import into.

## Dirty is exact

Serialisation is deterministic (#32), so the session keeps the text of the
project as last opened or saved, and a project is **dirty exactly when its
text differs**. Undoing back to the saved state makes it clean again. The app
bar shows a dot after the name, and the window title a leading ●, while it is
dirty.

## Autosave never touches the project file

Unsaved work is written to a **recovery file**, never to the project:

- beside the project — `Trip.blinkify.recovery` next to `Trip.blinkify`;
- for a project never saved, in the application's data folder under
  `recovery\`.

It is written to a temporary name and renamed into place, so a crash while
writing leaves the previous recovery whole. It is written every 60 seconds,
and at once after an edit that is expensive to redo — a ripple delete, a move
or delete of several clips, a split, a freeze frame, a detach, removing a
track, changing the sequence settings. If the text is unchanged since the last
autosave, nothing is written; if the project is clean, the recovery file is
removed. An explicit save writes the project file and removes the recovery.

The engine test `autosave_writes_beside_the_project_and_never_touches_it`
hashes the project file before and after autosaves and asserts it unchanged.

## Recovery

At launch, the recovery file beside each recent project is offered if it is
**newer** than the project (or the project is gone), and every never-saved
recovery is offered. The offer shows the project's name, **when the unsaved
work was written, and when the project was last saved** — the user is not
asked to guess. _Restore_ reopens the work, dirty, keeping the recovery file
until it is saved; _Discard_ deletes the recovery file.

## Closing

Closing a project drops its undo history (#37) and closes its preview, whose
decode processes exit before the command returns (the player's teardown,
asserted in `closing_a_player_terminates_its_decode_processes`). Closing the
window on unsaved work is stopped by the shell, which asks the renderer to ask
the user; _Don't save_ also deletes the recovery file — the user chose.

Recent projects are a preference, kept in the application's settings folder
(`recent-projects.json`, ten at most, case-insensitive paths), never in a
project file.
