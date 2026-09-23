# Playback: the audio clock, the transport, and clip boundaries

From issue [#28](https://github.com/ismetcahangirov/blinkify/issues/28), Epic
[#4](https://github.com/ismetcahangirov/blinkify/issues/4). How the player keeps
picture and sound together, moves by exact frames, and crosses from one clip to
the next without a gap. The decode path underneath is
[`preview-pipeline.md`](./preview-pipeline.md).

```
          PlaybackPlan: segments of sources placed on the timeline
                 │                                  │
       audio lanes (feeder)                  video lanes (presenter)
   one FFmpeg per segment → f32         one FFmpeg per segment → RGBA
                 │                                  │
   feeder thread: timeline → samples         FrameRing per lane
   anchors every jump ──────────► PlaybackClock ◄── take_due(clock)
                 │                        ▲
           OutputBuffer ── sink pulls ────┘ heard(now)
                 │
   DeviceSink (WASAPI via cpal) │ SilentSink (real time, no device)
```

## The plan

`blinkify_engine::playback::PlaybackPlan` is what the player plays: segments,
each a source range `[in, out)` in the source's own time base, placed at a
timeline position in microseconds. The player never reads the edit graph; the
shared evaluator (#30) will turn the graph into a plan, so the graph has one
interpretation. Until then a plan is a whole file, or built by hand in tests.

A segment converts between the two times with opposite roundings: a frame's
place on the timeline rounds **up**, a timeline position's source tick rounds
**down** (`crate::time`). That is what makes a frame survive the round trip —
`time.rs` asserts it for every tick of a second in four time bases.

## Audio is the clock

The clock is read from sound, never from a timer or from frame delivery. Audio
cannot be stretched without artefacts; a frame can be dropped or held. So:

- The **feeder** thread walks the timeline in output-sample steps, writing each
  segment's audio (or silence for gaps and silent segments) into the
  `OutputBuffer`, about 200 ms ahead of the speaker.
- The **sink** pulls from the buffer and reports when the first frame of each
  pull will be heard — for a device, `callback + (playback − callback)` from
  WASAPI's timestamps, which is the device latency.
- `OutputBuffer::heard(now)` is the frames heard by `now`: the last report
  advanced at the sample rate, and never more than was actually consumed. A
  starved device stops the clock rather than letting video run ahead.
- Wherever the timeline jumps — play, a seek, a loop, a speed change — the
  feeder records an **anchor**: output frame N is timeline position T at speed
  S. The position is the anchor plus the frames heard since it. It is computed,
  never accumulated, so it cannot drift.

Every jump also starts a new **generation**. Video lanes are tagged with the
generation they serve, so the decoder prepared for the far side of a loop is
not confused with the one playing now.

## Video follows

Each segment being shown gets a **lane**: a decoder and its frame ring (#27).
The presenter asks the clock for the position, maps it to the segment's source
tick, and takes the newest frame due. The lane for the next segment is started
1.5 s before the boundary, so the first frame after it is already decoded.
Frames past a segment's out point are never shown, even though the decoder runs
on beyond it.

Every `ShownFrame` records `chosen_at`, the clock at the moment it was chosen.
`chosen_at − position` is the A/V sync, measured where it is decided rather
than by a test thread that may be descheduled.

## Boundaries are seamless

The feeder cuts its chunks at every boundary. The next segment's audio decoder
was started ahead of time, from exactly the next segment's in point: `atrim`
with `-copyts` cuts at the sample, after a seek half a second early so the
decoder has converged. The test compares the samples either side of a boundary
with an independent decode of the source and pins them to the exact sample.

## Frame steps

A step moves to the next or previous **real frame** of the source, from the
keyframe index's frame table (every shown packet's timestamp, #24 extended in
#28) — never by a nominal frame duration, which a variable frame rate makes
wrong. At a segment's end it moves to the next segment's first frame; at its
start, to the previous segment's last. A step forward within what the lane has
already decoded only moves the clock; anything else restarts the lane. The
tests step through CFR and VFR sources and compare each frame with the
`ffprobe` frame list and, by pixels, with an independent decode.

The frame at a paused position is the frame starting there, so the timecode —
`floor(position × rate)`, formatted non-drop-frame at the nominal rate — names
the frame on screen. The renderer formats the frame number each frame carries;
it computes none of it.

## Transport

`TransportCommand`, one IPC command (`transport`) for all of it: play, pause,
toggle, stop, step by n frames, jump to start and end, seek, preview speed,
loop. Play waits up to 3 s for the first frame before starting the clock, so a
slow start costs a moment of waiting rather than the first frames.

- **Preview speed** is 0.25×, 0.5×, 1× or 2×: the anchor's speed, and `atempo`
  on the audio decoders, which keeps pitch so speech stays intelligible. It is a
  monitoring convenience and has nothing to do with the lossless speed change in
  Epic #6.
- **Loop** plays `[start, end)` over and over. At the end the feeder anchors
  back to the start in a new generation, with the start's audio and video
  prepared 1.5 s before.
- **The end** stops the clock on the last frame (`Ended`); Play from there
  starts again from the start.

State changes the engine makes on its own — the end, a new audio device — reach
the renderer as the `media://playback` event.

## When there is no device, or it goes away

`AudioChoice::Device` opens the default output device through
[`cpal`](https://github.com/RustAudio/cpal) (WASAPI shared mode). If there is
none, or it cannot be opened, the player uses a `SilentSink`: it consumes the
buffer at exactly the rate a device would and reports when each frame "plays",
so the clock and the picture carry on, in silence. The status says which, and
why.

A device that fails mid-playback sets a flag the player's monitor checks every
50 ms; it then reopens the default device (or falls back to silence) and
resumes from the position it had reached, with a new buffer if the sample rate
changed. `Player::switch_audio` is the same path, and the test drives it
directly: a switch to a 44.1 kHz output mid-playback keeps the clock within a
few hundred milliseconds of real time. Following the default device when the
user _changes_ it, rather than when it fails, is #31.

## What was considered and rejected

- **The renderer's `AudioContext` as the clock.** WebAudio follows the default
  device on its own and has a good clock. But the samples would have to cross
  the IPC boundary as well as the frames, and the renderer would own the
  timing — the opposite of `CLAUDE.md` section 2, which keeps every media
  decision in the engine. The level meter and the filter-chain insertion point
  of #31 also belong beside the audio in Rust.
- **`rodio`.** A higher-level layer over `cpal` with its own decoders and mixer.
  Blinkify decodes with FFmpeg and mixes in the feeder, so `rodio` would be a
  second, unused audio engine in the binary.
- **WASAPI directly, through the `windows` crate.** No abstraction to learn,
  but COM calls are `unsafe`, which the workspace forbids in our own code.
  `cpal` isolates that `unsafe` in a maintained crate.
- **A video timer as the clock.** Timers drift from the sound card's crystal;
  over ten minutes the difference is audible, and audio cannot be corrected by
  dropping samples without clicks.
