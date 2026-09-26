# Architecture

How Blinkify is put together and why the structure holds under change.

## What belongs here

- The layer model and the direction of dependency between renderer, shell and
  engine
- The IPC contract: its shape, how it is generated, and how it is versioned
- Data flow for a concrete operation, end to end — import, preview, export
- The edit-graph model and how it is evaluated for preview and for export
- The caching strategy and the cache key derivation
- Threading and cancellation: what runs where, and how work is stopped

## Documents

- [`keyframe-index.md`](./keyframe-index.md) — what the keyframe index stores,
  how open GOPs are told apart, and how it fills lazily and persists (#24)
- [`waveform-peaks.md`](./waveform-peaks.md) — min/max peaks per channel, the
  zoom pyramid, the cache format (#25)
- [`filmstrips-and-proxies.md`](./filmstrips-and-proxies.md) — sprite-sheet
  thumbnails, preview proxies, and the type that keeps proxies out of export
  (#26)
- [`preview-pipeline.md`](./preview-pipeline.md) — the decode process, the
  bounded frame ring, presentation by timestamp, rotation at draw time, and
  teardown (#27)
- [`playback.md`](./playback.md) — the audio master clock, anchors and
  generations, seamless clip boundaries, exact frame steps, the transport, and
  playback without an audio device (#28)
- [`seek-and-scrub.md`](./seek-and-scrub.md) — the shared two-stage seek,
  coalesced scrubbing with a bounded prefetch cache, the resolving indication,
  and previewing from a proxy without changing the frame shown (#29)
- [`audio-monitoring.md`](./audio-monitoring.md) — monitor volume kept apart
  from clip gain, the BS.1770 meter and clip latch, sound-only tracks with solo
  and mute, the filter-chain insertion point, and following the default device
  (#31)
- [`project-file.md`](./project-file.md) — the edit graph, integer time,
  source fingerprints and relinking, the project file and its migrations (#32)
- [`edit-graph-evaluation.md`](./edit-graph-evaluation.md) — the one
  evaluator the preview and the export share, how the single path is
  enforced, frame-to-microsecond rounding, and live graph changes (#30)
- [`edits-and-history.md`](./edits-and-history.md) — the one path by which
  the open graph changes, primitive invertible changes, gestures and the
  undo history (#37)
- [`timeline-rendering.md`](./timeline-rendering.md) — the canvas timeline:
  two layers and a dirty-layer scheduler, virtualisation, anchored zoom, crisp
  lines at every display scaling, thumbnails and waveforms (#33)
- [`clip-interaction.md`](./clip-interaction.md) — selecting, dragging,
  trimming, rippling and rolling clips; pixel snapping; frames in, exact
  ticks out (#34)
- [`project-lifecycle.md`](./project-lifecycle.md) — new, open, save, exact
  dirty state, autosave to a recovery file beside the project, recovery at
  launch, closing on unsaved work (#54)
- [`tracks.md`](./tracks.md) — the compositing order, mute, solo and lock as
  graph state, lock enforcement in the edit layer, detached and linked
  sound (#36)
- [`media-library.md`](./media-library.md) — import as source references,
  never copies; refusals with a reason; a drag between zones as data; library
  thumbnails from the filmstrip (#53)
- [`sequence-settings.md`](./sequence-settings.md) — the settings, matching
  the first clip, the one copy-eligibility predicate and who reads it (#57)
- [`export-planner.md`](./export-planner.md) — how the edit graph compiles
  into copied, smart-cut and re-encoded segments, every cause as data, one
  set of encoding parameters per stream (#39)
- [`export-executor.md`](./export-executor.md) — the plan carried out in one
  pass: readers, routers and one muxer over NUT pipes, exact packet selection
  and rebasing, what is preserved, failure and cancellation (#40); encoded
  sound beside copied pictures, the codec choice and clean joins (#43);
  constant speed by retiming copied packets (#42); the full re-encode of holds,
  reverses, gaps and clips in another shape, reverse in bounded memory (#55)
- [`encoder-capability-cache.md`](./encoder-capability-cache.md) — the
  encoder capability profile kept between launches, keyed on the sidecar's
  hash and every display driver's version, and never reported stale (#83)
- [`re-encode-profiles.md`](./re-encode-profiles.md) — matching a re-encode
  to the source it joins: the source's profile, the encoder that makes it or
  a named refusal, and the join checked on the SPS before writing (#44)
- [`export-overview.md`](./export-overview.md) — what an export will do,
  said before it runs: pictures and sound claimed apart, every reason with
  its time, the keyframe snap as an undoable edit, the size and whether it is
  an estimate, and the space for it (#50)
- [`export-report.md`](./export-report.md) — what an export did, measured
  on the file it wrote: each segment copied or re-encoded, why, what could
  have been done instead, and the text form without folders (#52)
- [`export-queue.md`](./export-queue.md) — exports in the background: a job
  as a project snapshot, one at a time, progress that never goes back,
  cancellation, and the offer to export again after a crash (#51)
- [`smart-cut.md`](./smart-cut.md) — frame-accurate cuts that re-encode only
  the windows their frames depend on, leading pictures, in-band parameter
  sets across the joins, and the decline where no encoder matches (#41)

- [`audio-chain.md`](./audio-chain.md) — one FFmpeg chain for a clip's sound
  in the preview and the export, in a fixed order; gain with a two-stage
  true-peak limiter; loudness and true-peak measurement, cached by content;
  the limiter's effect worked out before export (#46); RNNoise noise
  reduction with a bundled model and a blend, and chain changes taken over
  while playing without a gap (#47); two-pass loudness normalisation as one
  measured gain and a limiter, for a clip or the whole mix (#48)

## What does not belong here

- **A choice between alternatives** — that is an ADR in
  [`../decisions/`](../decisions/). Architecture documents describe what is;
  ADRs record why it was chosen over what it is not.
- **How to run the build** — that is [`../engineering/`](../engineering/).
- **What a screen looks like** — that is [`../design/`](../design/).

## Conventions

- One concern per file, named after the concern: `ipc-contract.md`,
  `edit-graph-evaluation.md`, `cache-keys.md`.
- Diagrams are ASCII or Mermaid in the Markdown itself. A diagram that lives in
  a binary file stops matching the code within a month.
- Every document states which Epic or issue it came from, so a reader can find
  the discussion.
