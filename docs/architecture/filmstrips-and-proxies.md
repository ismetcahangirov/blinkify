# Filmstrips and preview proxies

From [#26](https://github.com/ismetcahangirov/blinkify/issues/26), Epic #3.
Code: `crates/blinkify-engine/src/filmstrip.rs`, `crates/blinkify-engine/src/proxy.rs`.

Two cheaper representations of a video, generated ahead of time so the
timeline and the preview never wait on a full decode.

## Filmstrips

Thumbnails along a clip, at a fixed height, one every _interval_ seconds.

- **One FFmpeg process per filmstrip**: `fps=1/interval, scale, tile=10x10`,
  emitting JPEG sprite sheets to a pipe. Never a process per thumbnail.
- **Sprite sheets**, 100 thumbnails each, not thousands of files — slow to write
  on Windows and miserable to evict.
- **Progressive**: each sheet is written to the cache and announced
  (`media://filmstrip`) as soon as FFmpeg finishes it; the timeline fills in
  from the start of the clip while the end is decoded.
- **Interval from zoom**: `interval_for_zoom(px_per_second, tile_width)` rounds
  down to a power of two, so neighbouring zoom levels share a filmstrip instead
  of each generating its own.
- Cached under the content key, in the same budget and LRU eviction as
  waveforms and keyframe indices.

## Proxies

For sources too heavy to scrub — above 2560×1440, or above 60 Mbit/s — a proxy
is **offered** (`proxy_reasons`), never made unasked.

- **540 lines, MJPEG, all-intra**: every frame is a keyframe, so a scrub is one
  decode. Larger on disk than a long-GOP proxy, which is the trade that buys the
  seek. Measured on a 4K long-GOP source: median scrub 1.12 s from the original,
  47 ms from the proxy.
- **Ten-second segments.** A cancel deletes only the segment being written; the
  finished ones stay, and the next run resumes after them.
- **Their own directory and budget** (50 GiB), not the artefact cache's: an hour
  of 4K proxy is gigabytes, and sharing a budget would let it evict every
  waveform.

### A proxy can never reach an export

This is the one way a proxy could silently destroy quality, so it is a type,
not a flag:

```
MediaAsset ── export_source() ──▶ ExportSource   (private field: the original, always)
           └─ preview_source() ─▶ PreviewSource  (Original | Proxy)
```

`ExportSource` has one constructor and no conversion from a proxy; export code
takes an `ExportSource`, so it cannot be handed one. `compile_fail` doc tests
prove the conversion and the construction do not compile, and an integration
test locks every proxy file exclusively and shows a stream copy of the export
source still succeeds.

`PreviewSource::is_proxy()` is what the preview panel uses to say, persistently,
that it is showing a proxy.
