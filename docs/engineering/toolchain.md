# Toolchain pins

Every version this repository pins, and **why** — so the next person does not
spend an hour working out whether a pin is deliberate or stale.

A pin with no recorded reason gets bumped by the first person who notices it is
behind, which is fine when the reason was "nobody updated it" and expensive when
the reason was "the thing above it breaks".

| What       | Pinned to | Where                    |
| ---------- | --------- | ------------------------ |
| Node       | 24.19.0   | `.nvmrc`, `engines.node` |
| pnpm       | 11.11.0   | `packageManager`         |
| Rust       | 1.98.1    | `rust-toolchain.toml`    |
| TypeScript | **6.0.3** | every `package.json`     |

## TypeScript is held at 6, and 7 is out

`typescript-eslint@8.70.0` declares `typescript: ">=4.8.4 <6.1.0"` and **throws
on import** under TypeScript 7:

```
Error: typescript-eslint does not support TS 7.0.
```

It is not a warning and not a degraded mode — ESLint cannot start at all, which
takes the lint gate with it. TypeScript 7 is the Go rewrite; `tsc` itself works
fine here and the typecheck gate passed on it. It is only the lint toolchain
that has not caught up.

**So the choice was:** a fast compiler with no lint gate, or a slower compiler
with every gate working. `CLAUDE.md` section 20, rule 10 — no disabling a gate
to go green — settles it. TypeScript 6.

Upstream tracking: [typescript-eslint#10940](https://github.com/typescript-eslint/typescript-eslint/issues/10940).
**Bump to 7 as soon as that lands**, in one pull request that moves
`typescript`, `typescript-eslint`, and this note together. Do not bump
TypeScript alone; the failure is at ESLint startup, so it will look like an
unrelated CI break.

## Node and pnpm

Pinned because an unpinned toolchain makes a CI failure unreproducible locally,
which is the failure mode that costs a whole evening. `.nvmrc` and
`packageManager` must agree with what CI installs.

## Rust

`rust-toolchain.toml` pins `1.98.1` with `rustfmt` and `clippy`, and the
`x86_64-pc-windows-msvc` target. `rustup` reads it automatically, so a
contributor gets the right toolchain by entering the directory rather than by
reading a document.

Clippy runs at `-D warnings`. A clippy that only advises is a clippy that gets
ignored, and several of the lints denied in the workspace `Cargo.toml` —
`indexing_slicing`, `cast_possible_truncation`, `unwrap_used` — exist precisely
because a length that came out of a media file must never reach an index or a
cast unchecked (`CLAUDE.md` section 11).

## pnpm settings live in `pnpm-workspace.yaml`, not `.npmrc`

pnpm 11 reads `nodeLinker`, `autoInstallPeers` and `allowBuilds` from
`pnpm-workspace.yaml`. Setting `node-linker=hoisted` in `.npmrc` **looks** like
it works — the install succeeds and says nothing — and quietly leaves the
isolated layout in place.

That matters here: the hoisted layout is what makes the dependency-cruiser
phantom-dependency rule in #11 mean the same thing as it does in TezUsta. A
silent fallback to the isolated layout would leave a gate that passes and proves
something different from what it claims.

Verify it rather than trusting it — a hoisted `node_modules/` has hundreds of
top-level entries, an isolated one has only the direct dependencies:

```bash
ls node_modules | wc -l     # hoisted: ~295.  isolated: ~17.
```

## Windows build prerequisites

Not pinned, but required, and missing them produces a link error rather than a
clear message:

- **MSVC build tools** — `winget install Microsoft.VisualStudio.2022.BuildTools`
  with the `Microsoft.VisualStudio.Workload.VCTools` workload. Rust's
  `x86_64-pc-windows-msvc` target needs the linker; without it `cargo build`
  fails at link time even though `cargo fetch` and `cargo metadata` succeed.
  The installer needs elevation — a non-elevated run exits with `1602`, which
  reads as "user cancelled".
- **Run cargo from PowerShell, not Git Bash.** Git Bash puts GNU coreutils on
  `PATH`, and coreutils ships a program called `link`. Rust invokes the linker
  as `link.exe`, but when MSVC's is absent the resolution can land on
  coreutils' instead, and the error you get is not "no linker found" — it is:

  ```
  = note: link: extra operand '...rcgu.o'
          Try 'link --help' for more information.
  error: linking with `link.exe` failed: exit code: 1
  ```

  That reads like a rustc bug and is not one. It means MSVC is missing, or a
  coreutils `link` is shadowing it.

- **WebView2 runtime** — ships with Windows 11. Confirm with the registry key
  under `HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\`. Windows 10
  may not have it; [#14](https://github.com/ismetcahangirov/blinkify/issues/14)
  makes the installer detect and guide.
