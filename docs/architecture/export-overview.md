# What an export will do, before it runs

From [#50](https://github.com/ismetcahangirov/blinkify/issues/50), Epic
[#8](https://github.com/ismetcahangirov/blinkify/issues/8). Code:
`crates/blinkify-engine/src/export/overview.rs` (the overview),
`Edit::SnapToKeyframes` in `project/edit.rs` (the snap), the shell's
`export_overview` command, and `apps/desktop/src/export/` (the dialog and the
application bar's indicator).

Why the snap is an edit: [ADR-0015](../decisions/ADR-0015-the-keyframe-snap-is-an-edit-of-the-project.md).

The export dialog states, before anything is written, what the export will
copy and what it will re-encode, and why. Every claim is read off the plan
([`export-planner.md`](./export-planner.md)) and the executor's own checks
([`export-executor.md`](./export-executor.md)); the renderer computes none of
them (`CLAUDE.md` section 2).

```
project ─▶ evaluate ─▶ loudness (cached) ─▶ plan ─┐
                                                  ├─▶ overview() ─▶ ExportOverview
target, sound target, free space ─────────────────┘
```

## What the overview says

| Field           | From                                                                 |
| --------------- | -------------------------------------------------------------------- |
| `video`/`audio` | the plan's totals per stream: lossless, seconds copied, re-encoded   |
| `lossless`      | `plan.summary.lossless`: nothing at all is re-encoded                |
| `reasons`       | every cause of every segment, with its time on the sequence          |
| `snap`          | the keyframe-aligned cut of every smart-cut, as one undoable edit    |
| `size`          | the copied streams' rates, or the encoders' (below)                  |
| `audioEncoding` | the executor's choice of codec for re-encoded sound                  |
| `space`         | the target's drive against the size, with a margin                   |
| `targetExists`  | replacing it needs the save dialog's confirmation                    |
| `problems`      | why the executor would refuse, and a drive without room — as it says |

**Pictures and sound are claimed apart.** A gain change re-encodes the sound
and copies the pictures; one badge would hide one or deny the other.

**A reason has a time and, for a cut, a distance.** A cause at an in-point is
placed at the segment's start, one at an out-point at its end, and a cut says
how far it is from the nearest keyframe: "The clip starts 0.40 s from the
nearest keyframe, so …". A declined segment says why in the executor's terms —
"The cut cannot be smart-cut: this computer has no encoder for H264" — and
names the snap as the remedy. `overview::explain` is the one phrasing, for the
dialog and for the report (#52).

## The keyframe snap

Every smart-cut in the plan carries a `KeyframeAlternative`: the nearest place
a copy can start (a random-access keyframe) and end (a clean end, a keyframe
with no leading pictures, or the end of the stream). The overview turns those
into `Edit::SnapToKeyframes`, and states each clip's in and out shift in
seconds and the sequence's change in length.

Accepting it is an ordinary edit, applied through the project store and on
the undo stack, so what is previewed is what is exported. It:

- gives each clip its new trim and keeps its start;
- trims every clip linked to it — its detached sound — by the same source
  time, so they stay in sync;
- moves everything after it on every unlocked track by the change in its
  length, so no gap opens (a gap is black, which is re-encoded) and nothing
  falls out of sync;
- refuses a held or reversed clip, which is re-encoded whatever its cuts.

A smart-cut whose segment is not its clip's whole trim — a clip partly covered
from above — is cut by the cover, not by the trim; it is counted as
`unsnappable` and left alone.

Tested: two clips back to back, each cut between keyframes at both ends, plan
with seams; after the snap the plan's pictures are all stream copies, the
second clip meets the first, and undo returns the project byte for byte.

## The size

| Segment                   | Bytes                                                   |
| ------------------------- | ------------------------------------------------------- |
| copied or smart-cut       | the source stream's bitrate × the source time it covers |
| pictures re-encoded whole | the reference source's video bitrate × its length       |
| sound re-encoded          | the encoder's rate; FLAC ≈ 60 % of 24-bit PCM           |

`precise` is true when everything is copied **and** every copied stream's
rate is one its file records — or the file's rate less every other recorded
one, where only one stream has none. Then the estimate is within **10 %** of
the file written (tested on MP4, AV1-in-MP4, and a partial copy). Matroska
records no per-stream rates; with pictures and sound both unrecorded, the
sound is taken at 64 kb/s a channel and the pictures get the rest, and the
dialog says it is an estimate.

## The space

The shell asks the drive of the target's folder (or the nearest folder above
it that exists) for its free bytes (`fs4`, a safe wrapper over
`GetDiskFreeSpaceExW`). Enough means the size plus 5 % plus 64 MiB. The
dialog refuses to start without it, and the export job (#51) checks again
before it writes anything, since the drive may have filled in between.

## Replacing a file

The destination is chosen in the native save dialog, which asks before
replacing a file. Only that exact path is exported with `overwrite`; a path
changed afterwards — by switching the container — that turns out to exist
must be chosen again. The executor refuses a source as the target whatever
the confirmation.

## The application bar

The lossless indicator shows the plan in one state and a count —
`Lossless`, `2 seams`, `1 seam · 1 segment re-encoded`, or `1 cut cannot
export` — asked again after every edit. Without a plan it says the tier is not
computed.
