# ADR-0011 — One audio chain, in a fixed order, with an oversampled true-peak limiter

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#46](https://github.com/ismetcahangirov/blinkify/issues/46),
  and the rest of Epic [#7](https://github.com/ismetcahangirov/blinkify/issues/7)
  (#47, #48, #49)

## Context

Epic #7 adds three things a clip's sound can go through: noise reduction,
gain, and loudness normalisation. Three facts shape how they are built:

- **The preview and the export are separate processes.** The preview decodes
  a clip's sound to `f32` for the speaker, from wherever the playhead is; the
  export encodes it segment by segment beside copied pictures (ADR-0010). Each
  builds its own FFmpeg filter graph. The Epic's acceptance criteria require
  the two to be _the same code path_, asserted by a test, because two chains
  written twice will drift and the user only finds out after exporting.
- **Gain clips.** A gain that pushes a peak past full scale destroys it, and
  the damage is silent. The Epic asks for a _true-peak_ limiter: the peaks
  between samples, which a lossy encoder or a phone's resampler will find,
  exceed the sample peaks, so a file that looks clean in a sample meter can
  still distort.
- **FFmpeg's limiter sees samples.** `alimiter` holds the _sample_ peak to its
  limit exactly. Measured on the sidecar with pink noise and a 5 kHz tone
  raised 12 dB, limiting at −1 dBFS left a true peak of −0.1 dBTP: 0.9 dB over.

## Decision

**Every audio step becomes FFmpeg filters in exactly one function,
`audio::chain::filters`, which both the preview decoder and the export encoder
call. The order is fixed — noise reduction, gain and its limiter,
normalisation — whatever order the steps were added in. The limiter runs
twice: at four times the rate, 0.5 dB under the ceiling; then at the sound's
own rate, at the ceiling.**

- **One function, two callers.** `AudioRequest::command` (preview) and
  `export::audio::source_chain` (export) insert the same string at the same
  point: after the source is trimmed and resampled to the rate it is heading
  to, before any speed change or reverse. A test builds both commands for the
  same clip and asserts both contain the string, whole.
- **The rate the sound is heading to.** The chain runs at the output
  device's rate in the preview and at the encoding's rate in the export, so
  the ceiling holds at the rate the file is written at.
- **Before speed.** Noise reduction is trained on speech at its natural pace;
  it runs before `atempo`. The price is that a clip at another speed has its
  peaks reshaped slightly by the time-stretch after the limiter; at normal
  speed — nearly every clip — the ceiling holds exactly.
- **The fixed order, and why.** Noise reduction first: gain before it raises
  the noise it is about to attack, and a limiter before it flattens the peaks
  it tells speech from noise by. Gain, then its limiter: the limiter exists to
  catch what the gain pushed over. Normalisation last, with its own ceiling as
  the final constraint, because raising the level can push peaks back over.
- **The two-stage limiter.** Upsampled four times (`soxr`, the resampler the
  sidecar already ships for 44.1↔48 kHz), the peaks between samples are
  samples, and `alimiter` holds them at `ceiling − 0.5 dB`. Limiting makes
  new high-frequency content, and bringing the sound back to its own rate
  reshapes the peaks by up to about that margin; a second `alimiter` at the
  sound's own rate then holds every sample at the ceiling. Both compensate
  their look-ahead (`latency=1`): the sound is neither delayed nor lengthened —
  measured with an impulse, it comes out on the same sample.
- **Measured, with the margin chosen from the measurements.** On the sidecar,
  at a −1 dBTP ceiling, measured by FFmpeg's `ebur128` true-peak meter:

  | Signal                    | Gain   | Oversampled at −1.0 only | This decision |
  | ------------------------- | ------ | ------------------------ | ------------- |
  | Pink noise + 5 kHz tone   | +12 dB | −0.9 dBTP                | −1.4 dBTP     |
  | Pink noise + 5 kHz tone   | +24 dB | −0.9 dBTP                | −1.3 dBTP     |
  | White noise + 11 kHz tone | +12 dB | −0.4 dBTP                | −1.1 dBTP     |
  | White noise + 11 kHz tone | +24 dB | 0.0 dBTP                 | −1.0 dBTP     |

  White noise with an 11 kHz tone, 24 dB over, is the worst material there is
  for this; programme material sits well inside it. The engine's tests assert
  the ceiling on exported FLAC at +18 and +40 dB, at the default and at a
  −6 dBTP ceiling.

- **The limiter says what it does.** How far it turns the loudest peak down is
  worked out, not listened for: the clip's true peak before the gain is
  measured once (content-keyed, cached), and the limiter takes off whatever
  the gain puts over the ceiling. The inspector states it before export.

## Alternatives considered

### Keep the preview's gain as arithmetic in Rust — rejected

The preview applied gain by multiplying samples in the feeder while the export
used FFmpeg's `volume`. The results agree for a plain gain, which is why it
was acceptable in Epic #4. A limiter, a denoiser and a normaliser are not
arithmetic anyone should write twice, and a Rust limiter in the preview would
be a different limiter from FFmpeg's in the export. Rejected in favour of one
FFmpeg chain in both.

### Put a Rust processing stage in the export — rejected

The export could decode to PCM, run a Rust chain, and pipe the result to the
encoder, so the preview and export share Rust code instead. It puts a process
hop and an in-engine sample path into an export that today only routes
packets (ADR-0010), for no gain in fidelity: FFmpeg already has the filters.

### `alimiter` at the sound's own rate — rejected

Holds the sample peak exactly and lets the true peak through: 0.9 dB over on
the first measurement above. It is the limiter that "looks clean in a sample
meter and distorts on a phone".

### `alimiter` oversampled, alone — rejected

Holds the true peak at the oversampled rate, and the return to the sound's own
rate puts up to 1 dB back on hard-limited material (the third column). Not a
ceiling.

### Oversampled, with a low-pass between two limiters — rejected

Filtering the oversampled signal at 20 kHz before a second limiter held the
ceiling on the white-noise case but not on pink noise, and a two-pole
low-pass at 20 kHz takes about 1 dB off at 14 kHz — an audible change to every
clip with a gain, limited or not.

### `loudnorm` as the limiter — rejected

`loudnorm` contains a true-peak limiter, but only as part of a loudness
normaliser; it cannot be used as a limiter alone, and it outputs at 192 kHz,
which brings back the same resampling question. Normalisation (#48) is its
own decision.

## Consequences

### What this makes easy

- Adding a step is one arm in `chain::filters`, and the preview and export get
  it together; the test that asserts both commands carry the chain covers it.
- A measurement at any point of the chain (`filters_before`) hears exactly
  what the preview and export will: the gain advice measures before the gain,
  normalisation (#48) after the limiter.

### What this makes hard

- A change to a filter's parameters changes the preview only after the
  decoder restarts: the chain is baked into the decoder's command. Live
  adjustment during playback (#49) has to swap decoders without a gap.
- Four extra filters per clip with a gain, two of them resamplers at four
  times the rate: work for the preview's decoder that a multiplication did
  not cost.

### What we accept

- At speeds other than 1×, the time-stretch after the limiter can reshape
  peaks by a fraction of a dB.
- Where clips on several tracks overlap, their sum can exceed any one clip's
  ceiling. A limiter on the mix is a mastering feature Blinkify does not have.
