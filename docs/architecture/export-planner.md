# The export planner

From [#39](https://github.com/ismetcahangirov/blinkify/issues/39), Epic
[#6](https://github.com/ismetcahangirov/blinkify/issues/6). Code:
`crates/blinkify-engine/src/export/plan.rs` (the planner) and
`export/facts.rs` (what it plans from). The shell's `plan_export` command
returns the plan for the open project.

The planner compiles the evaluated edit graph into the segments of the output,
and decides for each one the lowest tier that satisfies the edit
(`CLAUDE.md` section 1). It is the single source of truth for what gets copied:
the executor does what it says, the export dialog (#50) shows it before the
export, the report (#52) states it after.

## Inputs, and where each rule lives

```
Timeline (evaluator, #30) ─┐
SequenceSettings ──────────┼──▶ plan() ──▶ ExportPlan { segments, summary }
SourceFacts per source ────┘
   ├─ probe (#23): geometry, codec, profile, level, pixel format, colour,
   │               sha256 of the codec configuration (SPS/PPS), B-frames
   └─ keyframe index (#24): every keyframe, and whether pictures shown before
                            it are decoded after it (open GOP, RADL)
```

The planner re-derives nothing that is decided elsewhere:

| Question                                | Decided by                          |
| --------------------------------------- | ----------------------------------- |
| Is the sequence the source's shape?     | `copy_eligibility` (ADR-0008)       |
| What does the speed do to the pictures? | `speed::verdict` (ADR-0009)         |
| Does a hold or reverse force a render?  | `Placement.forced` (evaluator, #35) |
| What does the picture consist of?       | `Timeline::picture` (#36)           |

## Segments

Video and audio are planned separately, and each stream's segments tile
`0..length` of the sequence with no gap and no overlap (property-tested).

- **Video** follows `Timeline::picture()`: the top visible clip at each frame.
  An empty stretch is a `gap` segment (black is encoded).
- **Audio** is swept over every audible sound — a video clip's own sound from
  its source's default audio stream unless detached, and every clip on an
  audible audio track. One sound alone may be copied; two at once are mixed.
  Silence between sounds is a `gap`.

## Causes

Each precondition of a copy that does not hold is its own `Cause`, recorded
as data. A segment carries all of its causes; its tier follows from them:

| Cause                                     | Effect                             |
| ----------------------------------------- | ---------------------------------- |
| `in-point-not-keyframe`                   | smart-cut, window to next keyframe |
| `out-point-not-keyframe`                  | smart-cut, window from last one    |
| `open-gop-at-out-point`                   | smart-cut                          |
| `keyframes-unknown`                       | re-encode: `copy-not-provable`     |
| `operation` (hold, reverse)               | re-encode                          |
| `speed-outside-container`                 | re-encode                          |
| `sequence-mismatch` (one per mismatch)    | re-encode                          |
| `incompatible-encoding` (fields named)    | re-encode to the reference         |
| `gap`                                     | re-encode (black or silence)       |
| `audio-chain`, `audio-speed`, `audio-mix` | audio re-encode only               |

Any whole-segment cause makes a full re-encode, named by the first; otherwise
a seam makes a smart-cut; otherwise the segment is a copy. `Cause::sentence`
states each one in words.

### Where a copy may start and end

- **Start**: exactly on a keyframe. An open-GOP keyframe is a valid start: the
  pictures shown before it are before the cut and are dropped.
- **End**: at the end of the stream; or on a keyframe with no leading pictures;
  or anywhere in a stream without B-frames, where every picture is decoded
  before the next is shown.

A seam's window is the stretch the smart-cut re-encodes, in source ticks. The
planner's window is an upper bound from the keyframes; the smart-cut executor
(#41) narrows it to the pictures that actually depend on the other side.

### One set of encoding parameters per stream

Copied packets from two sources can only share an output stream if a decoder
can carry one configuration across the join. Every copy candidate's
`EncodingSignature` — codec, profile, level, pixel format, coded size, colour,
and the hash of the codec configuration record — is compared. The output takes
the signature of the material covering most of it (ties to the earlier), and
every other candidate is re-encoded to match, with the differing fields named.
Two phone clips recorded at different levels are the ordinary case.

### When the planner cannot prove a copy

A cut point not yet in a keyframe index that is still being built is neither
provably on a keyframe nor provably off one. It plans as a re-encode with
`keyframes-unknown`. The answer improves as the index completes; it never
guesses.

### HDR

Per ADR-0008, an HDR source copied as recorded is lossless, and any part of it
that would have to be rendered is **declined**, not tone-mapped. Every
smart-cut — declined or not — carries a `KeyframeAlternative`: the nearest
keyframe-aligned cut, which the export dialog offers as the snap (ADR-0003
part 4, #50; [`export-overview.md`](./export-overview.md)). For a declined one
it is the way to export at all. `summary.exportable` is false while any
segment is declined.

## Summary

`PlanSummary` gives, per stream, the sequence frames copied and re-encoded —
the window of a smart-cut counts as re-encoded and the rest of it as copied —
and two answers: `lossless` (nothing is re-encoded) and `exportable` (nothing
is declined).

## Performance

The planner is pure: no file, no process, no clock. Measured on the reference
laptop, a 6000-clip timeline plans in 18 ms in an optimised build (about 3 µs a
clip) and 104 ms in the unoptimised test build, where the test asserts a
300 ms ceiling. A project of a few hundred clips re-plans inside one frame, so
the plan can be recomputed on every graph change.
