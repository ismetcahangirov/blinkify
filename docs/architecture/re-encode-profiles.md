# Re-encode profile matching

From [#44](https://github.com/ismetcahangirov/blinkify/issues/44), Epic
[#6](https://github.com/ismetcahangirov/blinkify/issues/6). Why decline rather
than degrade:
[ADR-0003](../decisions/ADR-0003-re-encode-encoder-strategy.md). Code:
`crates/blinkify-engine/src/export/profile.rs` and `export/sps.rs`.

A re-encoded stretch that joins copied packets in one stream — a smart-cut
seam (#41), a re-encoded segment between copies (#55) — must be the same kind
of stream as what it joins. A mismatch plays for a second and then breaks up,
and passes a test that only checks the file plays.

## Three questions, answered before anything is encoded

```
source probe ──▶ source_profile ──▶ SourceProfile ─┐
                                                   ├─▶ select ──▶ EncoderChoice ──▶ arguments
capability probe (#21) ──▶ EncoderCapabilities ────┘        │
                                                            └─▶ Unmatched (refusal, with the reason)

source configuration ─┐
                      ├─▶ validate_join ──▶ Ok, or JoinMismatch (abort before writing)
encoded configuration ┘
```

### What the source is

`source_profile` reads codec, profile, level, pixel format, bit depth, size,
colour (primaries, transfer, matrix, range) and time base from the probe. It
refuses outright what no encoder is asked to match:

| Source        | Answer                                                       |
| ------------- | ------------------------------------------------------------ |
| HDR           | `Unmatched::Hdr` — copied as recorded or declined (ADR-0008) |
| interlaced    | `Unmatched::Interlaced` — encoders are probed progressive    |
| not 4:2:0     | `Unmatched::Chroma`                                          |
| another codec | `Unmatched::Codec`                                           |

### Which encoder can make it

`select` walks the machine's encoders for the codec in ADR-0003's order —
NVENC, Quick Sync, AMF, then software — and takes the first whose probed
capabilities include **this profile, at this bit depth, with a level ceiling
at or above the source's**. An encoder that falls short is passed over. When
none qualifies, the refusal names the closest miss: no encoder at all, the
profile, the bit depth, or the level.

There is no software H.264 or HEVC encoder (ADR-0003 part 3). On a machine
without one of those GPUs, H.264 and HEVC re-encodes are refused; VP9 and AV1
are matched by `libvpx-vp9` and SVT-AV1 everywhere.

### How the encoder is asked

`EncoderChoice::arguments` states everything, rather than accepting the
encoder's defaults:

- the encoder, pixel format (the one its probe accepted for the profile),
  profile and level — the source's level, in the encoder's own numbering;
- colour primaries, transfer, matrix and range, wherever the source states
  them, so a seam does not flash a different colour;
- quality far above the source's — a seam is a fraction of a second, so its
  bits are free — but capped at 90% of the level's maximum bitrate (H.264
  Table A-1, HEVC Table A.8), so the encoder accepts the level and the seam
  never claims a higher one than the stream it joins.

### Whether what it made can join

`validate_join` compares the encoded stretch's codec configuration with the
source's, **before a packet of it is written**: profile, chroma format and bit
depth must be equal and the level no higher. Both are reduced to their
sequence parameter set first — the source's arrives as an `avcC`/`hvcC`/`av1C`
record, an encoder's as Annex B — and the same parser (`sps.rs`, Exp-Golomb
with emulation prevention removed) reads both, so the answer cannot depend on
the packaging. VP9 has no configuration record; its profile, depth and chroma
are in every keyframe and are matched by the choice itself.
