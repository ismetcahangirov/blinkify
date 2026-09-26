# The FFmpeg sidecar

Blinkify runs its own build of `ffmpeg.exe` and `ffprobe.exe` as separate
processes. This document covers where they come from, how they reach your
machine, and how to rebuild or upgrade them.

Why it is a separate process and an LGPL build:
[ADR-0002](../decisions/ADR-0002-lgpl-ffmpeg-sidecar.md). Why we build it
ourselves rather than download one:
[ADR-0004](../decisions/ADR-0004-build-the-ffmpeg-sidecar-ourselves.md).

## Getting it

```bash
pnpm sidecar:fetch
```

Downloads the pinned zip, checks its SHA-256 and each binary's SHA-256 against
[`tools/ffmpeg-sidecar/sidecar.lock.json`](../../tools/ffmpeg-sidecar/sidecar.lock.json),
and puts them in `apps/desktop/src-tauri/binaries/` under the names Tauri
expects (`ffmpeg-x86_64-pc-windows-msvc.exe`). Nothing is written if a hash
does not match. It is a no-op when the verified binaries are already there, and
`pnpm verify` runs it first.

**You need it before any Rust build of the shell.** Tauri refuses to compile
`blinkify-desktop` if an `externalBin` is missing, with:

```
resource path `binaries\ffmpeg-x86_64-pc-windows-msvc.exe` doesn't exist
```

The engine's integration tests run the real sidecar too, and fail — not skip —
without it.

The binaries are never committed. They are ignored by git, and
`pnpm sidecar:check` fails if one is ever tracked.

## At runtime

The application resolves the sidecar next to its own executable, where the NSIS
installer puts it and where `tauri dev` copies it. It never searches `PATH`: a
system FFmpeg may be a GPL build or a different version, and silently using it
would make both the licence position and the behaviour unknowable. A missing
sidecar is an error the user sees, not a fallback.

## The gate

```bash
pnpm sidecar:check        # the binaries in apps/desktop/src-tauri/binaries
pnpm sidecar:check:test   # proves each rule rejects what it should
```

`sidecar:check` asks the binary what it is — `-version`, `-encoders`,
`-filters` — and fails on any of:

| Rule                            | Fails when                                                                                               |
| ------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `no-gpl-configuration`          | `--enable-gpl`, `--enable-nonfree`, or a GPL library appears in the configuration                        |
| `no-software-h264-hevc-encoder` | `openh264`, `kvazaar`, `vvenc`, `libx264`, `libx265` or a Media Foundation H.264/HEVC encoder is present |
| `configuration-is-ours`         | the configure line differs from `configure.txt` in any flag                                              |
| `required-components`           | an encoder or filter Epics #6 and #7 depend on is missing — `arnndn`, `loudnorm`, `hevc_nvenc` …         |
| `provenance`                    | a binary's SHA-256 differs from the lock, `THIRD_PARTY.md` names another version, or a binary is tracked |

Both run in CI, in the _Boundaries and licences_ job. The injection test drives
the rules with real configuration strings, including BtbN's own `win64-lgpl`
build, which must fail.

## What the machine can encode

The sidecar lists every hardware encoder on every machine. That is not the same
as the machine having one. At launch the engine opens each candidate encoder
and makes it encode a few synthetic frames at each profile and bit depth
(`crates/blinkify-engine/src/capability.rs`), and reports only what worked.
Nothing downstream may assume an encoder exists. See ADR-0003 part 1.

The answer is cached, keyed on the SHA-256 of `ffmpeg.exe` and every display
driver's version, so replacing the sidecar re-probes on the next launch by
itself — there is no cache to clear after an upgrade. See
[`encoder-capability-cache.md`](../architecture/encoder-capability-cache.md).

## Rebuilding or upgrading

Do this when moving to a new FFmpeg release, when a library needs a security
fix, or when the configure line changes. Each step is a reviewed change; an
FFmpeg upgrade can change packet timing, which is the kind of change that
quietly breaks a losslessness guarantee.

1. **Change the inputs** in `tools/ffmpeg-sidecar/`:
   - For a new FFmpeg release: `source.tag` and `source.commit` in
     `sidecar.lock.json` (`git ls-remote https://github.com/FFmpeg/FFmpeg.git
refs/tags/<tag>^{}` gives the commit), and bump `version`.
   - For a new toolchain: pull `ghcr.io/btbn/ffmpeg-builds/win64-lgpl-<major.minor>`
     and record its digest (`docker inspect --format '{{index .RepoDigests 0}}'`).
   - For a new library or flag: `configure.txt`, with a comment saying what uses
     it and its licence. Check the licence before the API (`CLAUDE.md`
     section 9).
2. **Build.** Either run the _FFmpeg sidecar_ workflow from the Actions tab and
   download its artifact, or locally with Docker:

   ```bash
   bash tools/ffmpeg-sidecar/build.sh
   ```

   The zip lands in `tools/ffmpeg-sidecar/dist/`, and the script prints the
   SHA-256 of the zip and of both binaries. Budget about half an hour.

3. **Publish the zip** as an asset on a release tagged `ffmpeg-sidecar-<version>`,
   marked as a pre-release and **not** as latest — the updater reads the latest
   release, and an FFmpeg zip must never be what it finds:

   ```bash
   gh release create ffmpeg-sidecar-<version> tools/ffmpeg-sidecar/dist/*.zip \
     --prerelease --latest=false --title "FFmpeg sidecar <version>" \
     --notes "LGPL FFmpeg build for Blinkify. Source and configuration: tools/ffmpeg-sidecar/"
   ```

4. **Record** the asset URL and the three hashes in `sidecar.lock.json`, and the
   version and configure line in `THIRD_PARTY.md`.
5. **Verify** with `pnpm sidecar:fetch && pnpm sidecar:check`, then run the
   losslessness suite before merging. Open the pull request against the issue
   that motivated the upgrade.

## When it goes wrong

- **`zip SHA-256 is …, sidecar.lock.json records …`** — the asset changed
  after it was pinned, or the download was truncated. Never "fix" this by
  updating the hash without knowing why the bytes changed.
- **`configuration-is-ours` fails with `unexpected --enable-…`** — the binary
  was not built from `configure.txt`. Usually a hand-copied binary; fetch again.
- **`required-components` fails after an upgrade** — a component was renamed or
  dropped upstream. Find what replaced it before touching the list; Epics #6 and
  #7 depend on every entry.
