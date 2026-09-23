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
