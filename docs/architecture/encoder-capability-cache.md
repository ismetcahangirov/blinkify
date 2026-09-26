# The encoder capability cache

From [#83](https://github.com/ismetcahangirov/blinkify/issues/83), following
[#21](https://github.com/ismetcahangirov/blinkify/issues/21). Code:
`crates/blinkify-engine/src/capability_cache.rs` and
`crates/blinkify-engine/src/display_driver.rs`.

[ADR-0003](../decisions/ADR-0003-re-encode-encoder-strategy.md) part 1 says
the encoder capability profile is measured, not assumed — every advertised
encoder is opened and made to encode — and that it is cached, keyed on the
sidecar build and the driver version, and re-probed when that key changes. This
is that cache.

## Why it exists

Measured on 2026-09-26 on a laptop with an NVIDIA GeForce RTX 3060 and an
Intel UHD iGPU, debug build, by timing the two launches in
`a_second_launch_on_an_unchanged_machine_starts_no_encoder_trial`
(`crates/blinkify-engine/tests/sidecar.rs`), which asserts the counts:

| Launch                | Processes started | Time until the profile is known |
| --------------------- | ----------------: | ------------------------------: |
| Probe (no cache)      |                36 |                          44.8 s |
| Cached, key unchanged |                 0 |                            6 ms |

The probe never blocked anything — it runs on a background thread at
background priority — but for those 45 seconds the export dialog could only say
"checking", and every launch repeated them to reach the same answer.

## The key

The answer changes only when one of two things does, so the key is exactly
those two:

| Part                                   | Why                                                                                                                                                                                                                                                                        |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SHA-256 of `ffmpeg.exe`                | The sidecar decides what is advertised and how each encoder is driven. The hash, not the version string: a rebuild with a changed configure line keeps its version. It is the same hash `tools/ffmpeg-sidecar/sidecar.lock.json` records, and the test asserts they agree. |
| Every display adapter's driver version | A hardware encoder is the driver's. A new driver can add a profile, raise a level ceiling, or break an encoder that worked. A new or removed adapter changes the list too.                                                                                                 |

The driver list is read from the display adapter device class in the registry,
`HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}`:
one numbered subkey (`0000`, `0001` …) per installed adapter, each carrying
`DriverDesc` and `DriverVersion`. That is where the driver's INF writes them
and where Device Manager and WMI read them back; on the machine above it gives
exactly what `Win32_VideoController` reports (`32.0.15.7688` for the NVIDIA
part, `31.0.101.2141` for the Intel one). A standard user can read it. No
process is started.

An adapter key that cannot be read is left out rather than failing the list.
That can only cost a probe — the key changes when the entry later appears — and
never a stale answer, because anything that changes the list changes the key.

## The file

One file, `<app cache dir>/artefacts/capabilities/encoders.json`, in the same
size-budgeted cache as keyframe indices and waveform peaks, written through the
same atomic write-and-rename:

```json
{ "version": 1, "key": { "sidecarSha256": "…", "displayDrivers": [ … ] }, "capabilities": { … } }
```

The key lives inside the file rather than in its name, so a changed key
replaces the old profile instead of leaving it behind.

Reading it, in order, each step a reason to probe again and never an error:

1. The file is missing or unreadable.
2. It is not JSON, or it is truncated.
3. Its `version` is not the one this build writes. The version is read on its
   own first, so a file from another version is recognised as such rather than
   misread as this one because its fields happen to parse.
4. Its key is not this machine's key.

`FORMAT_VERSION` is bumped whenever the profile or the key changes shape or
meaning — including a new candidate encoder in the probe, which a profile
written before it would lack.

## Never stale

At launch the shell starts one background thread that computes the key, then
takes the profile from the cache or probes. Until that thread finishes, the
`encoder_capabilities` command answers `None` — "checking". A profile probed
under a different key is not shown in the meantime: a profile from before a
driver update is exactly the "listed but fails at export" case ADR-0003 exists
to prevent, so being briefly unsure is the correct state, and a stale answer
presented as current is not.

## What was rejected

**Reading driver versions through WMI** (`Win32_VideoController`). The same
numbers, but either through COM from Rust — a `wmi` crate and its dependency
tree, `unsafe`-heavy initialisation on the calling thread, and a service that
can take seconds to answer at logon — or by starting `powershell.exe` with a
query script, which is a process per launch and a program passed as a string,
exactly what `CLAUDE.md` section 20 rule 5 exists to keep out.

**DXGI** (`IDXGIAdapter::CheckInterfaceSupport`). Returns the user-mode driver
version per adapter, but only through COM calls the workspace's `forbid(unsafe)`
does not allow in our code, and it reports adapters DXGI enumerates rather than
every installed one.

**SetupAPI or the configuration manager** (`SetupDiGetDeviceRegistryProperty`,
`CM_Get_DevNode_Property`). The canonical device-property route, and the
registry values above are what it reads; it adds raw FFI for no information the
registry does not already give.

**The sidecar's version string instead of its hash.** Two builds of the same
FFmpeg release with different configure lines would share a key and a profile.

**The sidecar's size and modification time.** Cheap, and wrong the day an
installer lays down a different binary of the same size with a preserved
timestamp. Hashing 60 MB once per launch, on a background thread, is not worth
saving.

**A cache file per key** (the hash in the file name, as the content-keyed
artefacts do). Every driver update would leave an orphaned profile behind for
the size budget to find eventually, and there is only ever one current answer.
