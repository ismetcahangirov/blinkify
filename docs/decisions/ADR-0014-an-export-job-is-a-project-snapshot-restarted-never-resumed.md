# ADR-0014 — An export job is a project snapshot, run one at a time, restarted after a crash rather than resumed

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#51](https://github.com/ismetcahangirov/blinkify/issues/51),
  Epic [#8](https://github.com/ismetcahangirov/blinkify/issues/8)

## Context

Exports run in the background while the user goes on editing, several can be
asked for in a row, and the application can die in the middle of one. Four
questions follow, and each has a plausible wrong answer:

- **What does a job hold?** The export must not see edits made after it was
  asked for, and a job interrupted by a crash must still be runnable at the
  next launch — possibly by a newer build of Blinkify.
- **How many run at once?** An export is already a reader per stream, a muxer
  and, for a re-encoded segment, decoders and an encoder, all coupled by pipes
  (ADR-0010).
- **What does "resume after a crash" mean?** The issue itself warns that
  resuming is genuinely partial for most codecs.
- **How is the user told an export finished while the window is in the
  background?**

## Decision

**A job holds a copy of the project — the versioned document of ADR-0006 — and
plans from it when it starts. Jobs run one at a time, in the order they were
asked for. A job the application stopped under is offered at the next launch
to be exported again from the start, or discarded; it is never continued. An
export that ends while the window is not focused flashes the window in the
taskbar.**

- **The snapshot is the project, not the plan.** The project format is
  versioned and migrated; the plan is an internal structure that changes with
  the planner. A queue file written by one build must be readable by the next,
  so the job keeps the thing that has a compatibility promise. Planning from
  it at the start is cheap (18 ms for 6000 clips, `export-planner.md`) and
  measures loudness exactly once, in the job, as ADR-0013 requires.
- **One at a time.** The orchestrator's export slots are sized for the
  processes of one export (#110); two 4K exports side by side saturate the
  machine and finish later than one after the other. A queue in order is also
  what a user who asks for three exports expects to see.
- **Restarted, not resumed.** Every output stream is interleaved through one
  muxer writing one file; a file cut off mid-write has no index and no point a
  second muxer could append to that would not have to re-read and rewrite
  everything before it. What is offered is what is true: "export again from
  the start". Whatever the interrupted export left beside the target is
  removed first.
- **An orderly quit is an interruption too.** Closing Blinkify during an export
  stops it and leaves it to be offered at the next launch, rather than
  discarding work the user did not decide to discard.
- **The taskbar flash.** `request_user_attention` is Windows' own way to say
  "come and look", needs no new dependency, and cannot be mistaken for a
  notification from anything else. The export queue in the application bar
  says what happened.

## Alternatives considered

### Persist the plan — rejected

It would make the job independent of the probe and the keyframe index at
launch, but it ties the queue file to the planner's internal types: a build
that adds a field to a segment could not read the queue of the build before
it, and the interrupted export — the one case the file exists for — would be
lost at exactly the moment of an update.

### Resume from the last complete segment — rejected

Possible in principle for a copy-only export into Matroska, which tolerates
appending. Not for MP4, whose index is written at the end, nor for any export
with an encoder, whose state is gone with its process. Supporting it for one
container and one tier would make "resume" mean different things for exports
that look the same to the user.

### Run exports in parallel — rejected

Faster for two small exports on a large machine; slower, and fighting for the
same encoder, for anything else. The orchestrator's limits already decide how
much media work runs at once; a queue that ignored them would defeat them.

### `tauri-plugin-notification` for a toast — rejected for now

A new dependency, and on an unsigned per-user install the toast is attributed
to whatever application identity Windows resolves, which is not reliably
Blinkify. The taskbar flash says the same thing with no dependency. A toast can
follow when the installer is signed.

## Consequences

### What this makes easy

- Editing during an export is safe by construction: the job cannot see the
  document the user is changing.
- A queue file survives an update of the application.
- What the report (#52) says happened is the job's own plan, made once.

### What this makes hard

- An export whose sources moved between the crash and the relaunch fails when
  it is exported again, with the missing file named; the user relinks and asks
  again.

### What we accept

- An interrupted export costs its whole length again. For the exports Blinkify
  is built for — copies — that is the time it takes to read the file.
