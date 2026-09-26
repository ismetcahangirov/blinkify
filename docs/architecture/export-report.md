# The export report

From [#52](https://github.com/ismetcahangirov/blinkify/issues/52), Epic
[#8](https://github.com/ismetcahangirov/blinkify/issues/8). Why it is measured
rather than read back from the plan:
[ADR-0016](../decisions/ADR-0016-the-export-report-is-measured-on-the-written-file.md).
Code: `crates/blinkify-engine/src/export/report.rs`; the shell's runner, and
`export_report`, `export_report_text`, `save_export_report`; the dialog in
`apps/desktop/src/export/ExportReportDialog.tsx`.

## How it is made

```
export() ─▶ ExportOutcome ─┐
plan ──────────────────────┼─▶ report::build ─▶ ExportReport ─▶ kept with the job
sources ───────────────────┘       │
                                    ├─ output: every packet of v:0 and a:0, hashed
                                    └─ sources: the packets around each segment, hashed
```

After the executor has written the file, the job's third stage, `verifying`,
hashes every packet of the output by the hash boundary of #45
(`verify::payload_hashes`) and, for each planned segment, the packets its
sources hold around its range (`payload_hashes_between`: from the keyframe
before, with three seconds either side, so a long source is not read whole
for a short clip). Each of the output's packets shown in the segment's range
is **identical** when its payload is one of those.

## What it says

| Part               | What                                                                              |
| ------------------ | --------------------------------------------------------------------------------- |
| summary            | "Fully lossless: every packet …", or the identical share and what was re-encoded  |
| per segment        | time range, stream, what was measured, packets and identical packets              |
| `execution`        | `copied`, `partly-re-encoded`, `re-encoded` or `empty`, from the count            |
| `asPlanned`        | the measurement is what the plan decided; otherwise the reason says both          |
| reasons            | `overview::explain` — the dialog's own words (#50)                                |
| suggestions        | what could have been done, where anything could (below)                           |
| encoder            | for encoded packets: the video encoder, profile, pixel format and level, or sound |
| totals, per stream | seconds copied and re-encoded, split by the identical share; packets              |
| `identicalPercent` | identical packets of all the output's packets                                     |
| sources, output    | name, container, codecs, size, duration                                           |
| commands           | every sidecar command the export ran — only in the text with paths                |

A smart-cut that needed no encoding — every picture of its window independent
of the other side — measures as copied, and that is as planned.

### What could have been done instead

| Cause                                  | Suggestion                                                  |
| -------------------------------------- | ----------------------------------------------------------- |
| cut between keyframes, open GOP at end | snap the cut to the nearest keyframe                        |
| keyframes not indexed yet              | wait for the index before exporting                         |
| speed no container carries             | a speed that plays between 1 and 240 fps                    |
| sequence not the clip's shape          | set the sequence to the clip in Sequence settings           |
| encoded differently from the rest      | export the clip on its own                                  |
| gap                                    | close the gap                                               |
| sound adjusted, sped up or mixed       | FLAC or PCM as the sound target — unless it already was one |
| hold, reverse                          | none: there are no source packets to copy                   |

## Kept, and shared

The report is kept with the job in the queue's file (#51, `exports.json`), so
it is there after a restart and in the history until the history forgets the
job. The job says `hasReport`; a file that could not be measured is still an
exported file, and its job simply has no report.

`to_text(with_paths)` is plain text for pasting into an issue. Without paths it
names files by name only and leaves out the commands, which name them by path;
the dialog's _Include folders_ switch is the only way to put them in.

## Tested

`crates/blinkify-engine/tests/export_report.rs`: a copy (fully lossless, every
segment copied as planned, the files described, the identical count equal to
the verification suite's own); a smart-cut (partly re-encoded as planned, the
encoder named, the reason with its distance, the snap suggested, the count the
suite's); a plan of a copy measured against a file made otherwise (reported as
re-encoded, not as planned); the text without and with folders; and the report
kept across a relaunch of the queue.
