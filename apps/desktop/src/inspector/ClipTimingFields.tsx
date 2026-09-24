import type { Edit, Rational } from "@blinkify/types";
import { NumberInput } from "@blinkify/ui";
import type { Placement } from "../timeline/draw.js";
import { useProjectStore } from "../project/project.store.js";

/**
 * A clip's in point, out point and duration, typed (#34). Shared with the
 * clip inspector (#56), not reimplemented there.
 *
 * Numbers are frames at the sequence's rate: the in and out points in the
 * source's own time, the duration on the timeline. Typing a value sends the
 * same edit a handle drag commits — a trim of one edge by a number of
 * frames — so typing and dragging cannot produce different graphs, and the
 * engine bounds both by the source in the same way.
 */

/** Source frames at the sequence rate for `ticks` of the placement's source. */
export function sourceFrames(placement: Placement, ticks: number): number {
  const tb = placement.timeBase;
  const seq = placement.sequenceTimeBase;
  return (ticks * tb.num * seq.den) / (tb.den * seq.num);
}

const ratio = (r: Rational) => r.num / r.den;

/** The in point, out point and duration shown for a placement, in frames. */
export function timingOf(placement: Placement): {
  in: number;
  out: number;
  duration: number;
} {
  return {
    in: Math.round(sourceFrames(placement, placement.sourceIn)),
    out: Math.round(sourceFrames(placement, placement.sourceOut)),
    duration: placement.length,
  };
}

/** The edit that makes `field` of `placement` read `value`. */
export function timingEdit(
  placement: Placement,
  field: "in" | "out" | "duration",
  value: number,
): Edit | null {
  const now = timingOf(placement);
  const speed = ratio(placement.speed);
  // Source frames play at the clip's speed: a change of n source frames is
  // n / speed frames of the timeline.
  const frames =
    field === "duration"
      ? value - now.duration
      : Math.round((value - now[field]) / speed);
  if (frames === 0) return null;
  return {
    edit: "trim-edge",
    clip: placement.clip,
    edge: field === "in" ? "start" : "end",
    frames,
    ripple: false,
  };
}

export function ClipTimingFields({ placement }: { placement: Placement }) {
  const edit = useProjectStore((state) => state.edit);
  const timing = timingOf(placement);
  const send = (field: "in" | "out" | "duration", value: number) => {
    const change = timingEdit(placement, field, value);
    if (change) void edit(change);
  };
  return (
    <div className="clip-timing" role="group" aria-label="Clip timing">
      <NumberInput
        label="In"
        value={timing.in}
        step={1}
        unit="f"
        onValueChange={(value) => send("in", value)}
      />
      <NumberInput
        label="Out"
        value={timing.out}
        step={1}
        unit="f"
        onValueChange={(value) => send("out", value)}
      />
      <NumberInput
        label="Duration"
        value={timing.duration}
        min={1}
        step={1}
        unit="f"
        onValueChange={(value) => send("duration", value)}
      />
    </div>
  );
}
