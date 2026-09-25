# Third-party components

Every third-party component that **ships to a user** inside the Blinkify
installer, with its licence and what obligation that licence places on us.

## How this file is maintained

- A dependency that ships to the user is added here **in the same pull request
  that adds it** — `CLAUDE.md` section 10, rule 4.
- Build-time and development dependencies are **not** listed here. They do not
  reach a user and their licences are gated by `cargo deny check licenses` and
  by review.
- **GPL and AGPL are prohibited** in both the renderer and the engine. See
  [ADR-0002](./docs/decisions/ADR-0002-lgpl-ffmpeg-sidecar.md). If a component
  under either licence appears in this file, something has gone wrong and the
  build is not shippable.
- A stale entry is a compliance failure, not a formatting one. Versions are
  updated when they change.

## Bundled binaries

| Component                              | Version       | Licence      | Obligation                                                                                         | Status                                                                 |
| -------------------------------------- | ------------- | ------------ | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| FFmpeg (LGPL build, no `--enable-gpl`) | FFmpeg n8.1.3 | LGPL v2.1+   | Ship the licence text; keep the component replaceable by the user; publish the build configuration | Bundled — [#21](https://github.com/ismetcahangirov/blinkify/issues/21) |
| RNNoise model weights                  | —             | BSD-3-Clause | Attribution                                                                                        | Planned — [#47](https://github.com/ismetcahangirov/blinkify/issues/47) |

### The FFmpeg sidecar in detail

`ffmpeg.exe` and `ffprobe.exe` are FFmpeg **n8.1.3** (commit `1041abdc962f`),
built by Blinkify from the configure line in
[`tools/ffmpeg-sidecar/configure.txt`](./tools/ffmpeg-sidecar/configure.txt) —
not a distributor build, for the reasons in
[ADR-0004](./docs/decisions/ADR-0004-build-the-ffmpeg-sidecar-ourselves.md).
The exact source commit, toolchain image digest and SHA-256 of each binary are
in [`tools/ffmpeg-sidecar/sidecar.lock.json`](./tools/ffmpeg-sidecar/sidecar.lock.json),
and `pnpm sidecar:check` fails CI if the bundled binaries differ from any of it.

The configuration, as the binary reports it (toolchain flags omitted):

```
--disable-debug --disable-doc --disable-ffplay --enable-pthreads
--disable-w32threads --disable-network --disable-autodetect --enable-d3d11va
--enable-dxva2 --enable-zlib --enable-lzma --enable-ffnvcodec
--enable-nvenc --enable-nvdec --enable-cuvid --enable-amf --enable-libvpl
--enable-libvpx --enable-libsvtav1 --enable-libaom --enable-libdav1d
--enable-libopus --enable-libmp3lame --enable-libvorbis --enable-libzimg
--enable-libsoxr
```

No `--enable-gpl`, no `--enable-nonfree`, no `--enable-version3`: the build is
LGPL v2.1-or-later. It is also built with `--disable-network`, so it cannot make
a network request (`CLAUDE.md` forbidden behaviour 8).

Libraries statically linked into those two binaries, and therefore shipped:

| Library            | Licence                                     | Used for                     |
| ------------------ | ------------------------------------------- | ---------------------------- |
| libvpx             | BSD-3-Clause                                | VP9 encode and decode        |
| SVT-AV1            | BSD-3-Clause-Clear + AOMedia Patent Licence | AV1 encode                   |
| libaom             | BSD-2-Clause + AOMedia Patent Licence       | AV1 encode                   |
| dav1d              | BSD-2-Clause                                | AV1 decode                   |
| Opus               | BSD-3-Clause                                | Opus encode                  |
| LAME               | LGPL v2+                                    | MP3 encode                   |
| libvorbis, libogg  | BSD-3-Clause                                | Vorbis encode                |
| zimg               | WTFPL                                       | `zscale` for proxies         |
| soxr               | LGPL v2.1+                                  | resampling                   |
| oneVPL dispatcher  | MIT                                         | Intel Quick Sync             |
| nv-codec-headers   | MIT                                         | NVIDIA NVENC and NVDEC       |
| AMF headers        | MIT                                         | AMD AMF                      |
| zlib               | Zlib                                        | compressed container headers |
| liblzma (XZ Utils) | 0BSD                                        | compressed container headers |

Every one is permissive or LGPL, and LGPL is satisfied the same way as for
FFmpeg itself: the whole sidecar is a separate, replaceable executable. Their
licence texts ship in the attribution bundle tracked by
[#73](https://github.com/ismetcahangirov/blinkify/issues/73).

**The FFmpeg entry is load-bearing.** It is invoked as a separate sidecar
process, never linked into the Blinkify process, which is how the LGPL
replaceability obligation is satisfied. The build configuration is committed and
a CI gate asserts `--enable-gpl` is absent. Do not change the linkage model
without superseding ADR-0002.

**Note on what is deliberately absent.** `openh264` is **not** bundled, and
neither is any other software H.264 or HEVC encoder. The reasoning — it cannot
match a High-profile seam, and Cisco's royalty undertaking covers only binaries
Cisco itself distributes — is in
[ADR-0003](./docs/decisions/ADR-0003-re-encode-encoder-strategy.md). Adding one
is not a dependency decision; it is an ADR-0003 supersession.

## Runtime components supplied by the operating system

Not bundled, not redistributed, listed because they are part of what runs.

| Component                                        | Supplied by                              | Note                                                                                                                                                 |
| ------------------------------------------------ | ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| WebView2 runtime                                 | Microsoft, with Windows                  | Ships with Windows 11; may be absent on Windows 10. The installer detects and guides — [#14](https://github.com/ismetcahangirov/blinkify/issues/14). |
| Hardware video encoders (NVENC, Quick Sync, AMF) | NVIDIA / Intel / AMD, via the GPU driver | The only H.264 and HEVC encoding path Blinkify uses. See ADR-0003.                                                                                   |

## Typefaces bundled in the interface

Committed to the repository rather than installed, so that "bundled with the
application, never fetched at runtime" is a property of the shipped bytes —
[#16](https://github.com/ismetcahangirov/blinkify/issues/16). The files, their
licence texts and their provenance live together in
[`packages/ui/src/fonts/`](./packages/ui/src/fonts/).

| Component                   | Version                            | Licence                   | Obligation                                                                                              |
| --------------------------- | ---------------------------------- | ------------------------- | ------------------------------------------------------------------------------------------------------- |
| Inter (variable, Latin)     | `@fontsource-variable/inter@5.3.0` | SIL Open Font License 1.1 | Ship the licence text; do not sell the font alone; do not reuse the reserved name on a modified version |
| JetBrains Mono (400, Latin) | `@fontsource/jetbrains-mono@5.3.0` | SIL Open Font License 1.1 | Same                                                                                                    |

Both are embedded unmodified, under their own names, with the licence text
alongside. The OFL permits embedding in a bundled application without extending
its terms to the application — which is why a font licence gets a row here and
not an ADR.

## Renderer dependencies that ship in the bundle

Direct dependencies only.

| Component                   | Version | Licence           |
| --------------------------- | ------- | ----------------- |
| React                       | 19.3.0  | MIT               |
| React DOM                   | 19.3.0  | MIT               |
| Zustand                     | 5.0.15  | MIT               |
| `@tauri-apps/api`           | 2.11.1  | Apache-2.0 OR MIT |
| `@tauri-apps/plugin-dialog` | 2.7.3   | Apache-2.0 OR MIT |

### Radix UI primitives

The design system is built on them — [#17](https://github.com/ismetcahangirov/blinkify/issues/17).
They supply focus management, escape handling, portal behaviour, roving focus
and ARIA wiring, all of which are re-implemented badly by everyone who tries.
Every package is MIT.

| Component                       | Version |
| ------------------------------- | ------- |
| `@radix-ui/react-context-menu`  | 2.3.7   |
| `@radix-ui/react-dialog`        | 1.1.23  |
| `@radix-ui/react-dropdown-menu` | 2.1.24  |
| `@radix-ui/react-popover`       | 1.1.23  |
| `@radix-ui/react-scroll-area`   | 1.2.18  |
| `@radix-ui/react-select`        | 2.3.7   |
| `@radix-ui/react-slider`        | 1.4.7   |
| `@radix-ui/react-slot`          | 1.3.3   |
| `@radix-ui/react-switch`        | 1.3.7   |
| `@radix-ui/react-tabs`          | 1.1.21  |
| `@radix-ui/react-tooltip`       | 1.2.16  |

They pull in a small set of their own — `react-remove-scroll`,
`react-style-singleton`, `use-callback-ref`, `use-sidecar`, `aria-hidden` and
`tslib` — all MIT or 0BSD, all shipped in the renderer bundle, and all covered
by the generated attribution bundle tracked in
[#73](https://github.com/ismetcahangirov/blinkify/issues/73).

**Storybook, axe-core and the testing libraries are deliberately absent from
this file.** They are development dependencies, they do not reach a user, and
listing them here would make the file a dependency inventory rather than an
attribution record. axe-core is worth naming in review for a different reason:
it is MPL-2.0, a file-level copyleft that is neither of the two licences
`CLAUDE.md` section 10 prohibits, and it is confined to the test run.

## Rust crates that ship in the binary

Direct dependencies of the two crates that compile into the executable. The
transitive set is **496 packages** and is gated by `cargo deny check licenses`,
which rejects GPL and AGPL outright and fails on any licence not explicitly
allowed in `deny.toml`.

| Crate                  | Version | Licence           | Why it ships                                                                                                                  |
| ---------------------- | ------- | ----------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `tauri`                | 2.11.6  | Apache-2.0 OR MIT | The shell. ADR-0001.                                                                                                          |
| `tauri-plugin-updater` | 2.12.0  | Apache-2.0 OR MIT | The update check — the one outbound request.                                                                                  |
| `tauri-plugin-dialog`  | 2.7.3   | Apache-2.0 OR MIT | Native open and save dialogs for projects and media (#54, #53). Brings `rfd` (MIT) and `tauri-plugin-fs` (Apache-2.0 OR MIT). |
| `serde`                | 1.0.229 | MIT OR Apache-2.0 | The IPC contract's serialisation.                                                                                             |
| `serde_json`           | 1.0.151 | MIT OR Apache-2.0 | Same.                                                                                                                         |
| `thiserror`            | 2.0.20  | MIT OR Apache-2.0 | Engine error types.                                                                                                           |
| `ts-rs`                | 12.0.1  | MIT               | Generates the TypeScript side of the IPC contract.                                                                            |
| `win32job`             | 2.0.3   | MIT OR Apache-2.0 | The kill-on-close job object that stops any `ffmpeg.exe` outliving Blinkify (#22).                                            |
| `cpal`                 | 0.18.2  | Apache-2.0        | Preview audio output through WASAPI; audio is the playback clock (#28).                                                       |

One entry is worth reading twice. `tauri-plugin-updater` pulls in `reqwest` and,
under it, `webpki-root-certs` — Mozilla's root CA bundle, licensed
**CDLA-Permissive-2.0**. That is a _data_ licence, not a code licence: it grants
use, modification and redistribution of the certificate data with no condition
beyond the disclaimers, and places no obligation on the software shipped beside
it. It is allowed explicitly in `deny.toml`, with that reasoning next to it,
because the licence gate rejected it until somebody made a decision — which is
the gate working.

## Code ported into Blinkify

Source code from another project, rewritten in Rust and compiled into the
engine. It is not a dependency — nothing is linked or downloaded — but it is
that project's work, and its licence travels with it.

| Component                                                                     | Version         | Licence | Obligation                               | Where                                                                                                      |
| ----------------------------------------------------------------------------- | --------------- | ------- | ---------------------------------------- | ---------------------------------------------------------------------------------------------------------- |
| [`smartcut`](https://github.com/skeskinen/smartcut) (the smart-cut algorithm) | `main`, 2026-09 | MIT     | Keep the copyright and permission notice | `crates/blinkify-engine/src/export/seam.rs` — [#41](https://github.com/ismetcahangirov/blinkify/issues/41) |

Ported: the split of a clip into remuxed whole GOPs and recoded partial GOPs at
cut points off a keyframe (`CutSegment` / `require_recode`), and the recoding
of an open GOP's leading pictures after a discontinuity
(`hybrid_recode_cra_segment`). Not ported: the in-process PyAV muxing and the
`libx264`/`libx265` encoders it depends on — Blinkify routes packets between
sidecar processes (ADR-0010) and encodes on hardware (ADR-0003). The notice:

```
MIT License

Copyright (c) 2024 Santtu Keskinen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Licence texts

**Not yet shipped with the installer.** This is an open obligation, not a solved
one.

The permissive licences above require the notice to travel with the binary. With
496 transitive packages, the only version of that which stays true is
**generated** at build time and bundled by the installer — a hand-maintained
table goes stale on the first `cargo update`, and a stale attribution file is a
compliance failure rather than a formatting one.

Tracked in [#73](https://github.com/ismetcahangirov/blinkify/issues/73). Until it
lands, this document is the record and it covers direct dependencies only.
