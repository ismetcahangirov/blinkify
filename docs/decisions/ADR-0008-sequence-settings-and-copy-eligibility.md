# ADR-0008 — Sequences match their first clip, and one predicate decides copy eligibility

- **Status**: Accepted
- **Date**: 2026-09-25
- **Context issue**: [#57](https://github.com/ismetcahangirov/blinkify/issues/57)

## Context

A stream copy moves a source's packets into the output unchanged. That is
only possible where the sequence has the source's shape: a sequence at a
different resolution, frame rate or pixel aspect forces every frame of the
clip to be re-encoded to fit. Sequence settings are therefore the one switch
that can silently turn the whole product off — a 1080p sequence holding 4K
phone footage re-encodes everything and still plays fine.

Three consumers need the answer to "can this clip be copied, as far as the
sequence is concerned?": the export planner (#39), the clip inspector's
lossless statement (#56) and the export dialog's badge (#50). If each works
it out, they eventually disagree.

HDR also needed a stated position: Epic #4 previews in SDR, #44 requires HDR
to be handled or explicitly rejected, and #45 puts HDR in the corpus.

## Decision

1. **A new sequence matches its first clip.** `Sequence.matchFirstClip` is
   `true` on a new project (schema 2). The first video clip placed on an empty
   sequence gives it that clip's display size, nominal frame rate and pixel
   aspect **in the same edit**, so one undo takes back both. Choosing settings
   explicitly clears the flag. A clip whose shape no file can carry (an odd
   dimension, a rate outside 1–240 fps) leaves the settings alone and is
   reported as not copy-eligible, with the reason.
2. **One predicate.** `project::copy_eligibility(settings, geometry)` is the
   only code that decides whether a source's pictures can be copied into the
   sequence. It returns every mismatch separately — resolution, frame rate,
   pixel aspect — because #39 requires separately reportable preconditions.
   The document evaluates it per source (`Document::eligibility`) and the
   shell sends the answer in every `ProjectView`; the timeline marks the clips
   of an ineligible source and never computes anything itself. The planner
   (#39), the inspector (#56) and the export dialog (#50) read the same field.
3. **The cost of a change is stated before it is made.** `preview_settings`
   returns the clips and duration that would stop (or start) being
   copy-eligible; the settings dialog states it before Apply.
4. **Colour: SDR (Rec. 709) in v1.** `ColourPolicy::Sdr` is what the preview
   shows and what a re-encoded segment is made in. **An HDR source is copied
   as recorded** where its geometry permits — its pixels and metadata are not
   touched, which is exactly lossless. **Any part of an HDR source that would
   have to be rendered** (a smart-cut seam, a filter) **is declined** with the
   reason, never tone-mapped to SDR. The eligibility carries an `hdr` note so
   the interface can say so before export. This is the behaviour #44 asserts.
5. **Variable frame rate is copy-eligible** when its nominal rate matches the
   sequence. It is copied with its timing as recorded, never conformed to the
   sequence's grid; the eligibility carries a `variable-frame-rate` note.
6. **Geometry is read from the files, not stored.** The document keeps what
   each source's pictures are (`StreamGeometry`, from the probe) beside the
   graph, not in the project file. It is derived data, and a stored copy
   could disagree with the file it describes; the fingerprint (#32) already
   says whether the file changed.

## Alternatives rejected

- **A fixed default (1080p30) with a warning.** The default is wrong for most
  phone footage (vertical, 60 fps, 4K), so the common case would re-encode by
  default and rely on the user reading a warning. #57 names this failure.
- **The planner derives eligibility itself.** Simplest for #39, but then the
  inspector and the export dialog need their own copies, and three copies of
  a rule become three rules.
- **Tone-map HDR to SDR on export.** A silent conversion — the output differs
  from the source in a way the user did not ask for, which `CLAUDE.md`
  section 1 forbids. Declining is honest; tone-mapping, if it is ever offered,
  is an explicit, visible choice in a later ADR.
- **An HDR sequence policy in v1.** Needs an HDR preview path and HDR
  re-encode profiles (#44). Out of scope for v1; the enum leaves room for it.
- **Refuse VFR sources, or conform them to the sequence rate.** Refusing
  excludes most phone footage; conforming re-times or duplicates frames, which
  is a quality change. Copying the recorded timing is lossless.
- **Store the probed geometry in the project file.** It would open offline,
  but it can go stale and it doubles what must be migrated; the file stays
  the record of the user's decisions.

## Consequences

- The planner (#39) must take `copy_eligibility` as one of its preconditions
  and must not re-derive it. Its test asserts the two never disagree.
- The warning at import and at drop, and the offer to adopt a clip's settings
  there, are built on `ProjectView.eligibility` and `matchFirstClip` by the
  media library (#53).
- A project whose sources are offline has no geometry for them; their clips
  are not marked either way until the files are found.
