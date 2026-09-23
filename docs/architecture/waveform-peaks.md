# Waveform peaks

From [#25](https://github.com/ismetcahangirov/blinkify/issues/25), Epic #3.
Code: `crates/blinkify-engine/src/waveform/`.

The timeline draws audio at every zoom level. Decoding audio per repaint is not
viable, so each audio stream is decoded once per file version into peaks, and
every repaint reads those.

## What a peak is

Per bucket of samples, per channel: the **minimum and the maximum**. Not RMS,
which averages away exactly the clipping and transients a user zooms in to find.

Samples are decoded as 32-bit float at the source's own rate and channel count
— resampling would move peaks — and quantised into `i16` with a clamp, so a
sample at or past full scale lands on ±32767 and a clipped passage shows a flat
top at every zoom.

## The pyramid

```
level 0   256 samples/bucket     the base, from the decode
level 1   512                    each bucket = union of two below
level 2   1024
  …                              until a level has ≤ 512 buckets
```

A zoom of _z_ samples per pixel reads the coarsest level whose bucket is no
wider than _z_ — at least one bucket per pixel, so nothing is thrown away that
the screen could show, and nothing is decimated at draw time. The tests draw a
2 000-pixel window of a one-hour file at every zoom from one sample per pixel
to the whole hour, each within one 60 fps frame.

The summed view for display is the lowest minimum and highest maximum across
channels, per bucket.

## Storage

A versioned binary file in the content-keyed cache
([`keyframe-index.md`](./keyframe-index.md#persistence) describes the key),
one per audio stream. The header carries a format version; a file of any other
version is rejected and regenerated rather than misread, and every length in it
is checked against the bytes present. Layout: `waveform/format.rs`.

## To the renderer

`generate_waveform` returns at once with `pending`, and `media://waveform`
events carry progress and then `ready`: the timeline draws a placeholder, not an
empty track, until then. `waveform_peaks` returns one screen of summed peaks as
raw little-endian `i16` pairs — binary, because a screen is thousands of numbers.
