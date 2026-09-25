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
clip (see [`../design/shortcuts.md`](../design/shortcuts.md)).

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

## Split, duplicate, freeze, reverse

From [#35](https://github.com/ismetcahangirov/blinkify/issues/35). Code:
`crates/blinkify-engine/src/project/split.rs`, the `Split`, `Join`,
`Duplicate`, `FreezeFrame` and `SetReverse` edits, and
`apps/desktop/src/timeline/editActions.ts`.

- **Split** (Ctrl+B) cuts the selected clips under the playhead, or every
  clip under it when none is selected. The left half keeps the clip's id.
  The two halves meet in the source at the tick on screen at the cut — the
  evaluator's own reading, rounded down — so every source tick is in exactly
  one half: nothing dropped, nothing shown twice. If the far half's partial
  last frame would make it a frame too long, that sub-frame tail is left off,
  so the halves end exactly where the clip did. Where a frame is a whole
  number of ticks (an MP4's 1/15360 or 1/90000 at common rates), split then
  join is the original byte for byte; where it is not, the joined clip shows
  the same frames and may lack less than a frame of tail.
- **Duplicate** puts each copy right after its original and moves the rest
  of the track along.
- **Freeze frame** cuts the clip at the playhead and inserts a held frame of
  three seconds there, the rest of the track moving later. A held frame is a
  `Freeze { frames }` operation: its length is the frames, not its source.
- **Reverse** plays the selected clips backwards, or forwards again. Trimming
  the timeline start of a reversed clip trims its source's end.

Held and reversed clips **force a full re-encode**. The evaluator records the
reason on the placement (`forced: freeze-frame | reverse`), so the planner
(#39) and the export report (#52) cannot miss it; the timeline marks them
with a band along the top and says so in the label; reversing more than a
minute warns at the point of use. They are executed by the full re-encode
executor (#55), which does not exist yet: until it does, the preview shows
them as a gap and the diagnostic view says they are not previewed. They can
be edited now; they cannot be exported until #55.

### The keyframe indicator

A cut is lossless when the frame on screen at the cut is a keyframe a copy
can start from. The timeline asks the engine (`cut_point_at`) once the
playhead rests: the engine reads the keyframe index (#24) around that
position and answers with `CutPoint` — lossless or not, an open-GOP keyframe
named as such, and the keyframes before and after in timeline frames. The
toolbar states it: _Keyframe: a cut here is lossless_, or what a cut there
costs.

**Snap cuts to keyframes** is off by default. When it is on and the playhead
is off a keyframe, a split moves to the nearest keyframe inside the clip, and
the timeline says where it landed — _Cut moved to the keyframe 5 frames
later_. Moving a user's cut silently would be a correctness change disguised
as a convenience.
