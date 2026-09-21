# ADR-0003 — Probe encoders at runtime, and decline rather than degrade

- **Status**: Accepted
- **Date**: 2026-09-21
- **Context issue**: [#9](https://github.com/ismetcahangirov/blinkify/issues/9),
  [#21](https://github.com/ismetcahangirov/blinkify/issues/21),
  [#41](https://github.com/ismetcahangirov/blinkify/issues/41),
  [#44](https://github.com/ismetcahangirov/blinkify/issues/44),
  [#55](https://github.com/ismetcahangirov/blinkify/issues/55)

> [ADR-0002](./ADR-0002-lgpl-ffmpeg-sidecar.md) settles the **copyright**
> question — which licence governs the code we ship. This ADR settles two
> further questions it does not answer: **which encoder** we use when pixels
> genuinely must be re-encoded, and the **patent** position that encoding H.264
> or HEVC raises regardless of copyright licence.

## Context

Tier 2 (smart-cut) and tier 3 (full re-encode) in `CLAUDE.md` section 1 both
require an encoder. Tier 2 requires something considerably stronger than "an
encoder": it requires one whose output will **concatenate with stream-copied
packets from the source** and still decode correctly.

A seam encode is only valid if it matches the source on all of:

- Codec, **profile** and level — level at least that of the source
- Resolution and, where relevant, sample aspect ratio
- Pixel format: chroma subsampling **and** bit depth
- Colour primaries, transfer characteristics, matrix coefficients and range
- Progressive or interlaced, and field order if interlaced
- Bitstream conventions the decoder will carry across the join

"Close enough" is not a category here. A High-profile source spliced with a
Constrained-Baseline seam is not a slightly worse file; it is a file that
decoders may render wrong or refuse.

**ADR-0002 removed our two best tools.** Declining `--enable-gpl` means no
`libx264` and no `libx265`. What is actually left on an LGPL build:

| Source codec | LGPL software encoder                      | Hardware encoders (vendor-supplied)                  |
| ------------ | ------------------------------------------ | ---------------------------------------------------- |
| H.264 / AVC  | `openh264` — **Constrained Baseline only** | `h264_nvenc`, `h264_qsv`, `h264_amf` — Main and High |
| HEVC / H.265 | **none exists**                            | `hevc_nvenc`, `hevc_qsv`, `hevc_amf` — Main, Main 10 |
| AV1          | `libsvtav1`, `libaom-av1` (BSD)            | `av1_nvenc`, `av1_qsv`, `av1_amf` on recent parts    |
| VP9          | `libvpx-vp9` (BSD)                         | limited                                              |
| AAC audio    | FFmpeg native `aac` (LGPL)                 | n/a                                                  |
| Opus / MP3   | `libopus` (BSD), `libmp3lame` (LGPL)       | n/a                                                  |

Two facts in that table decide this ADR:

1. **`openh264` cannot encode High profile.** Phone and camera footage is
   overwhelmingly H.264 High profile, so `openh264` cannot produce a valid seam
   for the most common source Blinkify will ever see. It does not solve our
   problem.
2. **There is no LGPL software HEVC encoder at all.** Not a weak one — none. On
   a machine with no HEVC-capable GPU, there is no lawful way for us to produce
   an HEVC seam.

So on some real machines, for some real files, the correct engineering answer is
that the operation cannot be performed well. The only question this ADR settles
is what we do about that.

## Decision

**Blinkify probes the machine's encoders at runtime, answers "supported" or
"not supported" per machine and per source, and where nothing qualifies it
declines the operation and says why. It never substitutes a worse encoder and
calls the result done.**

Four parts:

### 1. Runtime capability probe

On first run and whenever the sidecar or the GPU driver changes, the engine
builds an **encoder capability profile** for the machine:

- Enumerate what the bundled sidecar advertises (`ffmpeg -encoders`).
- For every hardware encoder advertised, **actually initialise it and encode a
  few synthetic frames** at the profiles and bit depths we care about.
  Advertisement is not capability: a `hevc_qsv` that is listed but fails to
  initialise on this driver is worse than one that was never listed, because it
  fails at export time on the user's real work.
- Record profile, level ceiling, supported pixel formats and bit depths per
  encoder.
- Cache the profile, keyed on sidecar build and driver version, and re-probe
  when that key changes.

The profile is a plain data structure in the engine. The renderer consumes it
through `packages/types` and never computes it (`CLAUDE.md` section 2).

### 2. Per-segment encoder selection

For a seam, the planner selects the **highest-fidelity encoder that matches the
source exactly** on every attribute listed in the Context. Hardware first —
`nvenc`, then `qsv`, then `amf` — because those are the only ones that reach
Main and High for H.264 and the only HEVC option at all. Quality parameters for
a seam are set well above the source's apparent bitrate: a seam is a fraction of
a second, and spending bits there is free relative to the file.

For a tier-3 full re-encode where the **user** has chosen the output codec, the
constraint is looser — the output need not match the source — and an LGPL
software encoder is acceptable. Hardware is still preferred for speed.

### 3. `openh264` is not bundled

We do not ship it, for two independent reasons, either of which is sufficient:

- **It does not solve the problem.** Constrained Baseline cannot match a
  High-profile source, which is the case that matters.
- **Its patent coverage would require a network call.** Cisco's royalty
  undertaking attaches to the **binaries Cisco itself distributes**, not to
  builds we compile. Taking the benefit means downloading Cisco's binary at
  runtime — which forbidden behaviour 8 in `CLAUDE.md` prohibits outright.

Shipping a self-built `openh264` would give us an encoder that cannot do the job
_and_ an unresolved patent exposure. We ship neither.

### 4. When nothing qualifies — decline, visibly

If no encoder on this machine can match the source for a seam, the smart-cut is
**refused for that cut point**. The user is told, before the export starts, in
plain language: what could not be done, why, and what their machine would need.

They are then offered exactly two honest choices, and Blinkify never picks
silently:

- **Move the cut to the nearest keyframe.** Stays tier 1, zero loss, and the
  dialog states the time difference this costs (`+0.42 s`, for example).
- **Re-encode the segment in full.** Tier 3, explicitly marked as quality loss,
  with the reason recorded in the plan and surfaced in the export report
  ([#52](https://github.com/ismetcahangirov/blinkify/issues/52)).

The refusal path is a tested behaviour, not an error message. A machine with no
usable encoder must produce a clear decline on a known corpus file — see
[#45](https://github.com/ismetcahangirov/blinkify/issues/45).

## The patent position

This is a **separate question from ADR-0002**, and a more uncomfortable one. A
permissive or LGPL copyright licence says nothing about patents. H.264 and HEVC
are both covered by large patent portfolios:

- **H.264 / AVC** — pooled and administered by **Via LA** (formed from the
  merger of MPEG LA and Velos Media).
- **HEVC / H.265** — fragmented across **Access Advance** (formerly HEVC
  Advance), a separate **Via LA** HEVC pool, and holders who have joined no
  pool. HEVC's licensing landscape is materially worse than H.264's, and this is
  one of the reasons HEVC adoption on the open web stalled.

Blinkify's position, stated so a future contributor does not have to reconstruct
it:

1. **We ship no software H.264 or HEVC encoder.** See part 3 above. There is
   therefore no encoder in the Blinkify installer that implements those
   standards.
2. **All H.264 and HEVC encoding runs on the vendor's hardware encoder**, which
   reaches us through the GPU driver the user already has — shipped and licensed
   by Intel, NVIDIA or AMD as part of the device. This is the industry's
   conventional position and it is why part 2 above prefers hardware. It is not
   a guarantee of immunity for us, and this ADR does not claim one.
3. **Decoding is a distinct exposure from encoding** and a broader one, since
   the bundled sidecar decodes both. Patent holders have historically pursued
   encoders and distributors of encoding products far more actively than
   decoders, but "historically" is not "never".
4. **This is a documented engineering position, not legal advice.** Before
   Blinkify is distributed commercially — sold, bundled, or shipped at scale —
   this section must be reviewed by someone qualified to give that advice, and
   this ADR superseded by one that records the outcome. A separate issue tracks
   that review. It is deliberately **not** a blocker for building an open-source
   product now, and it is deliberately **not** something we discover after
   taking money.
5. **AV1 is the strategic exit.** It is royalty-free under the Alliance for Open
   Media's patent licence, and it has LGPL-compatible encoders. Where the user
   chooses the output codec, AV1 is the preset we can offer with no reservation.
   It does not help at a seam, because a seam must match the source.

## Alternatives considered

### Always fall back to the closest available encoder — rejected

Use whatever encoder exists, accept a profile mismatch at the seam, and mark the
result "degraded" in the report.

Rejected for the reason the product exists. A Constrained-Baseline seam inside a
High-profile stream is not a slightly worse file — it is a file some decoders
render incorrectly or refuse. And even where it decodes, this trades a visible,
honest refusal for a silent, invisible loss. Forbidden behaviour 3 in
`CLAUDE.md` prohibits exactly that, and section 17 requires declining honestly.
A report line the user reads after the export is not consent; it is notification
after the fact.

### Require a hardware encoder, refuse the whole feature without one — rejected

Simple rule: no NVENC, QSV or AMF, no smart-cut at all, anywhere.

Rejected because it discards capability we actually have. A machine with no GPU
encoder can still stream-copy every keyframe-aligned cut, which is tier 1 and is
the majority of ordinary editing. Refusing the feature globally would also hide
the "move the cut to the nearest keyframe" option, which is often what the user
wanted anyway and costs them nothing. The refusal belongs at the cut point, not
at the feature.

### Ship a GPL FFmpeg with `libx264` and `libx265` — rejected

This makes the entire problem disappear. Both encoders reach High profile and
Main 10, and every seam becomes possible on every machine.

Rejected in ADR-0002, and not reopened here. It is the right engineering answer
and the wrong product answer: it makes Blinkify GPL permanently and forecloses a
commercial licence, for a benefit that falls entirely on the seam of
non-aligned cuts.

### Bundle a self-built `openh264` anyway — rejected

Covered in part 3. It cannot match High profile, and self-building forfeits
Cisco's royalty undertaking, which attaches only to Cisco's own binaries. It
would add patent exposure in exchange for an encoder that does not solve the
case we needed it for.

### Download Cisco's `openh264` binary at runtime — rejected

This is what Firefox does, and it is the only way to take the benefit of Cisco's
undertaking.

Rejected on two counts: forbidden behaviour 8 prohibits any network call except
the update check, and even with the download it is still Constrained Baseline
and still cannot match a High-profile seam. We would break a rule and not get
the capability.

## Consequences

### What this makes easy

- The lossless promise stays literally true. Blinkify never quietly produces a
  degraded file; the worst outcome is a refusal the user can read.
- The user learns something actionable. "Your GPU has no HEVC encoder" is a fact
  they can act on; "exported successfully" over a damaged seam is not.
- The engine's answer is specific to the machine, not a compile-time assumption
  that is wrong for half the installed base.
- Export failures move earlier. A capability profile computed at import time
  means the export dialog can be honest before the user waits.

### What this makes hard

- **The capability probe is real work**, and must survive driver updates, hybrid
  GPU laptops, a GPU that is present but busy, and encoders that advertise
  support they do not have. This is why part 1 test-encodes rather than trusting
  the advertisement.
- **The planner's matching rules are strict and must be tested against real
  files**, including 10-bit HEVC from phones and interlaced material, not
  against synthetic clips alone.
- **The UI must carry a refusal state well.** A refusal that reads as a crash is
  a worse outcome than the degradation we refused to ship. This is a design
  requirement on [#50](https://github.com/ismetcahangirov/blinkify/issues/50)
  and [#52](https://github.com/ismetcahangirov/blinkify/issues/52), not an
  afterthought.

### What we accept

- **On a machine with no HEVC-capable GPU, HEVC footage gets no smart-cut.**
  Keyframe-aligned cuts still work and are lossless. This is a real, visible
  product limitation and we state it in the interface rather than hiding it.
- **Seam quality depends on the GPU.** A seam encoded by NVENC and one encoded
  by AMF are not identical. Both match the source's profile, which is what
  correctness requires; they differ in rate-distortion behaviour across a
  fraction of a second.
- **The patent position needs legal review before commercial distribution**, and
  that review may change part 2 of this decision. It is recorded here so the
  question is inherited rather than rediscovered.
