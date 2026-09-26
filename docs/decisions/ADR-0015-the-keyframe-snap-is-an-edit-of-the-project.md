# ADR-0015 — The keyframe snap is an edit of the project, rippled across unlocked tracks

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#50](https://github.com/ismetcahangirov/blinkify/issues/50),
  Epic [#8](https://github.com/ismetcahangirov/blinkify/issues/8)

## Context

A cut between keyframes is smart-cut: a fraction of a second at the seam is
re-encoded (tier 2), or, where no encoder here can match the source, the cut
is declined (ADR-0003). The export dialog offers the alternative: move the cut
to the nearest keyframe and the pictures are copied whole. ADR-0003 part 4
already chose to offer it; what it did not decide is what accepting it
_does_, and there are three plausible answers.

## Decision

**Accepting the snap applies `Edit::SnapToKeyframes` to the project: each
clip gets the keyframe-aligned trim the plan offers and keeps its start; a clip
linked to it is trimmed by the same source time; and everything after it on
every unlocked track moves by the change in its length.** It is undoable like
any other edit.

- **An edit, so the preview is the export.** The user sees and hears the
  moved cut before exporting it, and the project file records it.
- **Rippled, so no gap opens.** A clip that becomes shorter would otherwise
  leave a gap, which is black — re-encoded — and would defeat the point; one
  that becomes longer would overlap its neighbour and be refused.
- **Across unlocked tracks, like paste (#38).** Linked pictures and sound, and
  sound on other tracks, keep their places relative to each other. A locked
  track is left alone, as a lock means.
- **The nearest keyframe, either way.** The in-point moves to the nearest
  random-access keyframe, earlier or later; the out-point to the nearest clean
  end. The dialog states each shift in seconds so the choice is informed.

## Alternatives considered

### Snap in the export only — rejected

The file written would differ from what the preview plays and the project
says, silently. The export dialog would be the only place the difference was
visible, and it closes.

### Snap without rippling — rejected

A shorter clip leaves a gap that is encoded as black: the snap would trade a
seam for a re-encoded gap. A longer one overlaps the next clip and the edit is
refused.

### Only ever shorten the clip (in-point later, out-point earlier) — rejected

Always safe for the neighbours, but it can drop up to a whole GOP where the
keyframe before the cut was a frame away. The nearest keyframe moves the cut
least, and the dialog says which way.

## Consequences

### What this makes easy

- The claim "accepting the snap leaves no picture re-encoded" is testable on
  the plan, and is tested.
- Undo is exact, because the edit is primitive changes like every other.

### What this makes hard

- A cut made by a clip covering another from above is not the lower clip's
  trim; it cannot be snapped this way and is reported as unsnappable.

### What we accept

- The sequence's length changes by the sum of the shifts, which the dialog
  states before the user accepts.
