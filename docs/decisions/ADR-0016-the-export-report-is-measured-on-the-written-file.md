# ADR-0016 — The export report is measured on the written file

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#52](https://github.com/ismetcahangirov/blinkify/issues/52),
  Epic [#8](https://github.com/ismetcahangirov/blinkify/issues/8)

## Context

The export report states, per segment, what was copied and what was
re-encoded, and the share of the output that is bit-identical to its source.
The issue requires it to "describe what actually happened, including a
fallback the planner did not predict", and its bit-identical percentage to
match the verification suite's measurement of the same export (#45).

The plan already says what should happen, and the executor does what the plan
says. The question is whether the report may simply restate the plan.

## Decision

**The report is measured. After the export writes its file, a `verifying`
stage hashes every packet of the output by the hash boundary of #45 and looks
for each among the packets its source holds around the segment. The report's
counts, its identical share and each segment's execution come from that
measurement; the plan supplies only what was intended and why.**

- **One boundary.** The report calls the same `verify` functions the
  losslessness suite does, so their numbers cannot drift apart. A test asserts
  they are equal.
- **Divergence is visible.** A segment whose packets are not what the plan
  decided is reported as what the file shows, with both named.
- **Bounded reading.** A source is read around each segment's range, not
  whole, so a short clip of a long recording costs a short read.

## Alternatives considered

### Restate the plan — rejected

Free, and right whenever the executor is right. But the report is the one
artefact that proves the product did what it promised, and a proof that
cannot fail proves nothing: a regression in the executor would produce a
report that still said "fully lossless".

### Count packets inside the executor as it copies them — rejected

Cheaper than reading the file again, and exact about what the executor
_intended_ to write. It is not a measurement of the file: the muxer, the
container and the disk are all after the count. It would also be a second
definition of "identical" beside the suite's.

### Decode and compare pictures (PSNR, pixel hashes) — rejected

Answers a different question. Two decodes of the same packets are identical by
construction; the claim the product makes is about packets, and that is what is
measured.

## Consequences

### What this makes easy

- The report and the suite agree by construction, and the report catches an
  executor regression the day it happens.

### What this makes hard

- An export takes longer by a read of its output and of its sources around the
  exported ranges. For a copy that is about the time the copy itself took.

### What we accept

- A file that cannot be measured is still exported; its job reports no report
  rather than failing an export that succeeded.
