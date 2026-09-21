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

## Renderer dependencies that ship in the bundle

Populated as the scaffold lands in
[#10](https://github.com/ismetcahangirov/blinkify/issues/10) and the design
system in [#17](https://github.com/ismetcahangirov/blinkify/issues/17).
Expected: React, Zustand and Radix primitives, all MIT.

| Component  | Version | Licence |
| ---------- | ------- | ------- |
| _none yet_ |         |         |

## Rust crates that ship in the binary

Populated as the engine lands. Gated by `cargo deny check licenses`, which
rejects GPL and AGPL outright.

| Crate      | Version | Licence |
| ---------- | ------- | ------- |
| _none yet_ |         |         |

## Licence texts

Full licence texts are shipped alongside the installed application. Until the
packaging issue ([#14](https://github.com/ismetcahangirov/blinkify/issues/14))
lands, this table is the record.
