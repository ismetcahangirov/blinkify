# ADR-0013 — Loudness normalisation is one measured gain and a limiter

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#48](https://github.com/ismetcahangirov/blinkify/issues/48),
  Epic [#7](https://github.com/ismetcahangirov/blinkify/issues/7)

## Context

The issue asks for two-pass EBU R128 normalisation with FFmpeg's `loudnorm`:
measure, then apply the measured values. It is right that single-pass
`loudnorm` is the wrong tool — it is a dynamic normaliser that rides the level
and pumps on speech, and a test here shows it (below). But `loudnorm`'s second
pass has three properties that do not fit Blinkify:

- **It still goes dynamic.** `loudnorm` runs its "linear" mode only when the
  measured true peak, raised by the gain, stays under its ceiling. When it
  would not — which is most speech brought up to a streaming target — it
  switches to the dynamic mode silently: the pumping the issue forbids,
  chosen by the filter, not by the user.
- **It carries history.** The dynamic mode works over a three-second window.
  The export encodes sound segment by segment and the preview starts decoding
  wherever the playhead is; a filter whose output at one moment depends on
  seconds before it gives a different result in each.
- **It measures only what one FFmpeg graph can see.** Normalising a whole
  sequence means measuring the _mix_ of every track, which no single decode of
  one file contains.

## Decision

**Normalisation is two passes. The first measures the integrated loudness
with the engine's own BS.1770-4 meter (`audio::loudness`), at the point of the
chain normalisation runs. The second is one gain — the target less the
measurement — followed by the true-peak limiter of ADR-0011 at the
normalisation's ceiling. Where that limiter takes loudness off, the gain is
measured again and raised by the shortfall, until the output is within 0.1 LU
of the target. The sequence is normalised by measuring its mix and giving every
clip the same gain.**

- **One gain.** The level never moves with the sound. The only dynamic
  element is the limiter catching peaks — the same one gain already has —
  which acts over milliseconds, not phrases.
- **Converging, not guessing.** For programme material the limiter engages
  often: speech at −25 LUFS brought to −14 needs 11 dB, and its peaks do not
  have 11 dB of headroom. Measured, a single pass then lands 1.6 LU short. The
  gain is measured again through the whole chain, the shortfall added, and
  repeated — at most four times — while the limiter keeps working. It stays
  one gain. Every pass is cached.
- **Measured by the engine.** The meter is checked against FFmpeg's `ebur128`
  (within 0.1 LU integrated), and the same meter measures a clip and a mix of
  any number of tracks: every audio segment of the export plan is rendered as
  the export would make it, one after another, into one meter.
- **The sequence keeps its levels.** Its gain goes on every clip alike, after
  each clip's own chain. What the limiter takes from a louder clip can change
  the levels between clips a little when a target leaves the peaks no room;
  otherwise they are kept to within 0.2 LU (tested).
- **Cached against what came before.** A clip's measurement is keyed by its
  content, its range and the filters before the normalisation, so changing its
  gain or noise reduction measures it again (tested); the sequence's gain is
  remembered against a digest of the resolved clips it was measured on.
- **Not measured, not guessed.** The evaluator leaves a normalisation
  unresolved; the export resolves it by measuring, and refuses one it cannot
  (`ChainError::NotMeasured`). The preview resolves only from the cache and
  plays an unmeasured normalisation unprocessed while the diagnostics say so,
  until the inspector's report has measured it.
- **Near silence is left alone.** Sound that nothing of passes R128's
  −70 LUFS gate has no loudness to move; normalisation leaves it where it is
  rather than raise room tone by 60 dB.
- **Loudness range is measured, not targeted.** The inspector shows the range
  before and after. A range _target_ is what makes `loudnorm` compress; with
  one gain the range is kept, which is the point.

## Alternatives considered

### `loudnorm` two-pass (`measured_I`, `linear=true`) — rejected

For the three reasons in the Context: it goes dynamic whenever the peaks
would pass its ceiling, its output depends on seconds of history, and it
cannot measure a mix. It is also 192 kHz out, bringing back the resampling
question of ADR-0011.

### Single-pass `loudnorm` — rejected

The issue rejects it too. The test `one_gain_does_not_pump_where_single_pass_loudnorm_does`
plays a tone that steps between −30 and −10 dBFS every second: the gain this
decision applies varies by less than 0.3 dB across the steps, single-pass
`loudnorm`'s by more than 3 dB.

### One gain, no convergence — rejected

Exact when the limiter is idle, 1.6 LU short on speech brought to −14 LUFS.
The issue's acceptance criterion is 0.5 LU.

### Normalise each clip of a sequence on its own — rejected

The issue names it: it destroys the levels the user set between clips.

## Consequences

### What this makes easy

- One mechanism for a clip and for a sequence, and the same numbers in the
  preview (once measured) and the export.
- Every measurement is reproducible and cached by content.

### What this makes hard

- A normalisation costs a full decode of its clip the first time, and more
  passes when the limiter engages; a sequence costs a render of its whole mix.
  The inspector measures a clip when it is shown and the sequence when asked.
- The export must resolve before it plans; the export job queue (#51) runs it
  as the export's first step.

### What we accept

- Where a target leaves the peaks no room, the limiter works hard: that is the
  price of the target, and the inspector shows the peak before and after.
