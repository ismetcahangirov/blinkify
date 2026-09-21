# Issue rules

What makes an issue good enough to work from without interviewing the author.

## The distinction that matters

**Requirements are what you build. Acceptance criteria are how someone else
proves you built it.**

They are written by the same person in the same sitting, which is exactly why
they collapse into each other. Keep them apart, because they serve different
readers at different times:

|                                                       | Requirements            | Acceptance criteria         |
| ----------------------------------------------------- | ----------------------- | --------------------------- |
| Written for                                           | The person implementing | The person reviewing        |
| Read                                                  | Before the work         | After the work              |
| Voice                                                 | "Build X"               | "X can be observed to do Y" |
| Tense                                                 | Imperative              | Present, observable         |
| Mentions files and APIs?                              | Yes, freely             | Almost never                |
| Can be checked by someone who did not write the code? | Not necessarily         | **Always**                  |

### The test

> Could a reviewer who has not read the diff verify this criterion?

If no, it is a requirement wearing the wrong hat.

### Examples

**Requirement** — what to build:

> - [ ] `probe()` returns rotation metadata from the container and from the
>       stream side data, preferring the stream

**Acceptance criterion** — how it is proved:

> - [ ] A portrait video recorded on an iPhone appears upright in the preview

Note what the criterion does not say: not `probe()`, not "rotation metadata",
not "prefers stream side data". Those are implementation. The criterion names a
file and an observable outcome, so a reviewer can check it by opening the app.

Two more pairs:

| Requirement                                                        | Acceptance criterion                                                               |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| Cache waveform peaks in a zoom pyramid keyed by content hash       | Scrubbing a 90-minute file a second time draws its waveform with no visible delay  |
| Kill the sidecar process on cancellation and delete partial output | Cancelling an export leaves no `ffmpeg.exe` running and no file at the output path |
| Add `renderer-not-into-engine` to `.dependency-cruiser.cjs`        | `pnpm graph:validate` fails when the renderer imports an engine crate              |

### Bad acceptance criteria, and what is wrong with them

| Bad                                         | Why                                                       |
| ------------------------------------------- | --------------------------------------------------------- |
| "The export works correctly"                | Unfalsifiable. Nobody can fail this.                      |
| "Code is clean and well structured"         | Taste, not a criterion. Say it in review.                 |
| "`ExportPlanner` handles the keyframe case" | Implementation detail. Restates a requirement.            |
| "Performance is acceptable"                 | Give a number and the machine it is measured on.          |
| "Tests pass"                                | True of every merged change. Says nothing about this one. |

## Every issue needs

- **One `type:` label, one `area:` label, one `priority:` label, one `size:`
  label.** An `epic` label replaces `size:`.
- **A parent Epic**, unless it is an Epic. Work without a parent is work nobody
  scheduled.
- **A dependency line**, even if it reads "Blocked by nothing".
- **Testing requirements including the failure cases.** A happy-path-only list
  is not finished. For this project that means at least: a corrupt file, a
  missing stream, and a cancellation mid-operation.

## Sizing

| Label         | Means                                                                         |
| ------------- | ----------------------------------------------------------------------------- |
| `size:small`  | Focused change, one area, one sitting                                         |
| `size:medium` | Several files or a full module with tests                                     |
| `size:large`  | **Should probably be split.** Treat the label as a question, not an estimate. |

## Epics

- **Epics do not receive code.** An Epic is a container. Work happens in its
  sub-issues, and the Epic closes when they all do.
- **An Epic's scope section is a boundary, not a to-do list.** The to-do list is
  the sub-issues.
- **The out-of-scope section is the valuable one.** It records what a reader
  would reasonably assume is included and is not, with the reason — which is the
  thing that gets re-litigated six weeks later.

## Media bugs

A bug report about media is unreproducible without the file's characteristics.
Container, codec **and profile**, variable or constant frame rate, and whether
it came off a phone. "An mp4 from my phone" is four unknowns, not one.

The bug template asks for all of them, plus the GPU and driver version, because
[ADR-0003](../decisions/ADR-0003-re-encode-encoder-strategy.md) makes the set of
available encoders a per-machine property — the same file genuinely behaves
differently on two machines, and that is by design rather than a defect.

## Writing the technical considerations section

This is the section people skip, and it is the one that saves a day.

Write down the approach that is **wrong but looks right**. Not the traps you
solved — the traps you nearly fell into. "`includeOnly` in the dependency-cruiser
config silently deletes the edges the rules reason about, leaving a gate that
passes and proves nothing" is worth more than the rest of an issue put together.
