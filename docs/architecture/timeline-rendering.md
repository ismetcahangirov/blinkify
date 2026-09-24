# Timeline rendering

From [#33](https://github.com/ismetcahangirov/blinkify/issues/33). The code is
`apps/desktop/src/timeline/`.

## What is drawn from what

The timeline draws the **evaluated** timeline the engine sends in every
`ProjectView` (#30, #37): placements with their start, length, source range
and speed, per track, in timeline order. It never reads a clip's operations
(`pnpm evaluator:check`), so what the timeline shows is what the preview
plays and the export writes.

```
project store ──┐ view, selection         ┌─ content canvas: ruler, rows,
timeline store ─┤ zoom, scroll, playhead  │  clips, thumbnails, waveforms
preview store ──┘ frame on screen         └─ overlay canvas: playhead
        │                                          ▲
        ▼ subscribe (outside React)                │ drawn once per frame
   mark a layer dirty ──▶ RedrawScheduler ──rAF───┘
```

## Two layers, one frame

`mountTimeline` subscribes to the stores directly — no React render is
involved in drawing. A change marks a layer dirty; the `RedrawScheduler`
draws every dirty layer once, together, on the next animation frame, however
many changes arrived in between.

| Change                  | Dirties           |
| ----------------------- | ----------------- |
| a new graph (an edit)   | content           |
| the selection           | content           |
| zoom, scroll, resize    | content + overlay |
| the playhead            | overlay only      |
| a thumbnail or waveform | content           |

The playhead moves every frame during playback and is on its own canvas, so
playback never repaints a clip. `mountTimeline.test.ts` moves it 120 times and
asserts the content layer was drawn no further times.

## Virtualisation

Only what is in view is visited. Rows scrolled out vertically are skipped;
within a row, the first visible clip is found by binary search over the
placements (they arrive sorted) and the walk stops at the first clip past the
right edge. A frame costs the clips on screen, not the clips in the project.
Clip rectangles are clamped to just beyond the view, because an hour-long clip
at single-frame zoom is millions of pixels wide and canvas coordinates that
large lose precision.

## Zoom and scroll

`viewport.ts` is the arithmetic, pure and tested numerically:

- **Scale** is CSS pixels per sequence frame, from the whole project fitted
  into 92 % of the width, to 96 px per frame — one frame wide enough to see and
  grab.
- **Anchored zoom** keeps the frame under the anchor where it is: the pointer
  for Ctrl+wheel, the playhead for the buttons (the centre when the playhead is
  off screen). The only exception is the origin, which never goes before
  frame 0.
- **Scrolling** is by wheel (Shift+wheel or a horizontal wheel sideways) and
  by a hand-drawn scrollbar. A native scroll area would need a spacer as wide
  as the timeline, which at close zoom is past what a browser lays out.
- The track-header column is DOM, fixed horizontally, and follows the rows
  vertically.

## Crisp at every display scaling

The canvases are sized in device pixels (`clientWidth × devicePixelRatio`) and
drawn with the transform set to the ratio. A one-pixel line is drawn at
`crisp(x) = (floor(x·dpr) + 0.5) / dpr` — a device pixel's centre — and one
device pixel wide, so it is sharp at 100, 125, 150 and 200 %; rectangle edges
are snapped to device-pixel boundaries. The display scaling is watched, so
moving the window to another monitor redraws at the new ratio.

## Text

Measuring text and cutting a label to fit are the expensive part of a canvas
frame. `TextCache` caches the fitted label per text and width bucket and is
dropped only when the zoom changes.

## Thumbnails and waveforms

`TimelineMedia` answers the painter synchronously from what is loaded and
requests what is not; an arrival marks the content layer dirty.

- **Thumbnails** come from the filmstrip sprite sheets of #26. The timeline
  asks `generate_filmstrip` at the zoom's pixels per second, and the engine
  chooses the interval from its power-of-two ladder. Strips are kept per
  power-of-two bucket of the zoom, and a bucket still generating is drawn from
  the nearest one loaded. Sheets are served over the `frame` scheme at
  `/sheet/<path>`, and only files inside the engine's cache are served.
- **Waveforms** come from the peak pyramid of #25. The timeline asks
  `waveform_peaks` at the zoom's pixels per second in 512-column chunks, and
  the engine picks the pyramid level from the stream's real sample rate.

## Performance

`pnpm bench:timeline` runs on the heavy workflow, not on pull requests: it
draws the content and overlay layers of a 200-clip project while one clip is
dragged, 600 frames, through a painter that does no pixel work, and fails if
the 95th percentile exceeds 6 ms. On the development machine (AORUS 15G) it
measured a mean of 0.08 ms and a p95 of 0.13 ms of script per frame. That is
the renderer's share of a 16.7 ms frame; rasterisation is the GPU's and is not
measured on a hosted runner, which has none.
