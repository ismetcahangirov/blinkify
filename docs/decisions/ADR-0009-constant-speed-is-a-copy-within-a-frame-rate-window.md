# ADR-0009 — A constant speed is a copy while its frame rate is one a file can carry

- **Status**: Accepted
- **Date**: 2026-09-25
- **Context issue**: [#56](https://github.com/ismetcahangirov/blinkify/issues/56)

## Context

A constant speed change can be exported without touching a pixel: the video
packets are copied and only their timestamps are rescaled (#42). That makes
speed the place, on the video side, where the product's premise is visible
while the user is working — the clip inspector (#56) has to say whether the
speed being chosen is lossless before the user commits to an export.

Two things can stop a rescale from producing a valid file. A speed-up
multiplies the source's frame rate: 4× of 120 fps footage is 480 fps, which
no container Blinkify writes accepts. A slowdown divides it: a tenth of 5 fps
is half a frame a second. And a slowdown far enough below normal plays as a
slideshow, because Blinkify does not interpolate frames.

The inspector, the export planner (#39) and the export report (#50) all need
the same answer for the same graph. If each works it out, they disagree.

## Decision

1. **One rule.** `project::speed::verdict(placement, geometry)` decides what a
   clip's speed does to its pictures: the output frame rate (the source's
   nominal rate times the speed, exactly), whether the source rate varies,
   the tier, and any problem. `Document::speed_verdicts` evaluates it for
   every video clip and the shell sends the result in every `ProjectView` as
   `speeds`. The inspector phrases it and never computes a tier; the planner
   (#39) must read the same function.
2. **The copy window is the sequence frame-rate range, 1–240 fps.** A speed
   whose output rate is inside `FRAME_RATE_RANGE` — the same constant that
   bounds sequence settings (ADR-0008) — is a stream copy. Outside it the
   clip is re-encoded at the sequence rate with the recorded reason
   `speed-frame-rate-outside-container`, and the inspector raises the warning
   while the speed is being chosen, not at export.
3. **Below 12 fps is said to stutter.** The copy is still lossless, so it
   stays tier 1; the inspector says that every recorded frame is kept and no
   new ones are invented. Re-encoding would not help: without interpolation
   it only repeats the same frames.
4. **Variable frame rate is rescaled per packet.** The verdict carries the
   flag; the output rate shown is nominal and marked variable.
5. **The speed range is 0.1× to 100×** (`SPEED_RANGE`), the range a CapCut
   user already knows. The engine refuses a `set-speed` edit outside it, so no
   entry point can store one.
6. **Speeds are on a hundredths grid.** The slider moves in powers of two
   (so 0.5×, 1× and 2× are evenly spaced) and the typed field in factors, and
   both send `speedRatio(factor)`: the factor rounded to a hundredth, in
   lowest terms. For a given factor the two cannot produce different graphs.
7. **A drag is one undo entry.** The slider and the field's label scrub open
   one gesture (#37) on the first change and close it on release; the typed
   field closes it on Enter or blur. Changes inside a gesture are applied
   live, one at a time, latest first.

## Alternatives rejected

- **The renderer derives the verdict from the source rate and the factor.**
  Two lines of arithmetic, but it is a tier decision in the renderer, which
  `CLAUDE.md` section 2 forbids, and the planner would need its own copy.
- **A tighter ceiling for device playback (for example 120 fps).** A phone
  plays 240 fps footage it recorded, and the container accepts it. A ceiling
  below the container's would re-encode files that play; a lower "device"
  profile, if needed, belongs to the export presets.
- **Re-encode below the smooth-motion floor.** It spends a generation for no
  visible gain without frame interpolation, which Blinkify does not have.
- **A linear slider.** From 0.1× to 100× it gives everything below 2× — nearly
  every real edit — a few pixels.
- **Commit a speed on every slider move.** Hundreds of undo entries per drag;
  #37 requires the gesture.
- **Unbounded speeds.** A ratio of 1/10000 is a clip ten thousand times longer
  than its source; nothing useful lies outside CapCut's range.

## Consequences

- The planner (#39) takes `SpeedVerdict.tier` as the speed's contribution to
  a segment's tier and asserts it never disagrees with the inspector's.
- #42 implements the rescale behind tier 1 and the re-timed fallback behind
  `speed-frame-rate-outside-container`; the window is decided here.
- The multi-clip mixed state (`inspector/mixedValue.ts`) is the rule the audio
  inspector (#49) uses too.
