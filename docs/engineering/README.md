# Engineering

How to work on Blinkify day to day.

## What belongs here

- [`github-workflow.md`](./github-workflow.md) — branch naming, Conventional
  Commits, the Epic-to-sub-issue relationship
- The release process and the unsigned-binary gap
- Local environment setup: toolchain versions, WebView2, MSVC build tools
- Debugging guides — the Tauri IPC boundary, the FFmpeg sidecar, the timeline
  canvas
- Testing guides: how to write a losslessness assertion, how the corpus is
  produced
- Operational runbooks for CI

## What does not belong here

- **Binding rules** — those live in [`../../CLAUDE.md`](../../CLAUDE.md). This
  directory explains _how_; the rulebook states _what is required_. When the two
  disagree, the rulebook wins and this directory is wrong.
- **Why the stack is what it is** — that is [`../decisions/`](../decisions/).

## Conventions

- Every command in a document here is one someone has actually run on Windows.
  An untested command in a setup guide costs the next contributor an hour.
- Document the failure and its message, not just the happy path. People read
  these documents when something has already gone wrong.
