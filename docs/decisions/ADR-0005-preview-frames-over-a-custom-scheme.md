# ADR-0005 — Preview frames cross into the renderer as raw RGBA over a custom URI scheme

- **Status**: Accepted
- **Date**: 2026-09-23
- **Context issue**: [#27](https://github.com/ismetcahangirov/blinkify/issues/27)

## Context

The preview is built from decoded frames, not from an HTML `<video>` element
(Epic [#4](https://github.com/ismetcahangirov/blinkify/issues/4)). The frames
are decoded in Rust — by the FFmpeg sidecar, under the engine — and drawn in the
WebView2 renderer. They have to cross the process boundary between the two.

The volume is what makes this a decision rather than a detail. A 1080p RGBA
frame is 8.3 MB; at 30 fps that is 250 MB/s. Most preview frames are smaller,
because the engine decodes at the size of the player surface rather than the
source, but the path has to survive the full-size case.

Two further requirements come from the issue and from `CLAUDE.md`:

- **Backpressure is mandatory.** A decoder that outruns the renderer consumes
  memory until the application is killed.
- **The IPC surface is coarse.** A per-frame JSON command is "a performance bug
  waiting to be written" (`CLAUDE.md` section 2).

## Decision

The engine serves frames on a custom URI scheme, `frame`, registered with
Tauri's asynchronous URI-scheme protocol. The renderer `fetch`es
`http://frame.localhost/<session>/<after>` and receives the frame to show now as
the response body: a 32-byte little-endian header (magic, width, height,
sequence number, presentation timestamp) followed by the raw RGBA pixels, which
go straight into an `ImageData` and `putImageData` with no conversion.

The exchange is **pull-based with one request in flight**: the renderer asks
for "the frame after the last one I drew", the engine answers with the frame due
at the playback clock — waiting up to 50 ms for one to fall due, then answering
`204` — and the renderer does not ask again until it has drawn that frame. The engine
decides which frame is due and drops the ones that were superseded; the renderer
computes nothing about timing.

## Alternatives considered

### Base64 in a command result or an event payload — rejected

The obvious approach, and the one the issue names as too slow. Base64 inflates
8.3 MB to 11 MB, and the result then passes through JSON serialisation in Rust
and `JSON.parse` in the renderer before it is decoded back to bytes — three
full passes over every frame, on the renderer's main thread for the last two.
It is the reason many Electron-based editors have a sluggish preview.

### A raw-bytes command result (`tauri::ipc::Response`) per frame — rejected

Tauri 2 can return raw bytes from a command without base64, and Blinkify
already uses it for waveform peaks. For frames it is a per-frame `invoke`, which
is the pattern section 2 warns against, and it carries the command machinery —
argument deserialisation, the permission check, the invoke key — on every frame
for no benefit. The custom scheme is the same transport underneath without the
command layer.

### A Tauri `Channel` pushed from the engine — rejected

A channel lets the engine push frames as they are decoded, which looks natural.
But a push has no backpressure of its own: `Channel::send` returns once the
message is handed over, and a renderer that is slower than the decoder — a throttled window, a
busy main thread — lets the queue grow without bound in the shell. Adding
backpressure means an acknowledgement per frame from the renderer, which is a
per-frame command again. Pulling puts the bound in the shape of the exchange.

### Shared memory (`ICoreWebView2SharedBuffer`) — rejected for now

WebView2 can map a shared buffer into the renderer, which would avoid the
copy into the response body. It needs COM interop against the WebView2 API,
which the workspace's `unsafe_code = "forbid"` rules out in our own code, and
Tauri does not wrap it. The copy it would save is one `memcpy` of the frame —
for 1080p, on the order of a millisecond at ordinary memory bandwidth — against
a 33 ms frame budget. If a later measurement shows the copy is
the bottleneck, this is the alternative to revisit, in a crate that isolates the
`unsafe`, and with its own ADR.

### A localhost HTTP server — rejected

It would work the same way from the renderer's side, but it opens a listening
socket, which is a network surface section 11 does not allow and a firewall
prompt on first run.

## Consequences

### What this makes easy

- Backpressure is structural: one request in flight means nothing can queue for
  a slow renderer, in either process.
- Presentation timing lives in the engine, where the clock is. The renderer's
  loop is "fetch, draw, wait for paint, repeat".
- The same endpoint serves playback and, in #29, scrubbing — a seek is a new
  position on the session, and the next fetch returns the frame there.

### What this makes hard

- The frame body is one copy of the pixels per presented frame, in Rust. Cheap,
  but not free.
- The CSP must allow `connect-src` to the scheme (`frame:` and
  `http://frame.localhost`), and the response carries
  `Access-Control-Allow-Origin`, because the page's origin and the scheme's
  origin differ.

### What we accept

- A frame that is superseded before the renderer asks for it is never sent.
  That is the intended drop policy, and the engine counts every one of them.

## Later changes

Recorded here without changing the decision above.

- 2026-09-23, [#28](https://github.com/ismetcahangirov/blinkify/issues/28): the
  header grew to 48 bytes (magic `BKF2`) to carry the rotation, a "black"
  flag for gaps, the timeline position and the timeline frame number. The
  layout is documented in
  [`preview-pipeline.md`](../architecture/preview-pipeline.md).
