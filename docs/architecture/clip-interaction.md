# Clip interaction

From [#34](https://github.com/ismetcahangirov/blinkify/issues/34). Code:
`apps/desktop/src/timeline/interaction.ts` (what a drag would do),
`TimelineCanvas.tsx` (the pointer), and `crates/blinkify-engine/src/project/trim.rs`
with the `TrimEdge`, `Roll` and `RippleDelete` edits (what a drag does).

## A drag is previewed, then committed once

```
press ──▶ move past 3 px ──▶ dragTo() on every move ──▶ overlay ghost
                                      │
                           release ───┴──▶ one Edit ──▶ engine ──▶ one history entry
                           Escape  ─────▶ nothing
```

Nothing touches the graph while the pointer moves. The overlay draws where
the clips would land, the line an edge snapped to, and a warning bar where a
bound stopped the drag; the clips under it are not repainted. On release,
exactly one edit is sent, so one drag is one undo entry however many pointer
moves it took.

## What the pointer does

| Over                | Drag                | With                                            |
| ------------------- | ------------------- | ----------------------------------------------- |
| a clip's body       | moves the selection | onto another track of the same kind             |
| a clip's end (6 px) | trims that edge     | **Shift** ripples, **Ctrl** rolls a shared edge |
| the ruler           | scrubs the playhead |                                                 |
| anything            |                     | **Alt** suspends snapping, **Escape** abandons  |

Clicking selects one clip, **Ctrl**-click adds or removes one, **Shift**-click
selects the run of clips on that track from the last one selected, and a
click on empty track clears the selection. **Delete** removes the selection,
**Shift+Delete** removes it and closes the gaps, **Ctrl+A** selects every
clip (see [`../design/keyboard-shortcuts.md`](../design/keyboard-shortcuts.md)).

A drop onto a track of the other kind, or onto another clip, is refused in
the preview (red ghost) and by the engine.

## Snapping is in pixels

An edge snaps to a clip edge, the playhead or frame 0 when it comes within
**8 screen pixels** of it — a distance on screen, so the pull feels the same
at every zoom. A threshold in time would snap from across the screen when
zoomed out and never when zoomed in. Alt suspends it for the drag.

## Time is frames in, ticks out

The renderer computes a drag in whole sequence frames and sends frames. The
engine converts them to source ticks exactly (`project::trim`):

- **Start edge**: the in-point moves by the frames' ticks rounded up, then by
  a tick at a time if a sub-frame remainder would change the length — the far
  edge never moves.
- **End edge**: the out-point is set to the whole frames the clip should
  last, rounded down, so it lasts exactly that.
- **Bounds**: the source's extent (from the probe), one frame of length, and
  — unless the trim ripples — the neighbouring clips. A bound that stops the
  trim is reported as `clamped` and the timeline says so.
- **Ripple trim** keeps the clip's start and moves every later clip on the
  track by the change in length. **Ripple delete** moves every later clip by
  the length removed before it. **Roll** moves the cut between two touching
  clips and neither outer edge.

Typing an in point, out point or duration in the inspector sends the same
`TrimEdge` edit a handle drag sends, so the two cannot disagree.

The preview's bounds are approximate — floating point, for display — and
only show the user the limit while dragging. The engine's are exact and are
the ones that apply.
