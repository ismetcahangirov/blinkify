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

| Component                              | Version | Licence      | Obligation                                                                                         | Status                                                                 |
| -------------------------------------- | ------- | ------------ | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| FFmpeg (LGPL build, no `--enable-gpl`) | —       | LGPL v2.1+   | Ship the licence text; keep the component replaceable by the user; publish the build configuration | Planned — [#21](https://github.com/ismetcahangirov/blinkify/issues/21) |
| RNNoise model weights                  | —       | BSD-3-Clause | Attribution                                                                                        | Planned — [#47](https://github.com/ismetcahangirov/blinkify/issues/47) |

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

Direct dependencies only; the Radix primitives arrive with the design system in
[#17](https://github.com/ismetcahangirov/blinkify/issues/17).

| Component         | Version | Licence           |
| ----------------- | ------- | ----------------- |
| React             | 19.3.0  | MIT               |
| React DOM         | 19.3.0  | MIT               |
| Zustand           | 5.0.15  | MIT               |
| `@tauri-apps/api` | 2.11.1  | Apache-2.0 OR MIT |

## Rust crates that ship in the binary

Direct dependencies of the two crates that compile into the executable. The
transitive set is **496 packages** and is gated by `cargo deny check licenses`,
which rejects GPL and AGPL outright and fails on any licence not explicitly
allowed in `deny.toml`.

| Crate                  | Version | Licence           | Why it ships                                       |
| ---------------------- | ------- | ----------------- | -------------------------------------------------- |
| `tauri`                | 2.11.6  | Apache-2.0 OR MIT | The shell. ADR-0001.                               |
| `tauri-plugin-updater` | 2.12.0  | Apache-2.0 OR MIT | The update check — the one outbound request.       |
| `serde`                | 1.0.229 | MIT OR Apache-2.0 | The IPC contract's serialisation.                  |
| `serde_json`           | 1.0.151 | MIT OR Apache-2.0 | Same.                                              |
| `thiserror`            | 2.0.20  | MIT OR Apache-2.0 | Engine error types.                                |
| `ts-rs`                | 12.0.1  | MIT               | Generates the TypeScript side of the IPC contract. |

One entry is worth reading twice. `tauri-plugin-updater` pulls in `reqwest` and,
under it, `webpki-root-certs` — Mozilla's root CA bundle, licensed
**CDLA-Permissive-2.0**. That is a _data_ licence, not a code licence: it grants
use, modification and redistribution of the certificate data with no condition
beyond the disclaimers, and places no obligation on the software shipped beside
it. It is allowed explicitly in `deny.toml`, with that reasoning next to it,
because the licence gate rejected it until somebody made a decision — which is
the gate working.

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
