# Seek and scrub

From issue [#29](https://github.com/ismetcahangirov/blinkify/issues/29), Epic
[#4](https://github.com/ismetcahangirov/blinkify/issues/4). How the player lands
on exactly the frame under the playhead, how a drag keeps up with the pointer,
and how a preview proxy stands in for the file without changing which frame is
shown. Playback itself is [`playback.md`](./playback.md).

## The two-stage seek, once

`blinkify_engine::seek` is the seek, for every caller:

1. **Plan** (`seek::plan`): from the keyframe index — never a scan of the file —
   find the frame on screen at the position (the newest frame whose timestamp
   is at or before it, from the frame table) and the keyframe at or before that
   frame. For a leading picture of an open GOP, shown before its keyframe but
   decoded after it, that is the _previous_ keyframe, which is the one its
   references need.
2. **Decode** from the keyframe and discard, inside FFmpeg, every frame before
   the target (`select=gte(pts,N)` on integer ticks). The first frame delivered
   is the target.

The player's lanes start every decoder this way, and `seek::decode_frame`
decodes a single frame for anyone who wants one picture. The smart-cut port
([#41](https://github.com/ismetcahangirov/blinkify/issues/41)) takes the same
`SeekPlan` to find the window a cut depends on — `plan.keyframe` to
`plan.frame` — rather than working it out a second time.

Positions are timestamps from the index, never a frame number times a frame
duration, so a variable frame rate is exact; and the index reads the container
the way FFmpeg decodes it, so an MP4 edit list is exact too. The test compares
the frame decoded for a spread of timestamps — on frames, between frames, on
keyframes, before the first and in the last GOP — with an independent full
decode, pixel for pixel, on H.264 with B-frames, HEVC open and closed GOPs, a
variable-frame-rate screen recording, an edit-listed MP4 and VP9.

## Scrubbing: serve the latest, drop the rest

A drag produces far more positions than can be decoded, and queueing them is
what makes scrubbing feel broken — the picture trails further behind the
pointer with every move. So:

- **One slot.** `TransportCommand::Scrub` replaces whatever position was
  waiting. A worker thread takes the newest; positions a newer one overtook
  are never decoded. A test sends 400 positions in about 400 ms and asserts
  that fewer than a fifth of them cost a decode, and that the last one is the
  frame on screen.
- **Windows into a cache.** Each decode is the two-stage seek to the target,
  continued for up to 48 frames, and every frame goes into a cache keyed by
  segment and timestamp. Three quarters of the window lie in the direction the
  pointer is moving, so the next positions of a steady drag — forwards or
  backwards — come from the cache without a decode.
- **Bounded.** The cache holds at most the same 128 MB the playback lanes may,
  and evicts the oldest frames first. Prefetch cannot bring back the memory
  problem the ring bound exists to prevent; a test scrubs a 1080p source
  forwards and backwards and checks the bound at every step.
- **Progress over perfection.** A window is not abandoned before its target
  frame is on screen — otherwise a fast drag over a long-GOP source would show
  nothing at all — but once it is, a newer position the window will not reach
  soon stops it.

The drag ends with a `Seek`, which hands the screen back to the playback lanes
at exactly the frame under the playhead.

## Saying that a seek is still on its way

On a keyframe-dense source a seek is one or two decodes and is not noticeable.
On a camera's long GOP it can be most of a second. `PlaybackStatus.resolving`
is true from the command until its frame is on screen, the engine sends a
`media://playback` update when it clears, and the player shows "Finding the
frame…" over the picture it will replace — so a frozen frame is never mistaken
for the answer.

## Proxies

A source with a proxy (#26) is previewed from it. The proxy's ten-second
segments are decoded as one input through an FFmpeg concat list written beside
them (relative names, so a user directory with a quote in it needs no
escaping). Three things keep the proxy honest:

- **The same frame.** The proxy is timed in Matroska milliseconds. Each proxy
  frame is mapped back to the source frame nearest its time, through the frame
  table, so the frame shown — and every step, seek and timecode — is the one the
  original would give. The test seeks and steps through a proxy and compares the
  source timestamps shown with the file's own.
- **Upright.** Proxies were generated with FFmpeg's autorotation, so their
  frames are drawn without further rotation.
- **Said, always.** `PlaybackStatus.proxy` is true whenever the picture at the
  position comes from a proxy, and the player shows a persistent **Proxy**
  badge. A proxy is a 540-line copy; nobody should judge detail on it believing
  it is the file. Sound, timing and every frame decision still come from the
  file, and export cannot read a proxy at all (`ExportSource`).

A proxy made while a preview is open is used from the next open; making one is
offered, never done on the user's behalf (`proxy_reasons`).
