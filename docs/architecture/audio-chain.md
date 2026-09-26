# The audio chain

From Epic [#7](https://github.com/ismetcahangirov/blinkify/issues/7): issue
[#46](https://github.com/ismetcahangirov/blinkify/issues/46) (gain and the
true-peak limiter), with #47 (noise reduction), #48 (normalisation) and #49
(the inspector) building on it. Why it is shaped this way is
[ADR-0011](../decisions/ADR-0011-one-audio-chain-with-an-oversampled-true-peak-limiter.md).

## What a clip's sound goes through

A clip's audio steps are operations in the edit graph (`Operation::Gain`,
`Denoise`, `Normalise`), each with its settings and a `bypassed` switch. The
evaluator passes them on unchanged as the placement's `audio`; one function,
`audio::chain::filters`, turns them into FFmpeg filters:

```
 source ─ atrim ─ aresample(rate) ─┬─ denoise ─ gain ─ limiter ─ normalise ─┬─ areverse? ─ atempo? ─ aformat
                                   └──────── chain::filters(steps, rate) ───┘
```

- **Fixed order**, whatever order the steps were added in: noise reduction,
  gain and its limiter, normalisation. A bypassed step is absent from the
  filters, exactly; a chain with nothing to apply is no filter at all.
- **The same string in the preview and the export.** The preview's decoder
  (`AudioRequest`, built by `playback::audio_request`) and the export's
  encoder (`export::audio::source_chain`) both insert `chain::filters` straight
  after resampling to their rate. `tests/audio_gain.rs` builds both commands
  for one clip and asserts both carry it whole.
- **The rate it runs at** is the one the sound is heading to: the output
  device's in the preview, the encoding's in the export.

## Gain and the true-peak limiter (#46)

`volume` sets the gain; a two-stage limiter follows it, always, while the
gain step is present and not bypassed:

1. up to four times the rate (`soxr`), `alimiter` at `ceiling − 0.5 dB`: the
   peaks between samples are samples here;
2. back to the rate, `alimiter` at the ceiling: no sample exceeds it.

Both compensate their look-ahead, so nothing is delayed or lengthened. The
ceiling defaults to −1 dBTP and can be set from −20 to 0 dBTP. A gain of 0 dB
is no step at all: the edit removes it, so the sound is copied again rather
than re-encoded for nothing.

## Measuring: `audio::loudness`

The engine measures loudness itself, in one pass, for any stretch of any
source at any point of the chain (`chain::filters_before`):

- **integrated loudness** (ITU-R BS.1770-4: 400 ms blocks, 75 % overlap,
  absolute gate −70 LUFS, relative gate −10 LU),
- **loudness range** (EBU Tech 3342: 3 s short-term values every 100 ms, gated
  at −70 LUFS and −20 LU, 10th to 95th percentile),
- **true peak** (BS.1770-4 Annex 2: four-phase, 48-tap windowed-sinc
  interpolation).

The K-weighting is the level meter's ([`audio-monitoring.md`](./audio-monitoring.md)).
A test compares all three with FFmpeg's `ebur128` on the same file: within
0.1 LU, 0.5 LU and 0.2 dB. A mono source is measured as one channel; a
surround source as its stereo downmix.

Measurements are cached on disk, keyed by the file's content and the whole
request but the path — the stretch, the rate and the filters before the point
measured — so moving a file keeps the answer and changing an earlier step
does not (`measure_cached`).

## What the limiter is doing, before export

`audio::gain::advise` works the limiter's effect out rather than listening for
it: with the clip's true peak measured before its gain, the limiter must take
off `peak + gain − ceiling` when that is positive. The inspector shows that
figure, the level before the gain, and a suggested gain that would bring the
clip to −16 LUFS with what that would cost — advice, applied only when the
user applies it. The Tauri command is `gain_advice(clip)`.

## Where it is tested

- `audio::chain` — order, bypass, the limiter's filters.
- `audio::loudness` — the meter against BS.1770-4's reference tones, the
  gates, the interpolator; the command.
- `project::edit` — `SetAudio` and `ResetAudio`, their refusals, undo, and the
  random edit sequences that must undo byte for byte.
- `tests/audio_gain.rs` — exported true peak at or under the ceiling at
  +18 dB, +40 dB and a −6 dBTP ceiling; a clipped input limited, not clipped
  again; silence; an already-limited input turned down; the pictures copied
  packet for packet; the preview and export chains the same; the advice; the
  cache.
