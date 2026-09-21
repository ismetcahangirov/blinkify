# ADR-0002 — Bundle an LGPL-only FFmpeg build and invoke it as a sidecar process

- **Status**: Accepted
- **Date**: 2026-09-21
- **Context issue**: [#1](https://github.com/ismetcahangirov/blinkify/issues/1),
  [#9](https://github.com/ismetcahangirov/blinkify/issues/9),
  [#21](https://github.com/ismetcahangirov/blinkify/issues/21)

> This ADR settles the **copyright** question: which licence governs the code we
> ship, and what that permits. The **patent** question — who is owed a royalty
> for encoding H.264 or HEVC — is a separate matter and is settled in
> [ADR-0003](./ADR-0003-re-encode-encoder-strategy.md). Do not treat one as
> answering the other.

## Context

Blinkify cannot exist without FFmpeg. Probing, keyframe indexing, stream copy,
smart-cut and the audio filter chain are all FFmpeg work.

FFmpeg ships under two licences depending on how it is configured:

- **LGPL v2.1+** for the default build.
- **GPL v2+** once it is configured with `--enable-gpl`, which is what pulls in
  `libx264`, `libx265`, and a set of GPL filters.

This matters more than it first appears. GPL is viral across a linking boundary.
If Blinkify linked GPL FFmpeg into its own process, Blinkify itself would have
to be distributed under the GPL. That forecloses any future commercial licence
of the product, and it is a decision that is extremely expensive to unwind once
code has shipped — every contributor would have to agree to relicense.

LGPL is not viral in the same way, but it is not free of obligation either. It
requires that the user be able to replace the LGPL component with their own
version. Static linking makes that obligation awkward; a separate process makes
it trivial.

## Decision

Blinkify bundles an **LGPL-only FFmpeg build**, configured **without
`--enable-gpl`** and therefore without `libx264` and `libx265`, and invokes it
as a **separate sidecar process** over an argument vector and a pipe. Blinkify
does not link FFmpeg into its own address space.

Two gates enforce this, and both block a merge:

1. **A licence gate in CI** asserts the bundled binary's build configuration
   does not contain `--enable-gpl` and that no GPL-only component is present.
   Tracked in [#21](https://github.com/ismetcahangirov/blinkify/issues/21).
2. **`cargo deny check licenses`** rejects any GPL or AGPL Rust dependency,
   which is the same boundary approached from the other side.

The FFmpeg binary, its licence text and its build configuration are recorded in
[`../../THIRD_PARTY.md`](../../THIRD_PARTY.md).

## Alternatives considered

### Link FFmpeg into the process with `ffmpeg-next` or similar — rejected

Tempting: no process spawn, no argument-vector marshalling, no pipe parsing, and
direct access to frames without a copy.

Rejected because it makes the LGPL replaceability obligation genuinely hard to
satisfy — a statically linked build offers the user no way to substitute their
own FFmpeg — and because it removes the safety property we get for free from a
process boundary. A malformed file that crashes a decoder kills a sidecar we can
restart; in-process it kills the editor with the user's unsaved project in it.
`CLAUDE.md` section 11 treats every media file as hostile input, and a process
boundary is the cheapest way to mean it.

### Ship GPL FFmpeg and license Blinkify under the GPL — rejected

Technically the easiest path, and it would give us `libx264` and `libx265`,
which would remove most of ADR-0003's difficulty outright.

Rejected because it permanently forecloses a commercial licence, and because the
decision is effectively irreversible — relicensing after the fact requires
agreement from everyone who has contributed. We are not willing to pay that
price at the start of the project for encoder convenience, particularly when the
product's entire premise is to re-encode as little as possible.

### Require the user to install FFmpeg themselves — rejected

Sidesteps both licence questions completely: we would ship no FFmpeg at all.

Rejected on product grounds. Blinkify targets people who want to trim a clip
without damaging it, not people who will configure a `PATH`. It also destroys
reproducibility — we would be testing against whatever build each user happened
to install, including GPL builds with different behaviour around timestamps and
stream copy. Behaviour we cannot reproduce is behaviour we cannot support.

### Use the Windows Media Foundation APIs instead of FFmpeg — rejected

No third-party licence question at all, and the codecs are already on the
machine.

Rejected on capability. The container and codec coverage is narrower than
FFmpeg's, and the packet-level control that stream copy and smart-cut require is
not exposed in the form we need. Losing tier 1 to avoid a licence file is not a
trade.

## Consequences

### What this makes easy

- Blinkify's own licence stays open. A commercial licence remains available
  later.
- The LGPL replaceability obligation is satisfied by construction: the sidecar
  is a separate executable in a known location, and a user may replace it.
- A decoder crash on a malformed file is contained. The sidecar dies, the editor
  does not.
- Cancellation is honest. Killing a process actually stops the work, which is
  what `CLAUDE.md` section 12 requires.

### What this makes hard

- **No `libx264` and no `libx265`.** These are the two best software encoders
  for H.264 and HEVC and both are GPL. Every consequence of their absence is
  worked through in
  [ADR-0003](./ADR-0003-re-encode-encoder-strategy.md); it is the single largest
  cost of this decision and it is paid at the seam of every smart-cut.
- **Process-boundary overhead.** Frames and packets cross a pipe. The engine is
  designed around this — see `CLAUDE.md` section 2 on a coarse command surface.
- **Argument-vector discipline.** The sidecar is invoked with an argument vector
  and never a shell string, so a filename containing a quote is not an injection
  vector (`CLAUDE.md` section 11, forbidden behaviour 5).
- **Progress and errors must be parsed** out of the sidecar's output rather than
  read from a return value.

### What we accept

- We carry the FFmpeg licence text and build configuration in
  `THIRD_PARTY.md`, and we keep them accurate. A stale licence file is a
  compliance failure, not a formatting one.
- We accept the ~80–100 MB the bundled binary adds to the installer. It is still
  an order of magnitude below what the Electron alternative in ADR-0001 would
  have cost before any media capability at all.
- Anyone rebuilding the sidecar must reproduce the configuration. The build
  configuration is committed, not improvised.
