# ADR-0022 — Reframe is one edit of settings and per-clip crops, sized by the sources

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#132](https://github.com/ismetcahangirov/blinkify/issues/132),
  with [#130](https://github.com/ismetcahangirov/blinkify/issues/130); Epic
  [#125](https://github.com/ismetcahangirov/blinkify/issues/125)

## Context

The commonest reason to crop is a vertical or square version of a landscape
edit. Done clip by clip it is tedious and error-prone; done as one
sequence-wide filter it re-encodes clips that did not need it and hides which
ones. ADR-0019 already settled that a crop is a rectangle per clip in the
source's display pixels. What was left open, and what this records:

- how a crop of a given shape is fitted to a picture, and rounded, so the
  inspector's presets (#130), the reframe (#132) and the handles on the
  preview (#131) fit the same rectangle;
- what a reframe is in the edit graph, so one undo takes it back exactly;
- what size the reframed sequence gets. It is the decision that decides
  which clips stay copies, and the issue asks for "the largest even size of
  the target aspect that the reframed crops fill without upscaling the
  typical source" while also asking that "a 9:16 clip in that project stays
  copied" — two rules that disagree whenever the 9:16 clip is not the
  typical one;
- what happens to locked tracks, to crops the user has already moved, and on
  reframing back.

## Decision

**A preset is the largest rectangle of its shape centred on the picture, on
its chroma grid, worked out in one engine function; a reframe is one edit
made of the sequence settings and a crop per clip, and the sequence takes
the size of a source already in the target shape if there is one, and
otherwise the size of the typical source's crop.**

- **One fit, one rounding.** `crop::centred(shape, aspect)` fits the largest
  rectangle of the aspect (pixel aspect included) to the displayed picture,
  rounds each side to the _nearest_ step of its axis' chroma grid — so 9:16
  of 1920 × 1080 is 608 × 1080 from 656, as #130 requires — and centres it on
  the grid. A picture already of the shape to within that rounding is
  returned whole: a crop that would remove a rounding pixel costs a re-encode
  for nothing. The controls never round; they show the engine's rectangles
  (`CropFrame.presets`) and send `crop-to-aspect`, which fits each selected
  clip to its own picture.
- **Reframe is an edit, not a field.** `Edit::Reframe { aspect }` compiles to
  one `Settings` change and a `ReplaceClip` per video clip, the primitives
  every other edit uses, so undo is exact by construction (ADR-0007). There
  is no sequence-level crop in the project file.
- **Each clip on an unlocked video track** gets the centred crop of the
  target shape — unless its picture already has that shape, when it gets no
  crop (and one it had is removed), or its crop already has that shape, when
  the crop is kept where the user moved it. So reframing twice changes
  nothing, and reframing back to the shape the sources have removes the crops
  and restores the copies.
- **The size.** If a source of a reframed clip is already the target shape
  and at the sequence's frame rate, the sequence takes its exact size — the
  one with the most time on the timeline, if several — and its clips stay
  copies. The cropped clips are re-encoded anyway, so scaling them to that
  size is part of a re-encode they already need; where that scales them up,
  the dialog says which clips and that it does. Otherwise the size is the
  crop of the source with the most time on the timeline, sides rounded down
  to even: that source is never scaled up. The frame rate is kept; the pixel
  aspect is the chosen source's.
- **Locked tracks are left alone** and named in the statement. The new
  settings still apply to their clips, as any settings change does; if that
  re-encodes one, the cost statement counts it.
- **The cost is stated before applying, from the plan.** The engine compiles
  the same edit onto a copy of the project (`Document::preview_reframe`), the
  shell plans the project before and after with the same source facts, and
  `export::cost::reframe_impact` compares them: which clips would stop being
  copies and for how long, which stay copies, which were re-encoded already.
  Applying the edit makes exactly the previewed project, so the statement is
  the plan's afterwards; a test holds both to it.
- **Refused, not guessed:** an empty sequence, a clip on an unlocked track
  whose source is offline (its crop cannot be placed), every video track
  locked, and the source's own shape as a target.

## Alternatives considered

### A sequence-level crop or "canvas" — rejected

The model several editors present, and ADR-0019 already rejected it for
crop. For reframe it would re-encode a clip already in the target shape and
could not say which clips lost quality. The issue forbids it outright.

### Always the typical source's crop size — rejected

Follows the issue's first rule to the letter, and breaks its acceptance
criterion: in a 16:9 project with three landscape clips and one vertical
phone clip, the sequence would become 608 × 1080 and the phone clip — already
9:16 — would be scaled down and re-encoded. Keeping a clip that needs no
change bit for bit is `CLAUDE.md` section 1; how much the clips that are
re-encoded anyway are scaled is secondary, and is stated.

### The largest source's size, or a standard size such as 1080 × 1920 — rejected

Either scales the typical crop up whenever no source has that size, silently
unless stated, and a standard size matches no source, so it can keep no clip
a copy. The sources already say what sizes are real.

### The smallest crop — rejected

Never scales anything up, and scales the typical source down to suit its
smallest neighbour: a quality loss chosen for the user without a reason.

### Recording which crops a reframe added, to remove exactly those — rejected

It would add state to the project file for the one purpose of reframing
back. "A clip whose picture is the target shape has no crop after a reframe"
gives the same result for every clip the reframe cropped, without it — and
undo is the exact way back regardless.

### Re-centring every crop on every reframe — rejected

A reframe after the user moved a crop to frame a speaker would undo that
work silently. A crop that already has the target shape is kept.

### Refusing a reframe while any track is locked — rejected

A lock protects its clips from edits, not the rest of the project from
edits. The locked clips are left and the statement says so.

### Rounding presets down (floor) rather than to the nearest step — rejected

It gives 606 × 1080 for 9:16 of 1080p, which is further from 9:16 than 608
and is not the rectangle #130 specifies.

## Consequences

### What this makes easy

- #131 drags the same rectangles the presets produce (`crop::centred`,
  `crop::has_aspect`), and nothing is rounded twice.
- The inspector, the reframe dialog, the export dialog and the report all
  read one plan; none decides a tier.
- Undo of a reframe is one entry that restores the file byte for byte.

### What this makes hard

- Previewing a reframe plans the project twice. The plan is cheap once
  sources are indexed; a project with an offline source cannot be planned,
  so the dialog says the cost cannot be stated and does not apply.

### What we accept

- A mixed project reframed around a native-shape source scales the cropped
  clips up to that size. It is stated per clip before applying, and those
  clips were being re-encoded because of the crop in any case.
- Reframing back is exact only for what the reframe did; a clip that had a
  crop of another shape before the first reframe loses it. Undo is the exact
  way back.
