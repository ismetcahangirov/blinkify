# Keyboard shortcuts

The keys Blinkify answers to. Where CapCut's desktop editor has a shortcut for
the same action, Blinkify uses the same key, so a user arriving from CapCut does
not re-learn their hands — the same reasoning as the layout
([`capcut-layout-reference.md`](./capcut-layout-reference.md)). Where Blinkify
differs, or CapCut has no key, the table says why.

The source of truth is the registry,
`apps/desktop/src/shortcuts/shortcuts.ts` ([#38](https://github.com/ismetcahangirov/blinkify/issues/38)).
The in-app reference (**Ctrl+/**, or Help ▸ Keyboard shortcuts…) is generated
from it, and a test fails if this page and the registry disagree.

## Transport

Active whenever a preview is open.

| Action                                  | Keys  | CapCut                                                                        |
| --------------------------------------- | ----- | ----------------------------------------------------------------------------- |
| Play / pause                            | Space | Same                                                                          |
| Previous frame                          | ←     | Same                                                                          |
| Next frame                              | →     | Same                                                                          |
| Jump to the start                       | Home  | No CapCut shortcut. Home and End are where every other editor puts them.      |
| Jump to the last frame                  | End   | No CapCut shortcut. Home and End are where every other editor puts them.      |
| Back one second; held, keeps going      | J     | CapCut plays in reverse. Blinkify has no reverse play in v1, so J steps back. |
| Pause                                   | K     | Same                                                                          |
| Play; again while playing, double speed | L     | Same                                                                          |
| Set the in point of the loop range      | I     | No CapCut shortcut. I is the in point in Premiere Pro and DaVinci Resolve.    |
| Set the out point of the loop range     | O     | No CapCut shortcut. O is the out point in Premiere Pro and DaVinci Resolve.   |

The in and out points are shown beside the loop button, and Loop plays between
them — the whole timeline without them. They are a preview setting, not part of
the project.

## Editing

| Action                                   | Keys                          | CapCut                                                                                                        |
| ---------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------- |
| Undo                                     | Ctrl+Z                        | Same                                                                                                          |
| Redo                                     | Ctrl+Shift+Z, Ctrl+Y          | Same for Ctrl+Shift+Z. Ctrl+Y is kept as well, because it is Windows' own redo.                               |
| Copy the selected clips                  | Ctrl+C                        | Same                                                                                                          |
| Paste at the playhead                    | Ctrl+V                        | Same key. The paste is inserted: later clips on every unlocked track move along, so nothing goes out of sync. |
| Duplicate the selected clips             | Ctrl+D                        | Same                                                                                                          |
| Delete the selected clips                | Delete, Backspace             | Same                                                                                                          |
| Ripple delete: delete and close the gaps | Shift+Delete, Shift+Backspace | No CapCut shortcut: CapCut's main track closes gaps on its own. Shift+Delete is Premiere Pro's.               |
| Select every clip                        | Ctrl+A                        | Same                                                                                                          |
| Split at the playhead                    | Ctrl+B                        | Same                                                                                                          |

A paste never splits a clip: with the playhead inside a clip on a track the
copies go to, it is refused and says so. Copied clips that have been deleted
since cannot be pasted.

## Timeline

| Action                         | Keys           | CapCut                                                           |
| ------------------------------ | -------------- | ---------------------------------------------------------------- |
| Zoom in                        | Ctrl+=, Ctrl++ | Same                                                             |
| Zoom out                       | Ctrl+-         | Same                                                             |
| Fit the whole timeline in view | Shift+Z        | No CapCut shortcut. Shift+Z is DaVinci Resolve's zoom to fit.    |
| Turn snapping on or off        | N              | No CapCut shortcut: CapCut has a button. N is DaVinci Resolve's. |

Alt while dragging does the opposite of the snapping setting, for that drag.

## Application

| Action                  | Keys         | CapCut                                                                                 |
| ----------------------- | ------------ | -------------------------------------------------------------------------------------- |
| New project             | Ctrl+N       | Same                                                                                   |
| Open a project          | Ctrl+O       | Same                                                                                   |
| Save                    | Ctrl+S       | Same                                                                                   |
| Save as                 | Ctrl+Shift+S | No CapCut shortcut: CapCut saves to its own library. Ctrl+Shift+S is Windows' save as. |
| Export                  | Ctrl+E       | Same                                                                                   |
| Show keyboard shortcuts | Ctrl+/       | No CapCut shortcut. Ctrl+/ is the reference in most Windows applications.              |

Export is bound now so the key cannot be given to anything else; until the
export (Epic #6) exists it says that export is not available yet.

## Which control a key belongs to

- **Typing wins.** In a text field every key is the field's. A Space that starts
  playback while a project is being named is the classic bug this rule exists
  for.
- **A control keeps its own keys.** A slider, list, menu or tab list uses the
  arrows, Home, End and Space itself; the player's playhead slider steps a
  frame with the arrows as a slider, not as the transport.
- **A dialog is modal.** Nothing behind an open dialog reacts to the keyboard.
- **Space plays even from a focused button.** In an editor Space means play,
  whatever was clicked last; the key press is taken before the button can turn
  it into a click.
- **Holding a toggle does not stutter.** A repeated Space, K or L is ignored;
  arrows, J, undo and zoom repeat.
- **A key with nothing to act on is left alone.** Ctrl+C with no clip selected,
  or the transport with no preview open, is not taken from the browser.

## Keys Blinkify will not bind

Checked by the type of a chord, so binding one fails the build:

- What Windows handles before an application sees it: Alt+Tab, Shift+Alt+Tab,
  Alt+F4, Alt+Esc, Alt+Space, Ctrl+Esc, Ctrl+Shift+Esc, and F12 (the system
  debugger key).
- Every Ctrl+Alt combination. On the keyboards that type ə, ş and ğ, Ctrl+Alt
  is AltGr, and a shortcut there would eat a letter.

A chord bound twice fails the build too: the registry is keyed by chord, and
TypeScript rejects an object literal with two properties of one name.
Customisable shortcuts are out of scope for v1; the registry is data, so a user
layer over it is an addition rather than a rewrite.

## Timeline pointer

From [#34](https://github.com/ismetcahangirov/blinkify/issues/34); how each
works is [`../architecture/clip-interaction.md`](../architecture/clip-interaction.md).

| Pointer                       | Action                                    |
| ----------------------------- | ----------------------------------------- |
| Click a clip                  | Select it                                 |
| Ctrl+click                    | Add it to, or take it from, the selection |
| Shift+click                   | Select the run of clips on its track      |
| Drag a clip                   | Move the selection, to another track too  |
| Drag a clip's edge            | Trim it                                   |
| Shift while trimming          | Ripple: later clips follow                |
| Ctrl on an edge shared by two | Roll the cut between them                 |
| Alt while dragging            | The opposite of the snapping setting      |
| Escape while dragging         | Abandon the drag                          |
| Ctrl+wheel                    | Zoom about the pointer                    |
| Wheel, Shift+wheel            | Scroll the tracks, scroll sideways        |
