# The media test corpus

The engine's integration tests read real media files: an H.264 stream with
B-frames, an HEVC stream with open GOPs, a portrait phone video, a variable
frame rate recording, an MP4 with an edit list, HDR10. Each file exists to
carry one property the engine must get right.

## Getting it

```bash
pnpm corpus
```

Generates `corpus/` from the recipes in
[`tools/corpus/generate.mjs`](../../tools/corpus/generate.mjs). It is a no-op
when `corpus/manifest.json` records the current recipes and every file is
present; change a recipe and the corpus regenerates. `pnpm verify` runs it, and
CI caches it on the recipes.

**The corpus is never committed** — `CLAUDE.md` forbidden behaviour 12. Binary
blobs in git history are permanent. `/corpus/` is ignored by git.

## The corpus tool is not the sidecar

Real footage is overwhelmingly H.264 and HEVC with B-frames. Making such files
needs `libx264` and `libx265`, which are GPL — exactly what
[ADR-0002](../decisions/ADR-0002-lgpl-ffmpeg-sidecar.md) keeps out of Blinkify,
and exactly what the bundled sidecar is built without.

So the recipes run a **separate, pinned GPL FFmpeg**, the _corpus tool_:

- It is downloaded by `pnpm corpus` from the URL in
  [`tools/corpus/corpus-tool.lock.json`](../../tools/corpus/corpus-tool.lock.json)
  and verified against the SHA-256 recorded there.
- It lives in `target/corpus-tool/`, which is ignored by git and never bundled.
- Nothing in Blinkify runs it. Only `generate.mjs` does, to make test inputs.
  The code under test — the probe, the indexer, the orchestrator — only ever
  runs the bundled LGPL sidecar, and the sidecar gate proves that sidecar is
  what ships.

Using a GPL program to _produce data_ does not make the data, or the program
that later reads it, GPL — the same way compiling with GCC does not make the
output GPL. The line ADR-0002 draws is about what Blinkify links and
distributes, and the corpus tool is neither linked nor distributed.

## What each file is for

| File                       | The property it carries                                                 |
| -------------------------- | ----------------------------------------------------------------------- |
| `h264-high-closed-gop.mp4` | H.264 High, B-frames, keyframes at irregular times, AAC stereo          |
| `h264-open-gop.mp4`        | H.264 open GOPs: non-IDR recovery-point keyframes with leading B-frames |
| `hevc-open-gop.mp4`        | HEVC CRA keyframes followed by RASL leading pictures                    |
| `hevc-closed-gop-radl.mp4` | HEVC IDR_W_RADL keyframes: leading pictures, but a closed GOP           |
| `hevc-hdr10.mp4`           | HEVC Main 10, BT.2020 / PQ, mastering display and content light level   |
| `portrait-phone.mp4`       | A landscape-coded stream with a 90-degree display rotation              |
| `vfr-screen.mp4`           | Variable frame rate: 30 fps, then 10 fps; keyframes at 0, 1, 2.5, 3.2 s |
| `edit-list.mp4`            | Stream-copied from inside a GOP; the MP4 edit list hides the pre-roll   |
| `multi-audio.mkv`          | VP9 with Opus stereo and AAC 5.1 audio tracks, and two chapters         |
| `vp9.webm`                 | VP9 profile 0 and Opus                                                  |
| `av1.mp4`                  | AV1 and AAC                                                             |

Broken files — zero bytes, truncated, not media at all — are made by the tests
themselves, next to the case that needs them.

## Adding a file

Add a recipe to `RECIPES` with a `note` stating the one property it carries,
add a row above, and run `pnpm corpus`. Keep files short and small: the corpus
is regenerated on CI runners, and a recipe that takes a minute to encode is a
minute on every cache miss.

The full losslessness corpus — real phone captures, long files, the packet-hash
suite — is [#45](https://github.com/ismetcahangirov/blinkify/issues/45), and
builds on this one.
