# ADR-0004 — Build the FFmpeg sidecar from a committed configure line

- **Status**: Accepted
- **Date**: 2026-09-23
- **Context issue**: [#21](https://github.com/ismetcahangirov/blinkify/issues/21)

> [ADR-0002](./ADR-0002-lgpl-ffmpeg-sidecar.md) decided _what_ the sidecar is: an
> LGPL FFmpeg build, run as a separate process. This ADR decides _where the
> binary comes from_, because the obvious answer turned out to violate
> [ADR-0003](./ADR-0003-re-encode-encoder-strategy.md).

## Context

Nobody wants to build FFmpeg for Windows by hand. The usual answer is to
download a prebuilt one, and for an LGPL build there is essentially one serious
source: [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds), which
publishes `win64-lgpl` zips daily. The popular gyan.dev builds are GPL only.

Inspecting BtbN's `ffmpeg-n8.1.3-win64-lgpl-8.1` build, which `ffmpeg -version`
does honestly, shows three problems:

1. **It contains `libopenh264`.** ADR-0003 part 3 says Blinkify ships no
   software H.264 encoder, `openh264` by name, for two independent reasons — it
   cannot match a High-profile seam, and self-distributed `openh264` binaries
   carry no Cisco patent undertaking. An encoder inside the binary is shipped
   whether or not Blinkify ever selects it. It also contains `libkvazaar`
   (software HEVC) and `libvvenc` (software VVC), which raise the same patent
   question.
2. **It is configured `--enable-version3`**, making it LGPL **v3**, to admit
   libraries such as `opencore-amr` and `aribb24`. ADR-0002 and `THIRD_PARTY.md`
   both state LGPL v2.1-or-later, and nothing Blinkify does needs a v3-only
   library.
3. **It enables network protocols** — `schannel`, `libsrt`, `librist`,
   `libssh`, `libzmq`. `CLAUDE.md` forbidden behaviour 8 allows one network
   call, the update check, and it is not made by FFmpeg.

The build also changes daily and old builds are pruned, so "pin the download
URL" is not a pin.

## Decision

**Blinkify builds its own sidecar, from a configure line committed at
[`tools/ffmpeg-sidecar/configure.txt`](../../tools/ffmpeg-sidecar/configure.txt),
on BtbN's published cross-compilation image pinned by digest, and ships the
result as a release asset pinned by SHA-256.**

- **The toolchain is borrowed; the choices are ours.** BtbN's
  `win64-lgpl-8.1` image supplies mingw-w64 and every library pre-built as a
  static archive. It also supplies an `FF_CONFIGURE` variable with their
  choices, which `build.sh` deliberately ignores. What gets enabled is exactly
  the committed file.
- **`--disable-autodetect`.** A library that turns up because the image had it
  is a library nobody reviewed the licence of. Everything enabled is named.
- **`--disable-network`.** The sidecar can open local files and pipes, nothing
  else.
- **No `--enable-version3`.** The build is LGPL v2.1-or-later.
- **Everything is pinned in
  [`sidecar.lock.json`](../../tools/ffmpeg-sidecar/sidecar.lock.json)**: FFmpeg
  tag _and_ commit (a tag can move), image digest, the release asset's URL and
  SHA-256, and the SHA-256 of each binary.
- **The sidecar gate enforces it.** `pnpm sidecar:check` asserts that the
  configure line printed by the binary is exactly the committed one, so a
  binary from anywhere else — including BtbN's own "LGPL" build — fails CI.

The procedure is in
[`docs/engineering/ffmpeg-sidecar.md`](../engineering/ffmpeg-sidecar.md).

## Alternatives considered

### Ship BtbN's prebuilt `win64-lgpl` build — rejected

The cheapest option by far and a well-maintained build. Rejected for the three
problems in the Context: it ships `openh264` against ADR-0003, it is LGPL v3
rather than the v2.1+ our licence documents state, and it can open network
connections. Any one would be enough. The gate's injection test uses this build's
real configuration string as a case that must fail.

### Ship a prebuilt build and document the discrepancies — rejected

Keep BtbN's binary, never select `libopenh264`, and amend `THIRD_PARTY.md` to
say LGPL v3. Rejected because "present but never selected" is exactly the
position ADR-0003 part 3 refuses: the patent exposure attaches to distributing
the encoder, not to calling it. Documenting a violation does not make it one
fewer.

### Build every dependency from source ourselves — rejected

Maximum control: no borrowed image, every library compiled by our scripts.
Rejected on cost. It is BtbN's entire project, maintained daily, and the
libraries it builds are the same upstream sources we would use. Borrowing the
image pinned by digest gives a reproducible toolchain without our maintaining
one; what we must own — the configure line — we do.

### Build the sidecar on every CI run — rejected

No release asset to manage and no lock file to update. Rejected because an
FFmpeg build takes longer than the 15-minute pull-request budget on its own, and
because a binary rebuilt per run is a binary whose hash nobody can record in
`THIRD_PARTY.md`. The build runs manually, in
[`ffmpeg-sidecar.yml`](../../.github/workflows/ffmpeg-sidecar.yml) or locally,
when the pin moves.

## Consequences

### What this makes easy

- The binary matches the ADRs by construction, and CI proves it on every pull
  request by asking the binary itself.
- An upgrade is a reviewed diff: a new tag, a new digest, new hashes. Filter
  behaviour and packet timing change between FFmpeg versions, and that is
  exactly the change that quietly breaks a losslessness guarantee, so it should
  arrive as a pull request and not as a download.
- The sidecar is smaller than a distributor build — no `ffplay`, no subtitle
  renderer, no network stack.

### What this makes hard

- **Upgrading is work.** Someone has to run the build, publish the asset and
  update the lock. The procedure is written down so that it is only work, not
  research.
- **We depend on BtbN's image staying available** by digest. If it disappears
  the pinned build still exists as our release asset; only the _next_ build
  needs a new toolchain.
- **Libraries not in the image are not available** without building them.
  Nothing Blinkify needs is missing today.

### What we accept

- We host an FFmpeg binary as a release asset of this repository, and LGPL
  obliges us to make its corresponding source available. `SOURCE.md` inside the
  zip names the exact commit and the build scripts in this repository; that is
  the offer.
