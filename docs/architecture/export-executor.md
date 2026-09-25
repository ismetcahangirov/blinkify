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
- Every process runs through the orchestrator at foreground priority.

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
- A segment the executor cannot run yet is an `Unsupported` error before any
  output is written. It is never substituted by a copy or by an encode the
  plan did not choose.
