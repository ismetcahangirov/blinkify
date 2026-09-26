# Edit-graph evaluation: one evaluator, two consumers

From [#30](https://github.com/ismetcahangirov/blinkify/issues/30) (Epic
[#4](https://github.com/ismetcahangirov/blinkify/issues/4)). The graph it
evaluates is [`project-file.md`](./project-file.md).

A preview that disagrees with the export is the most expensive way an editor
can fail: the user finds out after the render. Two pieces of code that each
interpret the graph will disagree eventually, so there is one.

```
Project (graph) ──► project::evaluate::evaluate ──► Timeline
                                                     │
                     ┌───────────────────────────────┴──────────────────┐
                     ▼                                                  ▼
     PlaybackPlan::from_timeline                          export::ExportContent
     (sources: SourceMedia, may carry a proxy)            (sources: ExportSource, originals only)
                     │                                                  │
                  Player                                    planner (#39) → FFmpeg
```

## The evaluator

`blinkify_engine::project::evaluate` is pure: graph in, `Timeline` out, no
file, no clock, no media. Per clip it resolves:

| What             | Rule                                                              |
| ---------------- | ----------------------------------------------------------------- |
| Source range     | The last `trim`. Trims are absolute, so a later one replaces.     |
| Speed            | The product of every `speed`, reduced (`6/2` → `3/1`).            |
| Place and length | `start`; length in sequence frames, rounded **up**.               |
| Audio chain      | `gain`, `denoise`, `normalise`, in the order given.               |
| Errors           | Inconsistent graph, untrimmed clip, overlap on a track, overflow. |

`Timeline::at(position)` is what applies at one sequence frame, including
the source tick on screen (rounded **down**). It is the export's answer and
the diagnostic view's content.

## Enforcing the single path

By convention it would last until the first deadline, so it is structural:

- **The compiler.** `Clip`'s `operations` field is private to
  `blinkify_engine::project`. No other module can read it; a `compile_fail`
  doc test proves it.
- **`pnpm evaluator:check`** covers what the compiler cannot. No Rust file
  outside `project/` may name an `Operation::` variant, and no renderer or
  design-system source may read an `.operations` property. The renderer asks
  the engine (`operations_at`) instead. `pnpm evaluator:check:test` proves
  both rules fire. See
  [`architecture-gates.md`](../engineering/architecture-gates.md).

## Content and quality

The distinction is in the types. **Content** is the `Timeline`, and
preview and export share it. **Quality** is the sources. The preview gets
`SourceMedia`, which may carry a proxy (#26) and decodes at the size of the
surface. The export gets `ExportSource`, which cannot hold a proxy. The
evaluator never sees a source, so a proxy cannot change its result. The
agreement test builds the plan with and without a proxy and checks that the
segments are identical.

## Preview: from frames to microseconds

The player's clock is in microseconds; the timeline is in frames of the
sequence rate, which are usually not whole microseconds (1/30 s). The
conversion is where a preview drifts a frame from the export, so the
rounding is chosen:

- a segment's start rounds **down**;
- a seek to frame `p` rounds **up**.

So the preview's offset into a clip is never less than the exact one, and the
source tick it shows is never before the evaluator's. The opposite choice
shows the previous frame whenever a cut sits on a frame boundary — the common
case. The agreement test catches that: with the start rounded up it fails
with `preview tick 395999, export tick 396000`. Two seams rounded this way
can overlap by up to 2 µs (`SEAM_TOLERANCE`), and the later segment wins.

The first video track supplies the pictures and their own sound. Each audio
track is a sound-only track with the project's track id; track 0 is reserved
for the main track. Further video tracks are not previewed, because Blinkify
does not composite. A clip whose source is offline is a gap.

## Held and reversed clips

From [#113](https://github.com/ismetcahangirov/blinkify/issues/113). A held
or reversed clip is rendered whole by the export (#55): one frame per
sequence frame, the held frame repeated or the clip backwards. The preview
shows the same thing by asking the same question. Its segment keeps the
evaluator's `Placement`, and at timeline position `t` it takes the sequence
frame there and asks `Placement::source_at` for the source tick — the
evaluator's answer, not a second derivation of the motion. The agreement test
generates held and reversed clips too.

- **A hold** is its in-point frame, decoded once and shown again at every
  sequence frame of its length, so the timecode moves on while the picture
  does not. Its sound is silence: the plan leaves it out of the sound tracks,
  as the export writes silence for it.
- **A reverse** decodes backwards a chunk at a time, prefetching the next
  chunk while this one plays, in bounded memory; its sound is decoded
  backwards in chunks too. How and why is
  [ADR-0017](../decisions/ADR-0017-reversed-preview-decodes-backwards-a-chunk-at-a-time.md);
  the ring's side of it is in [`playback.md`](./playback.md).
- **Stepping** through either moves by sequence frames, since the export
  writes one frame per sequence frame there.

The test exports the same graph and compares, frame by frame, the preview's
picture with the source frame the evaluator names (bit for bit) and with the
export's frame at the same sequence frame (it is the best match among all of
them). On a source whose timestamps are rounded to milliseconds the
evaluator's tick can fall a rounding error before a frame, forwards and
backwards; that is
[#134](https://github.com/ismetcahangirov/blinkify/issues/134), and the test
uses MP4, whose time base holds every frame exactly.

A clip's speed changes its segment's time base: frames map through
`source tb × speed`, and the audio decoder's `atempo` is the clip's speed
times the transport's. The gain is exact arithmetic in the feeder, before
Epic #7's insert and the meter. Denoise (#47) and normalise (#48) are
evaluated and exported, but the preview does not render them yet. The
diagnostic view says so for each one, rather than letting the sound suggest
the export will skip them. Hold and reverse are previewed (#113) and are not
marked.

## A graph change

An edit (`edit_project`, see [`edits-and-history.md`](./edits-and-history.md))
evaluates the new graph, and refuses it with a reason if it cannot be
evaluated; the open graph is then unchanged. `Player::set_plan` then
swaps the plan in place:

- sources already opened (probe, keyframe index, proxy) are reused;
- a lane whose segment is unchanged (same source, range, speed and place)
  keeps its decoder under its new index;
- if what is under the playhead changed, it is shown again at the same
  position, as a seek would; otherwise playback carries on and only the sound
  restarts on the new plan;
- nothing is written. The test lists every file under the source's directory
  and its cache before and after a trim, a reorder, a speed change and a gain
  change, and the lists are identical.

Saving is #54. A graph change is a decision, not a file.

## The diagnostic view

**Operations** in the player header, shown for a project preview. It polls
`operations_at` and lists, for each track, the clip under the playhead and its
resolved operations, marking any the preview does not render yet. This is how
someone finds out why their export will not be lossless, so it names the
operations in plain words.

## Alternatives considered

- **Let the player read the graph.** This was the simplest option and was
  rejected, because it creates exactly the second interpretation the issue
  forbids.
- **Evaluate in the renderer and send segments to the engine.** Rejected. It
  moves a media decision into the renderer (`CLAUDE.md` section 2), and the
  export would still need its own evaluation.
- **A dependency-cruiser rule alone.** It cannot see Rust, and in TypeScript
  the graph arrives as data, not as an import. It would pass while proving
  nothing.
- **Restart the player on every change.** Simple and correct, but it probes
  and indexes again and drops every decoder. The issue asks for the opposite.
