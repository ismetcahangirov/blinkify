# ADR-0021 — One crop filter, applied in the orientation each decoder delivers

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issues**: [#128](https://github.com/ismetcahangirov/blinkify/issues/128),
  [#129](https://github.com/ismetcahangirov/blinkify/issues/129), Epic
  [#125](https://github.com/ismetcahangirov/blinkify/issues/125)

## Context

ADR-0019 stores a crop as a rectangle in the source's display pixels: the
picture upright, as the user drew it. Two consumers turn that rectangle into
pixels. The export renderer (#128) re-encodes the cropped clip, and the
preview (#129) shows it. Epic #125 requires one filter for both, so that the
preview and the export cannot disagree about which pixels a crop keeps.

Neither consumer decodes the picture upright, and they did not agree on how
they decoded it:

- The preview decoder runs FFmpeg with `-noautorotate`. A portrait phone clip
  arrives as its coded landscape picture, and the renderer turns it at draw
  time (#27).
- The full re-encode executor (#55) decoded **with** FFmpeg's default
  autorotation, so its pictures arrived upright. The muxer then wrote the
  first video source's display matrix on the whole output stream. A rendered
  segment of a rotated clip was therefore turned twice and played sideways.
  The smart-cut seam encoder has the same flaw, and it was checked on this
  machine: a seam of the corpus's portrait phone clip was encoded 720 × 1280,
  upright, under a 90° matrix, so it displays 1280 × 720 on its side (#142).
- A proxy (#26) is made upright and smaller, at 540 lines.

Before anything was built, the mapping was checked against FFmpeg on a VP9
file carrying each of the four display rotations. For every rotation, the
pixels of "autorotate, then crop the display rectangle" matched those of
"crop the mapped rectangle of the coded picture, then turn it" by checksum.

## Decision

**One function turns a display-pixel rectangle into the FFmpeg `crop` of the
picture a decoder delivers: `picture_filter::Crop::filter`. The preview
decoder and the export renderer both call it, and both decode without
autorotation. The export renders every picture in the output stream's coded
orientation.**

- **The mapping.** A picture shown turned counter-clockwise by `r` degrees is
  cropped in its coded pixels. With a coded width `W`, height `H`, and a
  display rectangle `(x, y, w, h)`:

  | `r` | coded `x`     | coded `y`     | coded size |
  | --- | ------------- | ------------- | ---------- |
  | 0   | `x`           | `y`           | `w × h`    |
  | 90  | `W − (y + h)` | `x`           | `h × w`    |
  | 180 | `W − (x + w)` | `H − (y + h)` | `w × h`    |
  | 270 | `y`           | `H − (x + w)` | `h × w`    |

  Coded sizes of a subsampled picture are even, so a rectangle on the chroma
  grid stays on it. The filter is `crop=…:exact=1`, because FFmpeg would
  otherwise round a subsampled picture's offsets to its grid on its own.

- **The crop comes before any scale.** In the export it is the first filter
  after the trim, before the scale to fit the sequence. In the preview it is
  the first filter after the frame selection, before the scale to the preview
  surface. The decoded frame is the cropped picture, so its size is the
  cropped size, and the frame scheme (ADR-0005) already carries a size per
  frame. The renderer draws that frame at its own shape and never cuts a
  sub-rectangle out of a whole frame.

- **The export renders in the output's orientation.** The output's display
  rotation (`export::render::output_rotation`) is that of the first video
  segment whose packets are copied, a copy or a smart-cut, because a copied
  packet cannot be turned. Where every picture is rendered, the rotation is
  0 and the output is encoded upright. A rendered picture is cropped as coded
  and then turned by the difference between its source's rotation and the
  output's (`picture_filter::turn`). It is fitted to the sequence's shape
  turned into that orientation. A portrait clip rendered between copies of
  itself is never transposed. The muxer writes the same rotation.

- **A proxy is mapped, not stored.** The rectangle is scaled onto the proxy's
  pixels, which are upright at `Proxy::frame_size`. Every edge is rounded
  outwards, then out again to an even pixel, and the result is clamped to the
  proxy's frame. The proxy then shows every source pixel the crop keeps, and
  at most three proxy pixels more on each side.

- **A crop change is like a chain change.** It moves nothing in time. While
  playing, the sound takes over without a gap (#47, #49). Only the changed
  segment's picture decoder is replaced, at the clock's position.

## Alternatives considered

### Autorotate in both consumers and crop in display pixels — rejected

With autorotation, the mapping would disappear, because the rectangle would
already be in the decoded picture's orientation. But the preview would pay a
transpose on every frame of every portrait clip, which #27 avoided on
purpose. The export would still have to turn the picture back to join copied
packets that keep their display matrix. Both consumers would also depend on
FFmpeg's autorotation rules, which the renderer's canvas would have to match
separately.

### Crop in the renderer's canvas — rejected

Drawing a sub-rectangle of the whole decoded frame looks the same until a
proxy, a rotation or a scale-to-fit makes the two definitions disagree. It
also decodes and transfers pixels that are then thrown away. The issue
(#129) rules it out.

### Render every picture upright and drop the display matrix — rejected

The copied packets cannot be turned, so a stream that holds copies has to
keep their matrix. An upright rendered segment in that stream would play
sideways, which was the #55 behaviour this decision replaces.

### Keep the output's rotation as the first video source's — rejected

This was the earlier rule. When the first video segment is rendered, for
example a portrait clip letterboxed into a landscape sequence, its rotation
would be written over copies that were never turned. The rotation now comes
from the packets that actually cannot change.

### Let FFmpeg round the crop to the chroma grid — rejected

`crop` without `exact=1` silently moves an odd offset. The edit layer already
guarantees the grid for a source (ADR-0019), so rounding there could only
hide a bug.

### A crop filter per consumer — rejected

This is the audio chain's argument again (ADR-0011). Two implementations of
the mapping eventually disagree by a pixel or a corner, and nothing would
test that they agree.

## Consequences

### What this makes easy

- The preview and the export crop with the same filter, and a test compares a
  preview frame with the exported frame for rotation 0 and rotation 90.
- Rendering a rotated clip is correct for every render, including holds,
  reverses, speed and shape. It no longer depends on whether the clip is
  cropped.
- #131 (dragging the crop on the preview) draws on the upright picture that
  the preview already shows, and changing the rectangle while playing costs
  one picture decoder.

### What this makes hard

- Every new picture filter has to be placed with the orientation in mind: it
  runs on the coded picture before the turn, or on the output's orientation
  after it.

### What we accept

- Smart-cut seams still decode with autorotation. A seam of a rotated source
  is encoded upright, into packets coded the other way. That is bug #142 in
  the smart-cut path, and this decision is the rule its fix follows.
- Copy eligibility (ADR-0008) compares display shapes and not rotations. Two
  sources with the same display shape but different coded orientations could
  in principle be copied into one stream. That was true before this decision
  and is not changed by it.
