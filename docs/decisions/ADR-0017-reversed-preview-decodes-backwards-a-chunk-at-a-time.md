# ADR-0017 — Preview a reversed clip by decoding it backwards a chunk at a time

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#113](https://github.com/ismetcahangirov/blinkify/issues/113),
  following [#55](https://github.com/ismetcahangirov/blinkify/issues/55) and
  [#30](https://github.com/ismetcahangirov/blinkify/issues/30)

## Context

Hold and reverse are editable (#35) and exported (#55), but the preview
showed both as gaps. The preview and the export must not disagree about what
the same graph means (#30), so the preview has to show, at every sequence
frame, the frame the evaluator names there (`Placement::source_at`) — the
frame the export writes.

A held clip is easy: one frame, decoded once. A reversed clip is not:

- **No decoder runs backwards.** Frames depend on the ones before them, so
  the only way to a frame is forwards from a keyframe.
- **Memory.** Decoding a clip to turn it round holds all of it; ten minutes
  at preview size is gigabytes. The export has the same trap and decodes a
  chunk at a time (#55).
- **Latency.** Playback cannot wait for a chunk to decode at every chunk
  boundary, and a seek into a reversed clip should cost about what a seek
  forwards does.
- **Truth of timestamps.** The presenter picks the due frame by its source
  timestamp. A frame must carry its own.
- **Sound.** A reversed clip is heard backwards, through its audio chain, as
  the export writes it: chain first, then reversed, then the speed.

## Decision

**A reversed clip's lane decodes it backwards a chunk at a time, in the
engine, keeping every frame's own timestamp; the next chunk decodes while the
current one plays; and the presenter takes the due frame in the other
direction. Its sound is decoded the same way in chunks of source time.**

- **A chunk** ends where the previous one began and reaches back at most
  `chunk_frames` frames (32 MB of pictures, 4 to 30 frames), starting at the
  earliest keyframe within that reach if there is one. With ordinary groups
  of pictures a chunk is a group — nothing decoded twice; with a group longer
  than the reach, chunks inside it decode from its keyframe and discard what
  they do not need, so the bound holds however long a group is.
- **Each chunk is an ordinary forward decode** through the preview's two-stage
  seek (`start_decoder`), proxy included, run to exactly its frame count. The
  frames are collected and handed to the lane's ring newest first. Nothing is
  written anywhere: the chunk lives in memory until it is shown.
- **Prefetch.** The next chunk's process starts before the current chunk's
  frames go into the ring, so it decodes while they play.
- **The bound** is the ring plus two chunks — `reverse_memory_bound` — for any
  clip length. The lane measures what it holds and a test asserts the peak
  stays under the bound on a 20-second clip at double speed.
- **Falling behind skips ahead.** The ring records the position the presenter
  last asked for; a chunk the clock has already passed is not decoded.
- **Presentation.** `FrameRing::take_due_backwards` takes the newest frame at
  or before the evaluator's tick and drops every newer one as late — the
  mirror of forwards. A held or reversed segment maps a timeline position to
  its sequence frame and asks `Placement::source_at` for the tick; nothing
  re-derives the motion.
- **Sound.** Chunks of two seconds of source, from the playback position down
  to the in point. Each is one process: trim with half a second of priming
  before it, resample, the clip's chain, cut the priming, `areverse`, then
  `atempo` — the export's order. Each chunk is cut or padded to the samples its
  span makes, counted from the start, so tempo rounding never drifts. The next
  chunk decodes while this one plays.
- **A hold** decodes its one frame and shows it at every sequence frame of its
  length, silent: its placement is left out of the sound tracks.

## Alternatives considered

### FFmpeg's `reverse` filter per chunk, as the export does — rejected

The export feeds a chunk through `reverse` and writes raw frames to an
encoder at a fixed rate; it never needs to know which frame is which. The
filter re-stamps its output with the input's timestamps in their original
order, so the preview would receive frame 89 labelled as frame 60's time. The
presenter chooses frames by timestamp, so every frame would have to be
re-labelled by position, which is exactly the frame-count inference the
preview forbids (`decode` module: timestamps from `showinfo`, never inferred).
Collecting the chunk in the engine keeps each frame's own timestamp for the
cost of holding one chunk, which `reverse` holds anyway.

### Decode the whole clip, or a whole group of pictures, into memory — rejected

The whole clip is unbounded. A whole group is bounded only by the source's
keyframe interval, which is ten seconds or more on screen recordings and some
cameras. Groups are still the preferred chunk boundary; they are not the
bound.

### Render a reversed copy to disk and play it forwards — rejected

A scratch render of the user's media is exactly what `CLAUDE.md` §20.1
forbids, it costs a full decode and encode before the first frame, and it
would show the encoder's picture rather than the source's.

### Seek to every frame individually — rejected

Each frame would be a separate decode from its keyframe: quadratic in the
group length and a process per frame. Far too slow to play.

### Reverse the whole clip's sound in one `areverse`, as the export does — rejected for the preview

Bounded by the export's ten-minute limit, but that is 230 MB and a full
decode before the first sample. Chunks of two seconds start in a few tens of
milliseconds. Their cost is a join every two seconds where the audio chain
restarts; half a second of priming makes the limiter and denoiser settled at
the join, and the test hears no gap across it.

## Consequences

### What this makes easy

- Held and reversed clips preview as the export renders them, and the
  diagnostic view no longer marks them as not previewed.
- Scrubbing, stepping (by sequence frame, one frame per sequence frame as
  the export writes them) and proxies work through the same paths.

### What this makes hard

- A seek into a reversed clip decodes a whole chunk before its first frame:
  up to a second of pictures, longer than a forward seek.
- Each chunk is a process. On long groups of pictures some frames are decoded
  more than once.

### What we accept

- On sources whose timestamps are rounded — Matroska's milliseconds — the
  evaluator can name the previous frame at some sequence frames, forwards and
  backwards, where the export writes the right one. That is the evaluator's
  arithmetic, not this decision's, and is
  [#134](https://github.com/ismetcahangirov/blinkify/issues/134).
