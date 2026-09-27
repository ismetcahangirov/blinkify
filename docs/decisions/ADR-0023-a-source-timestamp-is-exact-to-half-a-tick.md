# ADR-0023 — A source timestamp is exact to half a tick

- **Status**: Accepted
- **Date**: 2026-09-27
- **Context issue**: [#134](https://github.com/ismetcahangirov/blinkify/issues/134)

## Context

The evaluator (#30) names, for every sequence frame, the source tick on screen
there, and the preview shows the frame at or before that tick. It rounded the
sequence frame's moment **down** to a whole source tick, and counted a clip's
length in frames **up**, a partial last frame included. On a time base that
holds every frame exactly — MP4's 1/15360 at 30 fps — that is right.

Matroska and WebM store milliseconds. A 30 fps file's frames sit at 0, 33, 67,
100 ms: the muxer rounded 66.67 ms to 67. Sequence frame 2 is 66.67 ms, which
rounds down to 66, and the frame at or before 66 is the one at 33. So the
evaluator named the previous frame at every sequence frame whose moment is not
a whole millisecond, while the export wrote the right one:

- **forwards**, a copy writes the source's frames in order, and a render's
  `fps` filter rounds to the nearest slot;
- **backwards**, a chunked `reverse` writes frame N − 1 − n at sequence frame
  n. The evaluator's `2999 − floor(66.67) = 2933` is frame 88's timestamp
  exactly, one frame late;
- **held**, the export's `trim=start_pts=in, select=eq(n,0)` took the first
  frame **at or after** the in-point, and the preview the frame at or before
  it. They agree only when the in-point is a frame's own timestamp.

The preview showed what the evaluator named, so on a Matroska source it showed
a frame the export did not write. `tests/preview_motion.rs` proved it by
switching its source from MP4 to Matroska.

## Decision

**A source timestamp is exact to half a tick, and every conversion between
sequence frames and source ticks is made at that precision.**

1. A sequence frame's moment is taken to the **nearest** source tick
   (`Placement::ticks_into`), halves away from zero, which is how FFmpeg's
   muxers round a timestamp in the first place. `Placement::source_at` names
   `source_in + nearest` forwards and `source_out − 1 − nearest` backwards.
2. A span of source ticks lasts `ceil((2 × ticks − 1) / 2)` sequence frames
   (`Placement::frames_until`): a partial last frame counts only when it is
   longer than half a tick. This is the inverse of (1): the first frame whose
   nearest tick reaches `ticks`. A clip's length, a split's halves, a trim's
   new length and the frame at which a keyframe first shows all use it, so
   they keep agreeing with the evaluator.
3. The preview's forward segments take a timeline position to the nearest
   tick too, never past the last tick they play, and end no later than the
   clip's sequence frames, so a clip whose range ends less than half a tick
   into a frame does not overlap the clip after it.
4. The export's hold shows the frame **at or before** its in-point. FFmpeg
   has no filter that looks ahead, so every frame at or before the in-point
   is stamped 0 and every later one a second, and `fps`, at the sequence's
   rate, emits the frame it holds for 0 when a later one arrives. A `tpad`
   clone after the last frame is the later one for a hold on the last frame.
   Verified with the sidecar on a Matroska file: an in-point of 66 ms holds the
   frame at 33, of 67 ms the frame at 67, of the last frame's timestamp the
   last frame.

On an exact time base none of this changes an answer: a sequence frame's
moment is a whole tick, the nearest tick is that tick, and a span that ends on
a frame boundary is a whole number of frames either way. The existing agreement
tests pass unchanged on MP4.

## Alternatives rejected

- **Snap the moment to the frame table.** The evaluator could look up the
  nearest frame in the keyframe index's frame table. It knows no source, by
  design (#30): it is pure arithmetic over the graph, shared by the preview
  and the export, and it must not read media. The tolerance rule gives the
  same answer without it.
- **Change only `source_at`.** Rounding the moment to the nearest tick and
  leaving lengths rounded up breaks the split: the cut is the tick on screen,
  and a left half ending on a rounded-up tick counts one frame more than it
  covers, so a split on a millisecond source is refused or overlaps its right
  half. The two roundings are one pair and change together.
- **Keep the hold at or after the in-point and make the preview match it.**
  The hold is made by freezing the frame on screen at the playhead, which is
  the frame at or before. Showing the next one would freeze a frame the user
  never saw there.
- **Find the held frame by reversing up to the in-point.** `trim=end_pts,
reverse, select=eq(n,0)` gives the frame at or before, but holds every frame
  from the seek point, three seconds before the in-point: gigabytes at 4K.
- **Probe the frame at export time.** An `ffprobe` of the packets before the
  in-point would find it, but it is another process per hold and the render
  job is built from the plan without running anything.

## Consequences

- Forwards, backwards and held, the preview on a Matroska source shows the
  frame the export writes at each sequence frame
  (`on_millisecond_timestamps_every_frame_shown_is_the_frame_the_export_writes`).
- A clip whose source range ends less than half a tick into a frame is one
  frame shorter than before, on rounded time bases only: that frame showed
  nothing the source has there.
- A hold whose in-point is between two frames exports the earlier one, where
  it exported the later.
