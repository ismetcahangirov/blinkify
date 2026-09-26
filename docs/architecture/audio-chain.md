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

## Noise reduction (#47)

`arnndn` with the bundled RNNoise speech model
([ADR-0012](../decisions/ADR-0012-noise-reduction-is-a-bundled-rnnoise-speech-model-blended-in-the-graph.md)):

```
aresample=48000, apad=pad_len=480, arnndn=m=<model>:mix=<strength>,
atrim=start_sample=480, asetpts=PTS-STARTPTS, aresample=<rate>
```

- **Strength** is `mix`: the denoised signal blended with the original, in
  the graph. At 0 the output is the input, bit for bit.
- **The frame of delay** RNNoise adds is taken back — padded at the end,
  dropped at the start — so the sound stays on its samples.
- **The model** is `Models`: found in the installation's `rnnoise/`
  resource directory and checked against its SHA-256 at start-up. The path
  reaches the graph escaped for both of FFmpeg's parsers, so a quote, a
  comma or a bracket in an install path is only a path. Without the model,
  `chain::filters` refuses noise reduction (`ChainError::NoModel`) and the
  export with it; the preview plays the chain without it
  (`chain::playable`), and the diagnostics say it is not heard.
- **Measured**: on the corpus reading under pink noise, full strength raises
  the scale-invariant signal-to-noise ratio from 8.9 to 12.3 dB.

## Loudness normalisation (#48)

Two passes ([ADR-0013](../decisions/ADR-0013-loudness-normalisation-is-one-measured-gain-and-a-limiter.md)):

1. **Measure** the clip's integrated loudness where normalisation runs —
   after noise reduction and gain — over its whole range, with
   `audio::loudness`.
2. **Apply one gain**, the target less the measurement, then the true-peak
   limiter at the normalisation's own ceiling:
   `volume=<gain>dB` and the two-stage limiter above.

Where the limiter takes loudness off, the gain is measured again through the
whole chain and raised by the shortfall until the output is within 0.1 LU
(`normalise::converge`); it is still one gain. Sound below R128's −70 LUFS gate
is left where it is.

- **Resolution.** The evaluator gives each `Normalise` step a `gain_db` of
  `None`; `audio::normalise::resolve_clips` fills it in. The export resolves
  by measuring (`export::loudness::Resolver`), and an unresolved step refuses
  the export (`ChainError::NotMeasured`). The preview resolves from the cache
  only (`only_cached`) and plays an unmeasured normalisation unprocessed.
- **The sequence.** `sequence.loudness` normalises the whole mix: every audio
  segment of the plan is rendered as the export makes it (`render_job`) into
  one meter, and `apply_sequence` gives every clip with sound the same last
  step. The levels between clips are kept.
- **Reports.** `loudness_report(clip)` and `sequence_loudness_report()`
  measure and return the loudness before and after, in LUFS, with the range
  and true peak; the inspector shows them.

## Changing the chain while playing

Changing a clip's audio chain changes nothing else in the preview plan, so
`Player::set_plan` recognises it (`only_chains_differ`) and hands the plan to
the running feeder instead of restarting playback. The feeder starts a
decoder with the new chain 300 ms ahead of what it is writing, keeps the old
one playing until the walk reaches that point, and switches on that sample —
dropping anything the new decoder made for a stretch already written if it
was late. The picture and the clock are untouched; there is no gap
(`tests/audio_denoise.rs` plays a tone, changes its gain mid-playback, and
finds no silent 10 ms).

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
- `audio::denoise` — the model's hash, the path escaped for both parsers,
  the blend.
- `audio::normalise` — the gain arithmetic, convergence, clip and sequence
  resolution.
- `tests/audio_normalise.rs` — four references to −14 and −23 LUFS within
  0.5 LU and under the ceiling, the sequence keeping its levels, no pumping
  where single-pass `loudnorm` pumps, a measurement taken again when the chain
  before it changes, near silence left alone, an unmeasured normalisation
  refused.
- `tests/audio_denoise.rs` — the SNR gain on a noisy reading, strength 0 bit
  for bit, the model found under an install path with a quote in it, a
  missing model refused before anything is written, the pictures copied, and
  a change of chain while playing with no gap.
