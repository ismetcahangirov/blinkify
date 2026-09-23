# Keyboard shortcuts

The keys Blinkify answers to. Where CapCut's desktop editor has a shortcut for
the same action, Blinkify uses the same key, so a user arriving from CapCut does
not re-learn their hands — the same reasoning as the layout
([`capcut-layout-reference.md`](./capcut-layout-reference.md)).

This document grows as features land. The full CapCut-compatible map, and the
rule for resolving a clash, is
[#38](https://github.com/ismetcahangirov/blinkify/issues/38).

## Transport

From [#28](https://github.com/ismetcahangirov/blinkify/issues/28). Active
whenever a preview is open.

| Key   | Action                 | CapCut |
| ----- | ---------------------- | ------ |
| Space | Play / pause           | Same   |
| ←     | Previous frame         | Same   |
| →     | Next frame             | Same   |
| Home  | Jump to the start      | —      |
| End   | Jump to the last frame | —      |

## Shuttle

From [#29](https://github.com/ismetcahangirov/blinkify/issues/29). The J/K/L
keys every editor shares, less reverse play, which v1 does not have.

| Key | Action                                             | CapCut |
| --- | -------------------------------------------------- | ------ |
| L   | Play; pressed again while playing, double speed    | —      |
| K   | Pause                                              | —      |
| J   | Back one second, by real frames; held, keeps going | —      |

The player's **playhead** under the video is a slider: dragging it scrubs, and
with it focused, ← and → move it one frame and Home and End to either end —
the slider's own keys, which the transport does not take from it.

Rules:

- **Space plays even from a focused button.** In an editor Space means play,
  whatever was clicked last; the key press is taken before the button can turn
  it into a click.
- **A key belongs to the control it is typed into.** In a text field, a slider,
  a list or a combo box, the arrows, Home, End and Space do what that control
  does, and the transport does not see them.
- **Modifiers are not transport.** Ctrl, Alt or the Windows key with any of
  these keys is left for other shortcuts.
- **Holding Space does not stutter.** A repeated Space is ignored; each arrow
  repeat steps another frame.

The tooltip of every transport control names its key, from the same table in
code (`TRANSPORT_SHORTCUTS`), so the two cannot disagree.
