# ADR-0001 — Use Tauri 2 for the application shell, not Electron

- **Status**: Accepted
- **Date**: 2026-09-21
- **Context issue**: [#1](https://github.com/ismetcahangirov/blinkify/issues/1),
  [#9](https://github.com/ismetcahangirov/blinkify/issues/9)

## Context

Blinkify is a Windows desktop video editor with a web-technology interface and a
media engine that is necessarily native. The shell has to supply a window, a
renderer runtime, file system access, a process boundary for the FFmpeg sidecar,
an installer and an update channel.

Three forces shape the choice:

1. **The engine must be native regardless.** Probing, keyframe indexing,
   planning and export are CPU-bound and cannot run on a UI thread — see
   `CLAUDE.md` section 12. Whatever shell we pick, the hot path is compiled
   code, not JavaScript.
2. **Distribution size is user-visible.** Blinkify competes against tools people
   install on a whim. A 150 MB download for an editor that does trims is a
   reason not to try it.
3. **Memory is contended.** Editing 4K footage means large decoded frame buffers
   and caches. Memory the shell takes is memory the engine cannot have.

## Decision

The shell is **Tauri 2**. The renderer runs in the system WebView2 control, the
shell process is Rust, and the media engine is a set of plain Rust crates in
`crates/` that the shell calls directly and that do not depend on Tauri.

The shell stays thin: window lifecycle, the command surface, the event channel,
file dialogs and the updater. All media decisions live in the engine.

## Alternatives considered

### Electron — rejected

The obvious choice, and the one with the deepest ecosystem — including the
editor prior art almost everyone has seen.

It lost on three counts:

- **Size.** Electron bundles Chromium and Node: roughly 150 MB before any
  application code. Tauri links against the WebView2 runtime that already ships
  with Windows 11, producing a binary in the ~10 MB range.
- **A runtime we would never use.** Electron's value is that heavy work runs in
  Node in the main process. Our heavy work is Rust, so we would ship, update and
  secure a Node runtime that does nothing on the hot path.
- **Memory floor.** Chromium plus Node is a substantial resident footprint
  before a single frame is decoded, in an application whose defining workload is
  large frame buffers.

Electron's real advantage — a consistent rendering engine across platforms —
buys little when the target is Windows first (see `CLAUDE.md` section 3) and
WebView2 is a known, evergreen Chromium.

### A native Windows UI (WinUI 3 / Win32 / Qt) — rejected

Fastest possible interface and no web layer at all.

Rejected because the timeline, the inspector and the media library are dense,
highly iterated UI. The owner iterates faster in React than in XAML or Qt, and
the interface is where this product is won or lost. Qt additionally brings a
licensing question we do not need alongside the one ADR-0002 already settles.

### A pure Rust GUI toolkit (egui, iced) — rejected

Single language, no IPC boundary, no web stack.

Rejected on ecosystem maturity for this specific interface. A virtualised canvas
timeline, accessible primitives and a design system are solved problems in the
web stack and unsolved-to-partly-solved in Rust GUI today. That is a lot of
foundational work bought for the benefit of avoiding one serialisation boundary.

### Tauri 1 — rejected

Superseded. Tauri 2 has the plugin and IPC model we want, a better updater
story, and is where upstream effort goes.

## Consequences

### What this makes easy

- A ~10 MB installer and a small memory floor, leaving headroom for frame
  buffers and caches.
- The engine is ordinary Rust. `cargo test -p blinkify-engine` runs with no
  window, no WebView2 and no renderer — which is why `CLAUDE.md` section 2
  forbids a `tauri::` import inside an engine crate.
- WebView2 ships with Windows 11, so there is no renderer runtime to install for
  the great majority of target machines.
- Rust end to end below the interface: one toolchain, one dependency policy, one
  `cargo deny` licence gate.

### What this makes hard

- **The IPC boundary is real.** Every call between renderer and engine
  serialises. This is why the command surface must be coarse — few chunky
  commands plus an event channel — rather than many fine-grained calls.
- **The types must be generated, not mirrored.** Two hand-maintained type
  definitions across a boundary drift. `packages/types` is generated from the
  Rust types for this reason.
- **A smaller ecosystem.** Fewer worked examples than Electron for
  desktop-specific problems, so more reading of upstream source (`CLAUDE.md`
  section 9).

### What we accept

- **WebView2 is not guaranteed on Windows 10.** It ships with Windows 11 but not
  with every Windows 10 installation. The installer must detect its absence and
  guide the user — tracked in
  [#14](https://github.com/ismetcahangirov/blinkify/issues/14).
- **The renderer engine is whatever WebView2 is on that machine.** Evergreen
  Chromium, but not a version we pin. Renderer code targets baseline WebView2
  behaviour and does not rely on the newest APIs.
- **macOS and Linux are deferred**, deliberately. WebView2's equivalents there
  are WKWebView and WebKitGTK, which are genuinely different engines. Revisit
  after v1.
