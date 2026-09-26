# ADR-0012 — Noise reduction is a bundled RNNoise speech model, blended in the graph

- **Status**: Accepted
- **Date**: 2026-09-26
- **Context issue**: [#47](https://github.com/ismetcahangirov/blinkify/issues/47),
  Epic [#7](https://github.com/ismetcahangirov/blinkify/issues/7)

## Context

Background noise removal is one of the operations Blinkify was conceived
around. The Epic names RNNoise through FFmpeg's `arnndn`, and leaves open four
questions whose answers are expensive to change later:

- **Which model.** `arnndn` does nothing without a model file, and the
  RNNoise family has several, each trained on a different pair of signal and
  noise.
- **Where it comes from.** The file must be there with no network
  (`CLAUDE.md` forbidden behaviour 8) and must not be whatever happens to be
  on the machine.
- **What "strength" means.** Full-strength RNNoise on clean sound produces the
  "underwater" artefact, so an on/off switch is a trap.
- **Timing.** Measured on the sidecar with an impulse, `arnndn` emits its
  output 480 samples (one 10 ms RNNoise frame at 48 kHz) late. Unhandled,
  every denoised clip's sound would sit 10 ms behind its pictures.

## Decision

**Blinkify ships one model — `somnolent-hogwash` (speech in a recording
environment) from rnnoise-models — as an installed resource checked by
SHA-256, runs it through `arnndn` with `mix` as the strength, and takes the
filter's one-frame delay back inside the chain.**

- **The model.** rnnoise-models maps signal against noise; `somnolent-hogwash`
  is the _speech_ against _recording_ noise cell: fans, air conditioning,
  computers — the target user's room. Its training speech is listed in its
  `info.txt`; the test recording is not among it.
- **Bundled and checked.** The file is committed (298 KB of text weights, not
  media), installed by the bundler's `resources` map, and verified against its
  SHA-256 at start-up (`Models::in_dir`). A missing or altered file is an
  error that names the path; an export that needs it is refused before
  anything is written (`ExportError::AudioChain`), and the preview plays the
  step unprocessed while the diagnostics say so.
- **Strength is `mix`.** `arnndn`'s own blend of the denoised signal with the
  original, 0 to 1, in the same filter graph the preview plays and the export
  writes — never two renders mixed afterwards. A new noise reduction starts
  at 70 %.
- **The delay is taken back.** The chain pads the end by one frame, runs
  `arnndn` at 48 kHz, drops the first frame and returns to the chain's rate.
  At strength 0 the output is the input, bit for bit — asserted on an export.
- **First in the chain** (ADR-0011): before gain raises the noise and before
  a limiter flattens the peaks the network tells speech by.
- **A/B is a bypass that does not stop playback.** Bypassing the step changes
  only a clip's audio chain; the player hands the new plan to the running
  feeder, which starts a decoder with the new chain 300 ms ahead and switches
  on the exact sample. The same path serves every audio control (#49).

## Alternatives considered

### FFmpeg's `afftdn` (spectral subtraction) — rejected

Built in, no model to ship, and tunable. On speech it leaves the "musical
noise" artefact that RNNoise was designed to avoid, and the Epic names RNNoise
for that reason.

### The model built into `arnndn`'s defaults, or "orig" — rejected

`arnndn` has no built-in model; "orig" is RNNoise's original training, for
_general_ noise. Recording noise is what the target user has.

### Several models, chosen by the user — rejected for now

Five models ship in rnnoise-models. Offering a choice asks the user a question
about training data. One well-chosen model and a strength control cover the
case; another model is a later, additive change.

### Download the model on first use — rejected

A network call other than the update check is forbidden, and a feature that
works only after a download fails offline, which is exactly when a user
editing a recording on a laptop finds out.

### An on/off switch — rejected

The Epic says why: full strength on clean speech is worse than none. A blend
is what makes the feature usable.

### Restart playback on every change, as other edits do — rejected

A restart leaves a gap while the new decoder starts. An A/B comparison is only
worth anything if the two can be heard back to back.

## Consequences

### What this makes easy

- The model is part of the installation, versioned with the code.
- Every audio control gets gapless live changes through the same retune path.

### What this makes hard

- Adding a second model means a second resource, hash and selection rule.
- The preview's change arrives about half a second after the edit: the new
  decoder starts 300 ms ahead of what the feeder is writing, which is itself
  ahead of the speaker.

### What we accept

- On music, RNNoise removes what it takes for noise. The inspector says so;
  Blinkify does not detect music.
