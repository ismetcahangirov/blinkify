# Releasing Blinkify

How a build reaches a user, and the two things about it that are not yet good
enough.

## The short version

```bash
node tools/version.mjs --set 0.2.0     # every copy of the version, at once
pnpm verify                            # including the version gate
git commit -am "chore: release 0.2.0"  # via a pull request, like everything else
git tag v0.2.0 && git push origin v0.2.0
```

The tag starts [`release.yml`](../../.github/workflows/release.yml). It builds,
signs the update manifest, and opens a **draft** GitHub Release. Check the
installer runs, then publish it by hand.

Publishing is the moment every installed copy of Blinkify starts offering the
new version. That is a decision, not the last line of a script.

## The version has one source

`package.json` at the repository root. Everything else is derived from it or
checked against it:

| Where                                    | How                                          |
| ---------------------------------------- | -------------------------------------------- |
| `apps/desktop/src-tauri/tauri.conf.json` | Derived — `"version": "../package.json"`     |
| `Cargo.toml` `[workspace.package]`       | Checked by `pnpm version:check`              |
| The three workspace `package.json`s      | Checked by `pnpm version:check`              |
| The git tag                              | Checked by `--against-tag`, in `release.yml` |

`pnpm version:check` runs in `pnpm verify` and in CI. Never edit a version by
hand — `node tools/version.mjs --set <version>` writes all five and refreshes
`Cargo.lock`.

**Why a gate rather than a convention.** The expensive failure is not a wrong
number in a file. It is an installed build that reports `0.2.0` while the
manifest offers `0.2.0`, so the updater concludes there is nothing to do and the
user sits on a broken version forever — with every check green. A tag that
disagrees with the tree produces exactly that, which is why `release.yml`
compares them before it builds anything.

## The updater signing key

The updater verifies a signature on the release manifest and refuses anything
that fails. That is the whole security model of auto-update: without it, anyone
who can serve a manifest can serve an executable to every installed copy.

**This is already done.** The keypair exists, the private half is in Actions
secrets, and the public half is in `tauri.conf.json`. What follows is the
procedure — for a reader who needs to understand it, and for the day it has to
be repeated.

The keypair is generated **once**, by the repository owner, on their own
machine:

```bash
pnpm tauri signer generate -w $HOME/.blinkify/updater.key
```

- The **private** key goes into GitHub Actions secrets and nowhere else:

  ```bash
  gh secret set TAURI_SIGNING_PRIVATE_KEY < $HOME/.blinkify/updater.key
  gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
  ```

- The **public** key goes into `tauri.conf.json` under `plugins.updater.pubkey`.
  It is public by design; committing it is the point.

`CLAUDE.md` section 11 and section 20 rule 7: the private key is never in the
repository, never in a log, never in a commit message. Back it up somewhere you
control — **losing it means no installed build can ever be updated again**, and
the only remedy is asking every user to reinstall by hand.

`release.yml` fails before it builds if the secret is missing. A build without
the key produces a manifest with no signature, every client correctly rejects
it, and nobody finds out until the next release does not arrive.

### Rotating it is not free

Changing the keypair means every **already-installed** copy holds the old public
key and will reject everything signed with the new one. Those installations are
stranded: they have to be replaced by hand. So rotate only if the private key is
believed compromised, and expect to publish a direct download alongside it.

### The signature check is tested, not assumed

`apps/desktop/src-tauri/src/updater.rs` carries the negative cases — a tampered
payload, a signature made with a different key, a signature that does not parse
— and a positive control, so the tests cannot all pass because verification
rejects everything. None of them needs the production private key: they generate
a throwaway keypair, which is what makes the positive case possible at all.

## The installer is not code-signed

Windows SmartScreen shows **"Windows protected your PC"** on first run, and
Defender may flag the download.

This is what Windows does with any unsigned installer from a publisher it has
not seen before. It is not a defect in the build, and there is no way around it
in the build.

**Do not attempt to work around SmartScreen.** Every technique that suppresses
the warning without a certificate is a technique malware uses, and the ones that
work today stop working — usually by making the reputation problem worse.

The fix is an Extended Validation or standard code-signing certificate, plus the
download reputation that accumulates after it. That is money and calendar time,
not engineering, and it is tracked as its own issue. Until then the release notes
say so plainly, in front of the download rather than after it.

## What a release contains

| Asset                              |                                                              |
| ---------------------------------- | ------------------------------------------------------------ |
| `blinkify_<version>_x64-setup.exe` | The NSIS installer. Per-user, no elevation prompt.           |
| `latest.json`                      | The update manifest, signed. This is what the updater reads. |

The install is **per-user** deliberately: a consumer video editor that demands
an administrator prompt to install is a consumer video editor people close.

WebView2 is set to `downloadBootstrapper`, so a machine without it — Windows 10,
usually; Windows 11 ships it — is offered the runtime during install rather than
launching to a blank window.

## Known gaps

Stated here rather than discovered later.

### The `.blinkify` association is registered, and opening one does nothing

`tauri.conf.json` registers the `.blinkify` extension, so Windows shows the
Blinkify icon on those files and double-clicking one launches the application.

**The application then ignores the path it was given.** The project file format
is defined in #32, and until it exists there is nothing to open. The association
is registered now because adding it later means every already-installed copy
keeps the old registry entries until it is reinstalled.

Do not read a working association as a working open path. #32 closes the gap,
and the end-to-end check — double-clicking a `.blinkify` file opens that
project — belongs to #32, not here.

### Auto-update has not been exercised against a real release

The path is wired and the signature check is tested, but "publish 0.2.0 and
watch an installed 0.1.0 offer it" needs two real releases. That happens at the
first real release; it is not something a pull request can prove.

### Licence texts do not ship with the installer yet

The permissive licences in the tree require the notice to travel with the
binary, and it does not. `THIRD_PARTY.md` records the direct dependencies and
why each one ships; the generated attribution bundle for all 496 transitive
packages is [#73](https://github.com/ismetcahangirov/blinkify/issues/73).

### Install, update and uninstall have not been run on a clean machine

The installer is built and uploaded by CI on every run, and it has been run on
the development machine. A clean Windows 11 virtual machine and a
non-administrator account are the remaining checks, and they are manual by
nature.

## Related

- [`ci.md`](./ci.md) — the pull-request and release-gate pipelines
- [`toolchain.md`](./toolchain.md) — pinned versions, and the WebView2 registry check
- [`../../CLAUDE.md`](../../CLAUDE.md) sections 11, 19 and 20
