# The export executor

From [#40](https://github.com/ismetcahangirov/blinkify/issues/40), Epic
[#6](https://github.com/ismetcahangirov/blinkify/issues/6). Why it is built
this way:
[ADR-0010](../decisions/ADR-0010-export-routes-packets-over-nut-pipes.md).
Code: `crates/blinkify-engine/src/export/execute.rs` and `export/nut.rs`.

The executor carries out an `ExportPlan` (see
[`export-planner.md`](./export-planner.md)) in one pass to the output file.

## Processes and threads

```
 source ─▶ reader (ffmpeg -c copy -f nut) ─pipe─▶ video router ─┐
 source ─▶ reader (ffmpeg -c copy -f nut) ─pipe─▶ audio router ─┤
                                                                ▼
                                    interleaver (NUT writer, by pts)
                                                                │ pipe
                                                                ▼
             muxer (ffmpeg -f nut -i pipe:0 -c copy) ─▶ <target>.blinkify-partial
                                                                │ renamed when complete
                                                                ▼
                                                             <target>
```

- One **router** thread per output stream walks that stream's segments in
  order, starting one reader per segment. The first reader's stream header
  becomes the output stream's header.
- The **interleaver** writes the routers' packets into the muxer in
  presentation order across streams, through bounded queues, so memory is
  bounded however long the export is.
- Every process runs through the orchestrator at `Priority::Export`, which has
  slots of its own (#110). The processes of one export are coupled by pipes
  and must all run at once; admitted through the shared slots, one left
  queued stalls the rest for ever.

## Selecting a copied segment's packets

Timestamps are the reader's, in its NUT time base, offset by 100 s (ADR-0010).

| Packet                                                 | Taken? |
| ------------------------------------------------------ | ------ |
| before the in-point's keyframe, in decode order        | no     |
| the keyframe whose timestamp is the in-point           | yes    |
| after it, shown before the in-point (open-GOP leading) | no     |
| shown in `[in, out)`                                   | yes    |
| shown at or after the out-point                        | no     |
| a keyframe at or after the out-point                   | stops  |

A keyframe after the in-point with no keyframe at the in-point is a
`Mismatch`: the file disagrees with the plan, and the export fails rather than
copying from somewhere else. Audio packets are all sync points; a packet is
taken when it starts inside the range.

Taken packets are rebased: `output = segment start + (pts − in)`, converted to
the output stream's time base. Consecutive segments therefore join with no gap
and no overlap, and the output starts at zero.

## What is preserved

| Property            | How                                                                      |
| ------------------- | ------------------------------------------------------------------------ |
| Packet payloads     | copied byte for byte (asserted by payload hash)                          |
| Codec configuration | NUT stream header extradata, unchanged                                   |
| Colour metadata     | in the bitstream; read back by the muxer                                 |
| Rotation            | `-display_rotation` on the muxer input (NUT cannot carry it)             |
| Stream metadata     | NUT stream info packets (language, handler)                              |
| File metadata       | NUT global info (creation time), less the pipe's `encoder`               |
| Chapters            | moved onto the output timeline, as NUT chapter info packets              |
| Attachments         | not yet — [#107](https://github.com/ismetcahangirov/blinkify/issues/107) |

## Failure and cancellation

- Checked before any process starts: the target's container, that every
  copied codec fits it unchanged, that no segment is declined, that every
  source is present, and that the target does not exist unless replacing it
  was confirmed (`overwrite`).
- Any failure — a reader, the muxer, a full disk, a source that disappears —
  fails the export. The partial file is removed and the target is untouched.
- The caller's `CancelToken` stops the muxer and every router; a router drops
  its reader, which stops that process. Each reader has its own cancel
  switch, because a reader stopped on purpose once its segment is complete
  must not cancel the export.
- A segment the plan declines — no encoder here can make it, or it would
  render HDR — is refused before any output is written, never substituted. It is never substituted by a copy or by an encode the
  plan did not choose.

## Encoded sound (#43)

Code: `crates/blinkify-engine/src/export/audio.rs`.

An audio segment the plan re-encodes — adjusted, mixed, sped up or slowed
down, or silence in a gap — is made by an **encoder** process in place of a
reader. It writes NUT to the router like a reader does, so the pictures beside
it are still copied, and no process on this path decodes video: the one that
reads the video stream copies it (`-c copy`), and the encoder maps only sound.
`ExportOutcome.commands` lists every command, and the tests assert this on it.

### The codec

| The output's sound                    | Encoded to                                              |
| ------------------------------------- | ------------------------------------------------------- |
| some of it is copied                  | the copied sound's codec, sample rate and channels      |
| none of it is copied                  | `ExportRequest.audio`: AAC 256 kb/s by default          |
| none copied, lossless asked for       | FLAC (24-bit), or 24-bit PCM where the container allows |
| a codec no shipped encoder reproduces | refused before anything runs                            |
| a codec the container cannot hold     | refused before anything runs (`CodecNotInContainer`)    |

The encoding is reported in `ExportOutcome.audio` for the export report.

### The filter graph

Each source is trimmed to its range in the stream's own time
(`-copyts`, `atrim`), given its gain (`volume`) and speed (`atempo`, in the
same stages the preview uses), resampled to the encoding's rate and layout,
and — where several sounds play at once — mixed with `amix` at unity gain.
The result is padded and trimmed to exactly the segment's length in samples,
so sound and pictures end together.

Denoise and normalise are refused until Epic #7 gives the preview and the
export the same filter for them; the export never applies processing the
preview does not play.

### Joins

A lossy encoder primes: its first frame depends on sound before it, and its
last on sound after. The encoder is given two whole codec frames of the
source's real sound on either side of the segment (silence only where the
stream has none), and only the packets whose samples are the segment's own
are kept. A lossless codec has no priming and gets none, so a FLAC segment
decodes to exactly the processed samples.

## Constant speed (#42)

A clip at a constant speed the plan keeps as a copy (ADR-0009: its rescaled
frame rate is 1–240 fps) is copied like any other: the same packets, the same
bytes. Only the timestamps change. Each packet's distance from the in-point is
divided by the speed on its own timestamp —
`output = segment start + (pts − in) ÷ speed` — so a variable-frame-rate
source keeps its own rhythm, only faster or slower, and is never conformed to a
nominal rate.

| Speed                           | Pictures                                   | Sound                |
| ------------------------------- | ------------------------------------------ | -------------------- |
| rescaled rate 1–240 fps         | copied, retimed                            | `atempo`, re-encoded |
| rescaled rate below 12 fps      | copied, retimed; the plan says it stutters | `atempo`, re-encoded |
| rescaled rate outside 1–240 fps | re-encoded at the sequence rate (#55)      | `atempo`, re-encoded |
| outside 0.1×–100×               | refused by the edit layer                  | —                    |

The sound is always re-encoded at a speed other than normal: samples cannot be
retimed without resampling. `atempo` keeps the pitch, exactly as the preview
plays it. Sound and pictures are both made to the segment's exact length, so
they stay in sync.

The source's nominal frame-rate metadata is not carried into the output: once
speeds and joins retime a stream it would misstate it, so the muxer derives the
rate from the timestamps, down to the last frame's duration.

## Full re-encode (#55)

Code: `crates/blinkify-engine/src/export/render.rs`.

A video segment the plan re-encodes whole — a hold, a reverse, a clip at a
speed no file carries, a clip not in the sequence's shape, sources whose
parameters cannot share a copied stream, a gap — is **rendered** at the
sequence's size, pixel aspect and frame rate, and encoded to join the rest of
the output. This path stays rare, visible and slow by admission: nothing
reaches it without the plan's recorded reason.

| Segment                 | What the renderer does                                                   |
| ----------------------- | ------------------------------------------------------------------------ |
| held frame              | decodes the one frame at the in-point and repeats it for the held length |
| reversed clip           | decodes a chunk at a time, last chunk first, each reversed (below)       |
| speed no file carries   | retimes the pictures by the speed, then puts them on the sequence's grid |
| clip in another shape   | scales to fit the sequence, pads the rest black, conforms the frame rate |
| incompatible parameters | encodes the source's pictures to the output's parameters                 |
| gap                     | black at the sequence's shape                                            |

Every rendered segment is exactly its planned number of frames: short input is
padded by repeating its last frame, long input is cut, and frames are stamped
on the sequence's grid from the segment's start.

### The encoder

The encoder is the plan's choice for the output's **reference** source — the
copied material's, or the first source where nothing is copied — so a rendered
segment is the same kind of stream as the copied ones around it (#44). Its
join is checked on the SPS before a packet of it is written, as a seam's is,
and its parameter sets travel in-band exactly as a seam's do. Where no encoder
on this machine qualifies, the plan declines the segment and the export
refuses before anything runs. Where nothing in the stream is copied, the
output is the encoder's stream.

### Reverse, in bounded memory

Decoding a clip to reverse it would hold all of it. Instead the clip is split
into chunks of `REVERSE_CHUNK_FRAMES` (32) frames. One decoder at a time, from
the last chunk to the first, decodes its chunk from the keyframe before it,
reverses it with `reverse` — which holds that chunk only — and writes raw
frames to a pipe; one encoder reads them in order. The most held at once is a
chunk: about 400 MB at 4K, however long the clip.

### Sound of a held or reversed clip

A held picture's moment does not move, so its sound is silence. A reversed
clip's sound is reversed with `areverse`, which holds the stretch: clips up to
ten minutes (about 230 MB of stereo float) are reversed, and longer ones are
refused with the reason rather than silenced.

### Progress and cancellation

The muxer's progress reports the output's position, so a mixed copy-and-render
export shows real progress. Cancelling stops every process of the export —
renderers and reverse decoders included — and leaves no partial file.
