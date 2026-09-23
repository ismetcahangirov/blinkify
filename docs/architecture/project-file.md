# The edit graph and the project file

From [#32](https://github.com/ismetcahangirov/blinkify/issues/32) (Epic
[#5](https://github.com/ismetcahangirov/blinkify/issues/5)). Why the format is
what it is: [ADR-0006](../decisions/ADR-0006-project-file-format.md).

## The model

`blinkify_engine::project`:

```
Project
├── schemaVersion
├── name
├── sources: { id → SourceRef { path, fingerprint { size, modified, contentHash } } }
└── sequence
    ├── settings: SequenceSettings   (width, height, frameRate, pixelAspect, colour — #57)
    └── tracks: [ Track { id, kind: video | audio,
                          clips: [ Clip { id, source, stream, timeBase, start,
                                          operations: [ Operation ] } ] } ]

Operation = trim { from, to }        source ticks, `to` exclusive
          | speed { ratio }          rational, 2/1 is double speed
          | gain { db }
          | denoise { strength }     0…1
          | normalise { targetLufs }
```

Operations are data, and a clip's are private to the `project` module.
Nothing in the model decides what a gain does to a sample or whether a trim
is keyframe-aligned. The shared evaluator (#30) resolves them once, and the
preview and the planner (Epic #6) both consume what it returns; see
[`edit-graph-evaluation.md`](./edit-graph-evaluation.md). `Project::validate`
only checks what the types cannot: unique ids, every clip's source present,
parameters in range.

Sequence settings are defined once, in `project::settings`, and referred to
by the sequence. What they mean — how they are chosen, and how they bind copy
eligibility — is #57.

## Time

Two time bases, both rational, both integer:

| Where                  | Unit                                                              |
| ---------------------- | ----------------------------------------------------------------- |
| `Clip.start`           | Sequence ticks: one per frame (`SequenceSettings::time_base`)     |
| `trim.from`, `trim.to` | Ticks of the source stream's time base, stored as `Clip.timeBase` |

`Clip::length` and `Clip::source_at` convert between them through
`crate::time::rescale`. A speed change multiplies the source time base by the
ratio, so "0.7 s of source at triple speed" is seven 30 fps frames exactly
and never passes through 0.2333… seconds.

Rounding is chosen, not defaulted. A clip's length rounds **up**, so a
partial last frame still shows; the source tick for a timeline frame rounds
**down**, to the frame on screen at that moment. The VFR test runs a clip
across the 30 → 10 fps change in `vfr-screen.mp4` at four sequence rates and
checks every timeline frame lands on a real source frame inside the trim.

## Sources

A `SourceRef` is made from an `ExportSource` — the original file, never a
proxy (#26) — and cannot be made any other way. Its fingerprint identifies
the content: size plus a SHA-256 over the size and the first and last
mebibyte. The modification time is kept for display only.

Opening a project never fails because of a source. `Project::check_sources`
reports each one:

| Status    | Meaning                                                          | What the user is offered                       |
| --------- | ---------------------------------------------------------------- | ---------------------------------------------- |
| `present` | The same content at the recorded path                            | —                                              |
| `missing` | Nothing readable at the path: moved, renamed, drive disconnected | Relink: drop the file on the banner            |
| `changed` | A file at the path, but different content                        | Told it changed; relinking cannot undo an edit |

`Project::relink` accepts a candidate only if it has the same content, so
relinking cannot quietly swap in a different take. The shell saves the
project after a relink.

## The file

Pretty JSON, fields in declaration order, ordered maps, one trailing newline;
floats parsed with `float_roundtrip`. `Project::save` writes a sibling
`.blinkify.saving` and renames it over the project, so a crash never leaves
half a file.

Loading is `JSON → migrate::to_current → typed → validate`. The errors a user
can see:

| Error           | When                                                         |
| --------------- | ------------------------------------------------------------ |
| `Corrupt`       | Not JSON, no `schemaVersion`, or not the shape of the schema |
| `FutureVersion` | Written by a newer Blinkify; names both schema versions      |
| `Invalid`       | Well-formed but inconsistent — a clip with no source         |
| `Io`            | The file cannot be read or written                           |

### Changing the schema

1. Bump `SCHEMA_VERSION`.
2. Append the migration from the previous version to `migrate::MIGRATIONS`.
   It takes and returns `serde_json::Value` and must set the new
   `schemaVersion`.
3. Commit a file written by the previous version as
   `tests/fixtures/projects/v<N>.blinkify` — generated by the previous build,
   not hand-written.
4. `cargo test -p blinkify-engine --test project` opens every fixture.

## Opening from Explorer

The installer registers `.blinkify` (#14). A double-click starts Blinkify
with the file as its first argument; the shell keeps it (`LaunchFile`) and
the renderer asks for it with `launch_project` once it has mounted. The
project name appears in the app bar and the window title; offline sources appear in a banner that
is also the relink drop target. A project that cannot be opened says why
there.

## The TypeScript side

Every type above is exported by ts-rs into `packages/types`. `pnpm
types:check` regenerates the contract into a scratch directory and fails if
the committed one differs, lacks a file, or keeps a file no Rust type
generates — see [`architecture-gates.md`](../engineering/architecture-gates.md).
