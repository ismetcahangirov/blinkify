# Blinkify

A Windows desktop video editor that edits video **without degrading it**.

Mainstream editors re-encode the whole timeline on every export. Trim four
seconds off a 4K clip and you get back a file that is measurably worse than the
one you started with, having changed nothing about the pixels you kept.
Blinkify exists because that loss is avoidable.

Where your edit does not require the pixels to change, they reach the output
file bit-identical to the source. Where it does, Blinkify tells you which
segments and why — before the export runs, not after.

> **Status: in development.** Epic 1 of 8. Not yet installable.

## How it works

Every export segment resolves to one of three tiers, and the tier is always
recorded:

1. **Stream copy** — the segment is keyframe-bounded and unfiltered. Packets are
   copied byte-for-byte. Zero generation loss.
2. **Smart-cut** — the cut is not keyframe-aligned. Only the partial GOP at the
   seam is re-encoded; the interior is copied.
3. **Full re-encode** — a filter forces the pixels to change. You are told which
   segments these are, and why, before you commit to the export.

The planner always picks the lowest tier that satisfies your edit.

## Documentation

|                                                                              |                                                                    |
| ---------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| [`CLAUDE.md`](./CLAUDE.md)                                                   | The engineering rulebook. Binding. Start here before contributing. |
| [`docs/`](./docs/)                                                           | Architecture, decisions, design, engineering, product, planning.   |
| [`docs/decisions/`](./docs/decisions/)                                       | Why the load-bearing choices are what they are.                    |
| [`docs/project-management/roadmap.md`](./docs/project-management/roadmap.md) | The eight Epics and their order.                                   |
| [`THIRD_PARTY.md`](./THIRD_PARTY.md)                                         | What ships with Blinkify, and under which licence.                 |

## Stack

Tauri 2 shell, React 19 renderer, Rust media engine, bundled LGPL FFmpeg
sidecar. Windows first, deliberately — see
[ADR-0001](./docs/decisions/ADR-0001-tauri-over-electron.md) and
[ADR-0002](./docs/decisions/ADR-0002-lgpl-ffmpeg-sidecar.md).
