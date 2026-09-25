# Smart-cut

From [#41](https://github.com/ismetcahangirov/blinkify/issues/41), Epic
[#6](https://github.com/ismetcahangirov/blinkify/issues/6). A port of
[`skeskinen/smartcut`](https://github.com/skeskinen/smartcut) (MIT; notice in
[`THIRD_PARTY.md`](../../THIRD_PARTY.md)). Code:
`crates/blinkify-engine/src/export/seam.rs`, driven from `export/execute.rs`.
Built on the executor ([`export-executor.md`](./export-executor.md)) and on
profile matching ([`re-encode-profiles.md`](./re-encode-profiles.md)).

A cut between keyframes costs one small re-encoded window instead of the
whole clip. Without it, a cut lands up to a GOP away from where the user put
it; with it, cuts are frame-accurate and everything between the windows is
still a copy.

## Pieces

The planner (#39) marks a clip `smart-cut` and gives its windows. The executor
splits the clip into pieces, as upstream's `CutSegment`s:

```
source:  K0 ····· in ········ K1 ················· K2 ········ out ····· K3
                  └ recode ──┘└──────── remux ─────┘└─ recode ─┘
```

| Piece  | What is written                                                           |
| ------ | ------------------------------------------------------------------------- |
| recode | the pictures shown in `in..K1`, decoded from `K0`, encoded to match (#44) |
| remux  | the packets from `K1` to `K2` — copied packet for packet                  |
| recode | the pictures shown in `K2..out`, decoded from `K2`                        |

A clip inside one GOP is a single recode. A copy starts only on a keyframe a
decoder can start on: an H.264 **recovery point** (a non-IDR keyframe, as an
open-GOP x264 stream has) is not one — the pictures after it carry reference
commands for pictures before it, and a stream that starts there decodes with
errors even in a plain FFmpeg copy. The head window therefore runs to the next
IDR, which in an H.264 stream with open GOPs throughout can be the whole clip.

## Windows from real dependencies

- **Leading pictures after a seam.** An open GOP's keyframe (HEVC CRA, and
  RADL-carrying IDRs) has pictures shown before it and decoded after it. A
  remux that starts on it drops them — they are before the cut — and they are
  inside the head window, so the recode makes them. This is upstream's
  `hybrid_recode_cra_segment`.
- **Leading pictures before a remux ends.** A remux that ends on a keyframe
  with leading pictures cannot end cleanly there: the pictures just before it
  are decoded after it. The executor reads the packets after the keyframe,
  finds where the first leading picture is shown, and starts the next recode
  there — so the window is the leading pictures, not the GOP before them.
- **Without B-frames** every picture is decoded before the next is shown, and
  a remux may end at any frame; no tail recode is needed.

## Joining encoded and copied packets

The seam encoder is a sidecar process writing NUT (ADR-0010), trimmed to
exactly its pictures on their own timestamps
(`-vf trim=start_pts=…:end_pts=…`, `-fps_mode passthrough`,
`-enc_time_base demux`), with no B-frames so its decode order is its
presentation order.

Before a packet of it is written, its SPS is compared with the source's
(`validate_join`, #44): profile, chroma, bit depth, and a level no higher. A
mismatch aborts the export with `ExportError::Seam` and nothing at the target.

A hardware encoder cannot move its parameter-set ids away from the source's,
as upstream does with `x264 sps-id=3`. So the parameter sets travel in-band
(`Stitch`):

| Packet                                 | Carries                                          |
| -------------------------------------- | ------------------------------------------------ |
| the seam's first                       | the seam's VPS/SPS/PPS, then its picture         |
| every seam packet                      | its NAL units reframed to the stream's framing   |
| the first copied keyframe after a seam | the source's VPS/SPS/PPS again, then its picture |
| every other copied packet              | nothing added: its bytes are the source's        |

AV1 does the same with sequence header OBUs; VP9 has no parameter sets. In MP4
the sample entry becomes `avc3` or `hev1`, which allow parameter sets to change
in-band; Matroska needs nothing.

The payload of every copied packet outside the windows is the source's own,
with the one exception of the parameter sets re-sent before the keyframe that
follows a seam: those are the stream's configuration, not its pictures, and
the losslessness suite (#45) compares payloads without them.

## When there is no encoder

Where no encoder on this machine makes the source's kind of stream — H.264 or
HEVC without a matching GPU encoder (ADR-0003), an HDR source (ADR-0008) — the
planner declines the smart-cut and offers the nearest keyframe-aligned cut
(`KeyframeAlternative`), which stays a lossless copy. The export refuses a
declined plan before anything runs; it never writes a mismatched seam.
