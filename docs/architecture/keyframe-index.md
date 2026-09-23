# The keyframe index

From [#24](https://github.com/ismetcahangirov/blinkify/issues/24), Epic #3. Code:
`crates/blinkify-engine/src/keyframes/`.

Epic #6 decides between a free stream copy and a smart-cut re-encode purely
from whether a cut point lands on a keyframe, and whether the GOP it opens
depends on the one before. This index is where that answer comes from.

## What is stored

Per video stream, per keyframe, in the stream's own time base:

| Field                | Why                                                                            |
| -------------------- | ------------------------------------------------------------------------------ |
| `pts`                | Where the cut lands in presentation time                                       |
| `dts`                | Where the stream copy starts in decode order — storing only one causes offsets |
| `pos`                | Byte offset, for reading the picture and for seeking                           |
| `picture`            | IDR, BLA, CRA or recovery point, read from the first NAL unit (MP4/MOV only)   |
| `hasLeadingPictures` | Pictures presented before it follow it in decode order                         |
| `gop`                | `closed` or `open` — derived from the two above                                |

Timestamps are never computed from a frame number: a variable-frame-rate source
makes that arithmetic wrong, and the tests index one across a 30→10 fps change.

## Open or closed

```
picture is IDR or BLA                    → closed   (IDR_W_RADL leading pictures are decodable)
picture is CRA or a recovery point       → open if leading pictures follow, else closed
picture unknown (not MP4/MOV, unreadable) → open if leading pictures follow, else closed
```

The last row errs towards _open_: a wider cut window, never a corrupt cut. It
costs a smart-cut only for IDR_W_RADL streams outside MP4/MOV, where the picture
type cannot be read from the packet position `ffprobe` reports.

## Lazy, then complete

```
query(t) ── covered? ──yes──▶ answer from the map
               │no
               ▼
   ffprobe -read_intervals t%t+60s        (interactive priority)
   seeks to the keyframe at or before t, so the region is
   covered from that keyframe to t+60s, and merged in
               │
               ▼
          answer from the map

background thread: the first uncovered tick, 60 s at a time
                   (background priority), progress after each
```

`ffprobe` stops before printing the first packet past an interval's end, so
whether a region reached the end of the stream is decided from the stream's
probed length, not from the last timestamp read.

Region reads hold no lock while `ffprobe` runs; two queries into different
regions proceed at once, and a background pass never blocks a query.

## Persistence

The index is written to the on-disk cache — complete, or as far as it got when
the application closed — under the file's content key (see
[`../../crates/blinkify-engine/src/cache.rs`](../../crates/blinkify-engine/src/cache.rs)):
a hash of its first and last mebibyte, its size and its modification time.
Moving a file keeps its index; editing it discards it. The format carries a
version, and an entry with any other version is rebuilt rather than misread.
