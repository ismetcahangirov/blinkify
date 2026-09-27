# ADR-0019 — A crop is a rectangle per clip, in the source's display pixels

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#127](https://github.com/ismetcahangirov/blinkify/issues/127),
  Epic [#125](https://github.com/ismetcahangirov/blinkify/issues/125)

## Context

Crop is the first operation Blinkify offers that changes pixels on purpose.
Every clip it touches goes through tier 3 — decode, filter, re-encode — which
is the pipeline the product exists to avoid, so the model has to make two
things impossible to get wrong: that a crop costs quality only where it is,
and that the rectangle means the same thing to the preview, the export and
the controls that draw it.

Four questions had to be answered before any of the rest of Epic #125 could
be built on it: where the crop lives, in which orientation its rectangle is
stored, in what unit, and what a rectangle has to be.

## Decision

**A crop is `Operation::Crop { x, y, width, height }` on one clip: a
rectangle in the source's display pixels — after its rotation, as the user
sees it — whose offsets and sizes lie on the source's chroma grid.** It is
data in the edit graph like every other operation; the evaluator resolves it
onto the clip's `Placement` and is the only reader.

- **Per clip, never per sequence.** A cropped clip's pictures are re-encoded
  (`ReEncodeReason::FilterChangesPixels`, the reason that already existed and
  was unused). Its sound is copied (`Placement::sound_forced` leaves a crop
  out), and the clips around it are copied. A sequence-wide filter would
  re-encode every clip, including those whose picture it did not change — the
  silent loss `CLAUDE.md` section 1 calls the most serious bug this project
  has. Reframing a whole sequence (#132) is made of per-clip crops, one
  compound edit, so it keeps this property.
- **Display orientation.** The rectangle is what the user drew on an upright
  picture. A portrait phone clip is usually stored as a landscape stream with
  a 90° display matrix; storing coded pixels would make the same drawing mean
  a different region depending on how the phone was held. Each consumer maps
  the rectangle to the orientation it decodes in — the export renderer (#128)
  and the preview (#129) run FFmpeg without autorotation — and tests the
  mapping for 0°, 90°, 180° and 270°.
- **Source pixels, integers.** The rectangle is in the source's own pixels,
  not in fractions of the frame and not in the sequence's pixels. A fraction
  cannot say "the 608 columns from 656" exactly, and the sequence's pixels
  change when its settings do. A proxy (#26) is mapped by the preview, not
  stored.
- **On the chroma grid, checked in the edit layer.** A 4:2:0 picture stores
  its colour at half resolution both ways, so an odd offset or size either
  fails in the encoder or shifts the colour by half a sample. The edit layer
  refuses such a rectangle with the step it must keep to; 4:2:2 is checked
  across only, 4:4:4 not at all, and the axes swap with a quarter-turn
  rotation. An unknown subsampling is held to 4:2:0, the strictest. The
  rectangle must also lie inside the picture and be at least 16 × 16.
- **A crop of the whole picture is no crop.** It removes the operation, so the
  clip is copied again, as a 0 dB gain does (#46).
- **Crop before the scale to the sequence.** A clip in another shape than the
  sequence is scaled to fit and padded (#55); the crop comes first, so a 9:16
  crop of a 16:9 clip fills a 9:16 sequence with no bars and keeps every
  source pixel the scale would otherwise throw away.
- **HDR and missing encoders are the planner's existing declines.** A cropped
  HDR clip plans as declined (ADR-0008), and one no encoder here can match
  plans as declined (ADR-0003) — never encoded with whatever is available.

The schema moves to version 7. Nothing migrates: a version-6 file has no
crop, and the bump makes a version-6 build refuse a file holding one as newer
rather than damaged.

## Alternatives considered

### A sequence-wide crop or "canvas" — rejected

One rectangle for the whole output is how many editors present reframing,
and it is the wrong model here: every clip would re-encode, including one
already in the target shape, and the export dialog could not name which
segments lost quality because of it — all of them would. #132 builds the
same result out of per-clip crops.

### The rectangle in coded pixels — rejected

It would make the file's meaning depend on the source's rotation, and the
controls would have to rotate every drag before storing it. The mapping is
needed either way; doing it in the consumers keeps the stored value the one
the user drew.

### The rectangle as fractions of the frame — rejected

Resolution-independent, and inexact: rounding a fraction to pixels differs
between consumers by one pixel, which is the disagreement the one-filter rule
of Epic #125 exists to prevent.

### Rounding an odd rectangle to the grid silently — rejected

The stored value would not be the value used, and the user would never know
the edge moved. The rectangle is refused with the step to keep to; the
controls (#130, #131) snap to the grid before they ask, so the refusal is for
anything else that asks.

### A new `ReEncodeReason::Crop` — rejected

`FilterChangesPixels` already says what happens, the report and dialog
already phrase `Operation` causes, and a crop is not the only pixel filter the
project will have. The sentence names the crop: "The clip is cropped, so its
pictures are re-encoded; its sound is copied."

## Consequences

### What this makes easy

- The export renderer, the preview and the controls all read one resolved
  rectangle from the evaluator; none interprets the graph.
- Undo is exact by construction: a crop is a clip replaced by its copy with
  one operation more or less (ADR-0007).
- The cost is visible everywhere a tier is: the plan names the crop as the
  cause of each cropped segment, and the report suggests removing it.

### What this makes hard

- Every consumer must map display to coded orientation itself, and has to be
  tested at each rotation to prove it did.
- Animated crops (pan and scan) do not fit a single rectangle; they are a
  standing non-goal (Epic #125, out of scope).

### What we accept

- The edit layer needs the source's shape to check a rectangle, so a clip
  whose source is offline cannot be cropped until it is relinked. Resetting a
  crop needs no shape and always works.
- Whether an H.264 or HEVC crop can be a copy — by rewriting the SPS cropping
  window — is decided separately (#126). If it is adopted, the planner chooses
  that route over tier 3 where it applies; the model here does not change.
