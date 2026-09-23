# The preview decode pipeline

From issue [#27](https://github.com/ismetcahangirov/blinkify/issues/27), Epic
[#4](https://github.com/ismetcahangirov/blinkify/issues/4). How a frame gets
from the source file onto the player canvas, and the rules that keep it there
at full rate without consuming unbounded memory. The choice of transport is
[ADR-0005](../decisions/ADR-0005-preview-frames-over-a-custom-scheme.md).

```
 source file (read-only)
      │
      ▼
 ffmpeg.exe  -copyts -ss <keyframe> -noautorotate
             select=gte(pts,<first>), scale=<preview size>, format=rgba, showinfo
      │ stdout: raw RGBA          │ stderr: one showinfo line per frame (its pts)
      ▼                           ▼
 FrameAssembler ── pairs each frame with its timestamp ──┐
      │ blocks while the ring is full  ◄── backpressure  │
      ▼                                                  │
 FrameRing  (bounded: 128 MB, 3–32 frames)               │
      │ take_due(clock): newest frame at or before the   │
      │ clock; older ones dropped and counted            │
      ▼                                                  │
 PreviewSession ── WallClock (#28 replaces with audio) ──┘
      │
      ▼  http://frame.localhost/<session>/<after>   (one request in flight)
 renderer: fetch → ImageData → putImageData (offscreen, frame size)
                 → drawImage onto the visible canvas, rotated and fitted
```

## Decoding

`blinkify_engine::decode`.

- **One process per source.** An FFmpeg process decodes forward into a pipe for
  as long as playback runs forward. The FFmpeg command line cannot seek a
  running process, so a jump the process cannot decode forward to (a seek, a
  resynchronisation) replaces it — there is never more than one per source.
- **Pixel work happens in FFmpeg.** Scaling to the preview size and conversion
  to RGBA are filters in the decode graph. JavaScript receives bytes it can put
  on a canvas as they are.
- **Timestamps are the stream's own.** `-copyts` keeps them; `showinfo` at the
  end of the graph logs every frame in output order, so the nth line on stderr
  belongs to the nth frame on stdout. Frames carry that timestamp, in the
  stream's time base — never a frame number, which a variable frame rate makes
  meaningless. A test checks the decoded timestamps against `ffprobe`'s for
  CFR, VFR, edit-list and open-GOP sources.
- **Frame-exact start.** `-seek_timestamp 1 -noaccurate_seek -ss` seeks to the
  keyframe the index names (the seconds value is rounded _up_ to the
  microsecond, so the demuxer cannot land on the keyframe before), and
  `select=gte(pts,N)` discards everything before the requested frame by integer
  tick, before it is scaled or piped. The first delivered frame is compared
  pixel for pixel with an independent full decode in the tests.

## Backpressure

The ring holds at most 128 MB of frames, between 3 and 32 of them. When it is
full, `FrameRing::push` blocks the thread reading FFmpeg's stdout; the pipe
fills; FFmpeg blocks on its write and stops decoding. Nothing is buffered
anywhere else. A test starts a decode, never consumes, and asserts that the ring
holds exactly its capacity, the decoder has delivered no more than that, and the
FFmpeg process is paused — alive, not finished — until one frame is taken.

A blocked producer also watches its decoder's cancel token, so a decoder that is
being replaced — blocked on a full ring that nobody will drain until it is gone
— stops at once.

## Presentation

The engine, not the renderer, decides what is on screen.

- **Newest due frame.** `FrameRing::take_due(now)` returns the newest frame
  whose timestamp is at or before the clock and drops every older one. A late
  frame is skipped, never shown late. `dropped_frames` counts them.
- **Pre-roll.** The clock starts when the first frame is ready, not when the
  decode is requested.
- **Resynchronisation.** If the clock runs more than a second past the newest
  decoded frame — the machine cannot decode in real time — the decoder restarts
  at the next keyframe 300 ms ahead of the clock, at most once every two
  seconds. Video drops frames rather than falling behind.
- **The renderer pulls.** One request in flight; the next is sent once the
  previous frame has been drawn. The engine holds a request open until a newer
  frame is due, so that is where the pacing comes from — the renderer does not
  also wait for an animation frame, which would add up to a display interval to
  every cycle and drop frames at 30 fps. A throttled window sees fewer, newer
  frames.

Audio is not in this path. The playback clock becomes the audio clock in
[#28](https://github.com/ismetcahangirov/blinkify/issues/28), and audio decode
runs in its own process on its own thread, so a stalled video decoder cannot
stall it.

## Rotation and aspect

Frames arrive **as coded**: FFmpeg's autorotation is off. The renderer turns
them upright at draw time — a canvas transform, free on the GPU — by rotating
counter-clockwise by the probe's `rotation`. That is the rule FFmpeg's own
autorotation applies; the engine test
`a_portrait_video_arrives_as_coded_and_turns_upright_by_its_rotation` rotates a
delivered portrait frame by it and compares the result with FFmpeg's
autorotated decode, pixel for pixel, and `orientation.test.ts` checks the
canvas transform maps each corner of the frame accordingly.

An anamorphic source is widened by its sample aspect ratio in the decode graph,
so every delivered frame has square pixels and the renderer only ever rotates.

## Teardown

`PreviewSession::close` closes the ring (waking a blocked producer), cancels the
decoder, and returns once its process has exited. The shell closes every
session on `close_all_previews` — what closing a project calls — and before the
application exits; the orchestrator's job object covers an unclean exit. A test
asserts the decoder's process id is gone from the process table after close.

A source that disappears is reported rather than panicking: while a decoder
holds it Windows refuses the delete, and once nothing holds it a new decode
fails with "no longer available" in the session's statistics.

## Diagnostics

`preview_stats` returns `DecodeStats`: buffer depth against its bound, bytes
buffered, frames decoded, presented and dropped, the decode rate over the last
32 frames, resynchronisations, and the last error. The player's **Stats** button
shows them over the video.

## What this does not do yet

- **Audio** and the audio master clock: #28 and #31.
- **Seek and scrub**, and prefetch around the playhead: #29.
- **Proxies.** A session opens the original; choosing `PreviewSource::Proxy`
  when one is attached belongs with seek (#29) and the edit graph (#30).
- **HDR.** Frames are converted to 8-bit RGBA; v1 previews in SDR (Epic #4, out
  of scope).
