# ADR-0010 — The export routes packets between sidecar processes over NUT pipes

- **Status**: Accepted
- **Date**: 2026-09-25
- **Context issue**: [#40](https://github.com/ismetcahangirov/blinkify/issues/40),
  and the rest of Epic [#6](https://github.com/ismetcahangirov/blinkify/issues/6)
  (#41, #42, #43, #55)

## Context

The planner (#39) decides, per segment, what is copied and what is encoded.
Carrying that out needs one capability FFmpeg's command line does not offer:
**putting packets from several places into one output stream** — copied
packets from two files, copied packets around a re-encoded seam, video copied
while the sound beside it is encoded — with exact control over which packets
go in and what their timestamps are.

Three constraints close off the obvious answers:

- **No intermediate media file** (`CLAUDE.md` forbidden behaviour 1). "Cut
  each segment to a file, then concatenate" is exactly what is forbidden.
- **FFmpeg runs as a separate process** (ADR-0002). Blinkify does not link
  libavformat, so it cannot mux packets in-process as `smartcut` does with
  PyAV.
- **The cut must be exact.** A copy that starts on a keyframe must start on
  that keyframe's packet and must leave out the leading pictures of an open
  GOP; one that ends before a closed keyframe must end on the last packet
  shown before it. A frame too many or too few is a defect the losslessness
  suite (#45) exists to catch.

## Decision

**The engine is a packet router between sidecar processes, and the pipes
between them carry NUT.**

- Each segment's packets come from a process writing NUT to its standard
  output: for a copied segment,
  `ffmpeg -copyts -i <source> -map 0:<stream> -c copy -output_ts_offset 100 -f nut pipe:1`.
  Encoders for seams (#41), sound (#43) and whole segments (#55) are further
  such processes.
- The engine reads NUT, selects each segment's packets by their own
  timestamps, rebases them onto the output timeline, interleaves the streams
  by presentation time, and writes NUT into the standard input of **one**
  muxer process — `ffmpeg -f nut -i pipe:0 -c copy <partial output>` — which
  writes the only file the export produces.
- Readers add a fixed 100-second offset to every timestamp. NUT cannot carry
  a negative timestamp, and a source's first decode timestamps are negative
  wherever it has B-frames or an edit list's pre-roll; left to itself FFmpeg
  shifts them by an amount the engine cannot see. A known offset keeps them
  positive and exact, and the engine subtracts it.
- The output is written to `<target>.blinkify-partial` beside the target and
  renamed when the muxer has finished. A failure or a cancellation removes it.

NUT is used because it is FFmpeg's own streaming format: it carries every
codec FFmpeg can copy, the codec configuration unchanged, and every timestamp
exactly in the stream's own time base. The engine's reader handles whatever
frame-code table and elision headers FFmpeg writes; its writer writes the
simplest valid NUT (a syncpoint before every frame, full timestamps, every
frame header checksummed).

What NUT does not carry is set on the muxer: the display rotation
(`-display_rotation`), which lives in side data. Colour metadata travels in
the bitstream and is read back by the muxer; stream and file metadata and
chapters travel as NUT info packets, the chapters moved onto the output
timeline.

## Alternatives considered

### Segment files, then the concat demuxer — rejected

The standard recipe: cut each segment to its own file with `-c copy`, write a
list, and concatenate. It writes an intermediate media file per segment,
which forbidden behaviour 1 prohibits outright, and it multiplies disk I/O on
a long timeline.

### The concat demuxer with `inpoint` and `outpoint` on the sources — rejected

No intermediate file: the list names the sources and the cut points. But
`outpoint` is compared with **decode** timestamps, so with B-frames it keeps
the next GOP's keyframe and the frames decoded before it — frames past the
cut. And it can only concatenate files: a re-encoded seam would have to be a
file, which puts us back in the previous alternative.

### Link libavformat and mux in-process — rejected

This is what `smartcut` does with PyAV, and it would make packet routing a
library call. ADR-0002 keeps FFmpeg across a process boundary so that the
licence boundary, crash isolation and the bundled build's identity are
enforceable; linking reopens all three for a convenience the pipe provides.

### Matroska on the pipes — rejected

FFmpeg's Matroska muxer writes timestamps in milliseconds. A 1/90000 or
1/15360 source would be rounded on the way through, which moves frames on a
variable-frame-rate timeline and makes the engine's exact integer time
(ADR-0006) a fiction at the one place it matters.

### MPEG-TS on the pipes — rejected

Timestamps in 90 kHz only, and a codec list that excludes much of what a
phone or a WebM file contains.

### Fragmented MP4 on the pipes — rejected

Exact timestamps, and a well-known format. But the engine would need to
write `moov` sample entries for every codec and handle edit lists in the
fragments FFmpeg writes, and the format does not carry some codecs FFmpeg
copies. NUT carries all of them with a fraction of the parsing.

## Consequences

### What this makes easy

- One output pass, no intermediate file, and exact packet selection in Rust
  where it can be unit-tested.
- Timestamp work — rebasing (#40), rescaling for a constant speed (#42) — is
  arithmetic on integers the engine already reasons about.
- Seams, re-encoded sound and re-encoded segments join the same router: a
  new kind of segment is a new producer process, not a new pipeline.
- Cancellation is one switch: every process of the export belongs to it.

### What this makes hard

- The engine owns a NUT reader and writer. They are small, tested against
  truncation and corruption, and tested against FFmpeg's own output, but they
  are code a pure FFmpeg pipeline would not need.
- Packet side data does not cross NUT version 3, which FFmpeg writes by
  default. The copied audio keeps its packets unchanged; encoder-delay side
  data from the source container is not carried.

### What we accept

- Timestamps from a second source whose time base differs from the first's
  are rescaled to the first's, to the nearest tick. They are outside the hash
  boundary (#45); the frames are unchanged.
- Attachments (fonts in Matroska) are not carried yet; NUT has no stream for
  them. [#107](https://github.com/ismetcahangirov/blinkify/issues/107) adds
  them through a second muxer input where the output is Matroska.
