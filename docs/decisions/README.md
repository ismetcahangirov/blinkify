# Architecture Decision Records

An ADR records a decision that is expensive to reverse, the alternatives that
were rejected, and the reasoning that separated them.

## The rules

1. **ADRs are immutable once accepted.** To change a decision, write a new ADR
   that supersedes the old one, and edit the old one only to add
   `Superseded by ADR-NNNN` to its status line. Never edit an accepted
   decision's substance.
2. **Every ADR lists the rejected alternatives and why they lost.** An ADR that
   records only the winner is half an ADR — the next person re-derives the
   losing options from scratch.
3. **Number sequentially, never reuse a number**, even for an abandoned draft.
4. **Status is one of** `Proposed`, `Accepted`, `Superseded by ADR-NNNN`, or
   `Rejected`. A `Rejected` ADR is kept, not deleted; it is the record that the
   idea was considered.
5. **Date the decision**, not the file's last edit.

## When to write one

Write an ADR when the decision:

- Is expensive or embarrassing to reverse later
- Constrains what other code may do
- Has a plausible alternative a reasonable person would pick
- Touches the licence, the lossless constraint, or the IPC contract

Do not write one for a naming choice, a file location, or anything
[`../../CLAUDE.md`](../../CLAUDE.md) already decides.

## Template

```markdown
# ADR-NNNN — <short title in the imperative>

- **Status**: Proposed | Accepted | Superseded by ADR-NNNN | Rejected
- **Date**: YYYY-MM-DD
- **Context issue**: #N

## Context

What forces are in play? What makes this hard?

## Decision

What we are doing. One paragraph, stated plainly.

## Alternatives considered

### <Alternative> — rejected

Why it was plausible. Why it lost.

## Consequences

### What this makes easy

### What this makes hard

### What we accept
```

## Index

| ADR                                                       | Title                                                               | Status   |
| --------------------------------------------------------- | ------------------------------------------------------------------- | -------- |
| [0001](./ADR-0001-tauri-over-electron.md)                 | Tauri 2 over Electron for the application shell                     | Accepted |
| [0002](./ADR-0002-lgpl-ffmpeg-sidecar.md)                 | Bundle an LGPL FFmpeg build as a sidecar process                    | Accepted |
| [0003](./ADR-0003-re-encode-encoder-strategy.md)          | Probe encoders at runtime and decline rather than degrade           | Accepted |
| [0004](./ADR-0004-build-the-ffmpeg-sidecar-ourselves.md)  | Build the FFmpeg sidecar from a committed configure line            | Accepted |
| [0005](./ADR-0005-preview-frames-over-a-custom-scheme.md) | Preview frames cross into the renderer over a custom URI scheme     | Accepted |
| [0006](./ADR-0006-project-file-format.md)                 | The project file is versioned, deterministic JSON over integer time | Accepted |
