# Roadmap

Blinkify is planned as eight Epics, plus one that is deliberately held back
until the core proposition is proved.

GitHub is the source of truth for scope and status. This document records the
**order** and **why that order** — it does not restate the issues.

## Order

Each Epic links to its GitHub issue. Blockers are the Epic's own recorded
dependencies.

| #   | Epic                                                                                                                                   | Blocked by    | Status      |
| --- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------- | ----------- |
| 1   | [Repository foundation, engineering rules and Windows CI](https://github.com/ismetcahangirov/blinkify/issues/1)                        | —             | In progress |
| 2   | [Blinkify design system and the CapCut-structured application shell](https://github.com/ismetcahangirov/blinkify/issues/2)             | 1             | Not started |
| 3   | [Media engine foundation: FFmpeg sidecar, probe, keyframe index and caches](https://github.com/ismetcahangirov/blinkify/issues/3)      | 1             | Not started |
| 4   | [Preview player: frame-accurate playback, scrubbing and live edit-graph preview](https://github.com/ismetcahangirov/blinkify/issues/4) | 1, 2, 3       | Not started |
| 5   | [Timeline: canvas renderer and the non-destructive edit graph](https://github.com/ismetcahangirov/blinkify/issues/5)                   | 1, 2, 3, 4    | Not started |
| 6   | [Lossless export core: stream copy, smart-cut and proof of losslessness](https://github.com/ismetcahangirov/blinkify/issues/6)         | 1, 3, 4, 5    | Not started |
| 7   | [Audio processing: gain, RNNoise reduction and R128 normalisation](https://github.com/ismetcahangirov/blinkify/issues/7)               | 1, 2, 3, 4, 6 | Not started |
| 8   | [Export UI, background job queue and the honest lossless report](https://github.com/ismetcahangirov/blinkify/issues/8)                 | 1, 2, 3, 5, 6 | Not started |
| 9   | Crop and reframe — **not yet opened**, see below                                                                                       | 6 proved      | Deferred    |

## Why this order

**Epic 1 first, alone.** Nothing can be verified before there is something that
runs a gate. Every other Epic depends on it.

**Epics 2 and 3 run in parallel.** The design system and the media engine share
no code and block each other in neither direction. Two independent fronts out of
one foundation is the only place in this plan where real parallelism exists.

**Epic 4 before 5, but they interleave.** The player needs the shell and the
engine. The shared edit-graph evaluator ([#30](https://github.com/ismetcahangirov/blinkify/issues/30))
additionally needs the edit-graph model
([#32](https://github.com/ismetcahangirov/blinkify/issues/32)) from Epic 5, so
#32 is scheduled ahead of #30 rather than Epic 4 waiting on Epic 5 as a whole.
This reads like a cycle at Epic granularity and is not one at issue granularity.
Epic 4's issue body records this explicitly; read it before rescheduling either.

**Epic 6 is the project.** Everything before it is scaffolding for it, and
everything after it depends on its proof. Epic 2 is deliberately _not_ a blocker
— every sub-issue in Epic 6 is engine or test code with no interface — so the
lossless core can be proved before the export UI exists to show it off.

**Epics 7 and 8 run in parallel after 6.** Audio processing and the export
interface touch different code. Epic 8 does not depend on Epic 7; the export
dialog reports on whatever plan the planner produced, whether or not an audio
filter is in it.

## The milestone that matters

> Epic 6 closing with
> [#45](https://github.com/ismetcahangirov/blinkify/issues/45) green — the
> losslessness suite passing against the media corpus — is the point at which
> Blinkify's premise stops being a claim and becomes a measured property.

Nothing shipped before that milestone proves anything about the product. Plan
accordingly: an interface that looks finished over an unproven core is the
failure mode to avoid.

## Epic 9 — crop and reframe, deferred deliberately

Crop is the most requested capability that **cannot** be lossless. Changing the
frame rectangle forces every frame through tier 3 — decode, filter, re-encode —
which is exactly the pipeline Blinkify exists to avoid.

It is deferred, not dropped. It ships as **Epic 9, after Epic 6 is closed and
the losslessness suite is green.** The order is the point:

- Building the re-encode path first would make it the path of least resistance,
  and the lossless core would end up bolted onto a re-encoding editor rather
  than the other way round.
- The tier-3 executor
  ([#55](https://github.com/ismetcahangirov/blinkify/issues/55)) that crop needs
  is already in Epic 6, built to serve segments that genuinely cannot be copied.
  Crop is a consumer of that executor, not a reason to build a second one.
- Once losslessness is a measured property, crop can be offered honestly:
  clearly marked as the operation that costs quality, with the reason shown
  before the export runs, per `CLAUDE.md` section 17.

Epic 9 is not opened as an issue until Epic 6 closes, so that it cannot be
started early.

## Out of scope for v1

Recorded here so they are declined once rather than reconsidered repeatedly:

- **macOS and Linux builds.** Windows first, deliberately. Revisit after v1.
- **Code signing.** The v1 installer is unsigned and SmartScreen will warn. The
  certificate is tracked as its own issue, not worked around.
- **Compositing, colour grading and motion graphics.** See
  [`../product/README.md`](../product/README.md) — these are standing
  non-goals, not backlog.
