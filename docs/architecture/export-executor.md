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
- A segment the executor cannot run yet — a smart-cut, a video re-encode, a
  copied clip at another speed — is an `Unsupported` error before any output
  is written. It is never substituted by a copy or by an encode the
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
