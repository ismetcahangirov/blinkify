# Sequence settings and copy eligibility

From [#57](https://github.com/ismetcahangirov/blinkify/issues/57). Why:
[ADR-0008](../decisions/ADR-0008-sequence-settings-and-copy-eligibility.md).
Code: `crates/blinkify-engine/src/project/settings.rs`, and
`Document::eligibility` / `settings_impact` in `project/edit.rs`.

## The settings

| Field         | Meaning                                                        |
| ------------- | -------------------------------------------------------------- |
| `width`       | Pixels, even, 2 to 8192                                        |
| `height`      | Pixels, even, 2 to 8192                                        |
| `frameRate`   | Exact rational, 1 to 240 frames a second                       |
| `pixelAspect` | Exact rational; display aspect is width × pixelAspect : height |
| `colour`      | `sdr` — Rec. 709; the only policy in v1                        |

`SequenceSettings::validate` refuses what no file can carry; the dialog shows
the refusal and cannot apply it.

## Where the settings come from

```
new project ──▶ matchFirstClip = true, placeholder 1080p30
                       │
first video clip placed (AddClip, same edit) ──▶ settings = the clip's,
                       │                          matchFirstClip = false
settings chosen in the dialog (SetSettings) ───▶ matchFirstClip = false
```

Undo of the first placement returns the placeholder and the flag.

## The predicate

`copy_eligibility(settings, geometry)` compares the sequence with a source's
`StreamGeometry` — its **display** size (rotation applied: the copied stream
keeps its rotation and plays upright), its **nominal** frame rate (the average
for a constant-rate source, the timestamp base for a variable one), and its
pixel aspect. Rates compare after reduction, so `60000/2002` equals
`30000/1001`. Each difference is its own `Mismatch`; a variable-rate or HDR
source adds a `CopyNote`.

| Source                           | Sequence 1080p 30 | Result                               |
| -------------------------------- | ----------------- | ------------------------------------ |
| 1920×1080, 30 fps                | —                 | eligible                             |
| 3840×2160, 30 fps                | —                 | `resolution`                         |
| 1920×1080, 24 fps                | —                 | `frame-rate`                         |
| 1920×1080, 30 fps VFR phone clip | —                 | eligible, note `variable-frame-rate` |
| 1920×1080, 30 fps HLG            | —                 | eligible, note `hdr`                 |

## Who reads it

- `ProjectView.eligibility` carries the answer per source with pictures.
- The timeline marks every video clip of an ineligible source with a band
  along its bottom edge (`ineligibleClips`), before any export.
- The settings dialog asks `preview_settings` what a change would cost —
  clips and duration that would stop or start being copy-eligible — and says
  so before Apply.
- The planner (#39), the inspector (#56) and the export dialog (#50) read the
  same field; none of them works it out again.
