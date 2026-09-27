# Crop controls and reframe

From [#130](https://github.com/ismetcahangirov/blinkify/issues/130) and
[#132](https://github.com/ismetcahangirov/blinkify/issues/132), Epic
[#125](https://github.com/ismetcahangirov/blinkify/issues/125). The crop
model is
[ADR-0019](../decisions/ADR-0019-a-crop-is-a-rectangle-per-clip-in-display-pixels.md);
presets and reframe are
[ADR-0022](../decisions/ADR-0022-reframe-is-one-edit-of-settings-and-per-clip-crops.md).
Code: `project/crop.rs` and `project/reframe.rs` in the engine,
`export/cost.rs` for the statements, `inspector/CropSection.tsx` and
`project/ReframeDialog.tsx` in the renderer.

## Who decides what

| Question                               | Answered by                                    |
| -------------------------------------- | ---------------------------------------------- |
| A preset's rectangle on a picture      | `crop::centred(shape, aspect)`                 |
| Whether a crop already has a shape     | `crop::has_aspect(rect, shape, aspect)`        |
| What the controls need of a source     | `CropFrame::of(shape)` → `ProjectView.frames`  |
| Whether a rectangle can crop a picture | `crop::check` (the edit layer, ADR-0019)       |
| What the export does to selected clips | `cost::clips_cost(plan, clips)`                |
| What a reframe decides                 | `reframe::reframe` (compiled edit)             |
| What a reframe costs                   | `cost::reframe_impact(reframe, before, after)` |

The renderer phrases the answers (`inspector/crop.ts`, `project/reframe.ts`)
and computes none of them (`CLAUDE.md` section 2).

## Presets

`Aspect` is `source`, `16:9`, `9:16`, `1:1` or `4:5`. `centred` fits the
largest rectangle of that shape on screen — the pixel aspect is part of it —
to the displayed picture, rounds each side to the nearest step of its axis'
chroma grid (`crop::alignment`: 2 × 2 for 4:2:0, swapped by a quarter turn
for 4:2:2), and centres it on the grid:

| Picture                  | 9:16                 | 1:1                   | 16:9                    |
| ------------------------ | -------------------- | --------------------- | ----------------------- |
| 1920 × 1080              | 608 × 1080 at 656, 0 | 1080 × 1080 at 420    | whole — no crop         |
| 1080 × 1920 (phone, 90°) | whole — no crop      | 1080 × 1080 at 0, 420 | 1080 × 608 at 0, 656    |
| 1919 × 1079 (odd, 4:2:0) | 606 × 1078 at 656, 0 | 1078 × 1078           | whole — within rounding |

A picture of the shape to within the rounding is returned whole: no crop, so
no re-encode. `source` is always the whole picture. A picture too small to
hold the shape at 16 pixels a side has no preset of it.

## The crop section

`ProjectView.frames` carries, for every source with pictures, its displayed
size, its grid and every preset's rectangle. A source not listed is offline:
its clips' fields are disabled, and reset still works (it needs no shape).

| Control                  | Sends                                   | Undo        |
| ------------------------ | --------------------------------------- | ----------- |
| Aspect preset            | `crop-to-aspect` — each clip to its own | one edit    |
| Left, top, width, height | `set-crop-sides` — that side, all clips | one gesture |
| Reset crop               | `reset-crop`                            | one edit    |

A field steps on the coarsest grid among the selected sources, so a stepped
or scrubbed value is always on it. A value the engine refuses — outside the
picture, too small — comes back as the edit's refusal and is shown under the
field that sent it (`editGesture`'s `answered`), never as a toast. The
section shows "Mixed" for a side the selected clips do not share; changing it
sets that side on every clip and leaves each clip's other sides.

The cost is `clip_cost(clips)`: the export plan of the whole project, summed
for the selected clips — seconds of pictures re-encoded because of the crop,
for another reason, with a seam, copied; and the sound, copied or re-encoded.
The section asks again after every change to the graph, as the application
bar's indicator does, so "Cropping re-encodes this clip's pictures (4.2 s).
Its sound is still copied." turns back into "copied bit for bit" on reset.
Without a plan (a source offline) it says the cost is not stated.

## Reframe

```
File ▸ Reframe…  or  Sequence settings ▸ Reframe…
        │
        ▼
preview_reframe(aspect) ── Document::preview_reframe ──▶ (Reframe, project after)
        │                    (the edit's own compile, on a copy)
        ├── plan(project now)      ┐
        ├── plan(project after)    ├─▶ reframe_impact ──▶ the dialog's statement
        ▼                          ┘
edit(reframe) ── one entry: Settings + ReplaceClip per clip ── undo is exact
```

`reframe` decides, for each video clip on an unlocked track: no crop if its
picture is the target shape, its own crop kept if that has the shape,
otherwise the centred crop. The settings take the size of a source already
in the target shape (at the sequence's frame rate), or else of the crop of
the source with the most time on the timeline; the frame rate is kept. The
`Reframe` record says which clips were cropped, kept, left uncropped, left on
locked tracks, and scaled up, and on what basis the size was chosen.

Because the edit applied makes exactly the previewed project, the statement
equals the plan afterwards; `tests/crop_cost.rs` holds it to that, and to one
undo returning the file byte for byte.

## Framing on the preview

From [#131](https://github.com/ismetcahangirov/blinkify/issues/131). While a
clip's crop is framed on the player, the preview plays that clip whole, so
the rectangle is drawn over the picture it cuts from:

```
Frame on preview ── frame_crop(clip) ── ProjectPreview.framing
        │
        ▼
refresh: evaluate ─▶ Timeline::with_crop_lifted(clip) ─▶ PlaybackPlan
                     (the preview's copy only; the export never sees it)
```

The overlay (`player/CropOverlay.tsx`) is one canvas over the preview. Its
arithmetic is `player/cropFraming.ts`: the picture's box is worked out from
the last frame drawn exactly as `PreviewCanvas` places it — at device pixels,
rounded as it rounds, then back to CSS pixels — so a pointer maps to the same
source pixel at every rotation and display scale. Every rectangle it sends is
on the source's grid (`CropFrame.across`, `down`), inside the picture and at
least the minimum size, and a drag is one gesture (#37). What the user sees
and presses is specified in
[`../design/capcut-layout-reference.md`](../design/capcut-layout-reference.md).
