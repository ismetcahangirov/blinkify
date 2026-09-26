# The export queue

From [#51](https://github.com/ismetcahangirov/blinkify/issues/51), Epic
[#8](https://github.com/ismetcahangirov/blinkify/issues/8). Why it is built
this way:
[ADR-0014](../decisions/ADR-0014-an-export-job-is-a-project-snapshot-restarted-never-resumed.md).
Code: `crates/blinkify-engine/src/export/queue.rs` (the queue) and
`apps/desktop/src-tauri/src/export.rs` (the shell's runner and commands).

## What a job is

```
submit_export ──▶ ExportSpec { project (a copy), target, overwrite, audio }
                        │ queued, in order
                        ▼
 worker thread ──▶ Runner::run ──▶ evaluate ─▶ resolve loudness ─▶ plan ─▶ execute
                        │                     (Preparing)               (Exporting)
                        ▼
                  Completed { bytes } | Failed { message } | Cancelled
```

- The job holds a copy of the project as it was when the export was asked
  for. The user goes on editing; the job plans from its copy when it starts.
- The queue refuses a target that is one of the project's sources, however
  the path is spelled. So does the executor, for every caller, even with
  replacing a file confirmed (`TargetIsSource`).
- The engine's queue knows nothing about how an export is made: the shell
  passes a `Runner` that resolves loudness (ADR-0013), plans (#39) and
  executes (#40). The engine tests pass one that runs the executor over the
  corpus, and scripted ones where the queue's own behaviour is under test.

## Order and concurrency

One worker thread takes the first queued job, runs it, records how it ended,
and takes the next. One export at a time: the processes of one export already
fill the orchestrator's export slots (#110).

## Progress

| Stage       | What it is                                | Progress                     |
| ----------- | ----------------------------------------- | ---------------------------- |
| `preparing` | loudness measured, the plan made          | none: its length is unknown  |
| `exporting` | the executor writing the output's streams | the muxer's `-progress` time |
| `verifying` | the output measured for its report (#52)  | none: its length is unknown  |

The fraction never goes back, and a stage never returns to the one before. The
time left is `elapsed × (1 − fraction) ÷ fraction` from the time measured in
the stage — offered only after a second and 1 % of it, since a rate measured
over less is noise. Nothing comes from a timer.

## Cancelling

`cancel_export` on a queued job marks it cancelled; it never starts. On the
running job it trips the job's cancel token, which stops every process of the
export; the executor removes the partial file. The job is recorded as
cancelled only when the user asked: the executor also trips the token to stop
its own processes when an export fails, and that is a failure, with its
reason. An export that completed before the request arrived is reported as
completed.

## On disk

The queue is kept in `exports.json` in the application's data folder, written
to a file beside it and renamed over it, before each change is announced.

- A job the file records as queued or running when the queue opens was
  **interrupted**. It is offered — "export again" from the start, or discard —
  and never runs until the user chooses (ADR-0014). Either removes what the
  interrupted export left beside the target.
- Closing Blinkify during an export stops it and leaves it interrupted.
- A finished job keeps its outcome and its report
  ([`export-report.md`](./export-report.md)), not its project. The history
  keeps the last 50.
- A file that cannot be read is set aside as `exports.json.unreadable` and the
  queue starts empty: losing a history is better than not being able to
  export.

## In the interface

- **Exports** in the application bar opens the queue: each job with its
  progress, the time left once there is an estimate, and Cancel; below it the
  history, newest first.
- An interrupted export is offered in a banner at launch.
- When an export ends while the window is not focused, the window flashes in
  the taskbar.

## Tested

`crates/blinkify-engine/tests/export_queue.rs`:

| Behaviour                                                          | How                                                               |
| ------------------------------------------------------------------ | ----------------------------------------------------------------- |
| jobs run in order, never two at once                               | a scripted runner records its peak concurrency                    |
| a cancelled queued job never starts; a running one stops           | a runner held until cancelled                                     |
| progress never moves back; the stage never returns                 | out-of-order reports                                              |
| editing does not change a running export                           | the project is changed after submitting; the runner sees the copy |
| a crash leaves jobs interrupted; restart and discard work          | the queue is dropped without shutting down, then reopened         |
| the queue answers during a real export                             | `jobs()` polled while a reversed clip renders: under 100 ms       |
| cancelling a real export leaves no file, no partial and no process | the orchestrator's count and the process table (child `ff*.exe`)  |
| the source is untouched                                            | its SHA-256 before and after                                      |
| a source removed, or the target's folder removed                   | the job fails; nothing is left                                    |

A disk that fills mid-export is not simulated: it reaches the queue as the
executor's I/O error, the same path as the removed folder.
