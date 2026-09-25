import type { Edit, Rational } from "@blinkify/types";
import { Button, NumberInput, Slider } from "@blinkify/ui";
import { useEffect, useRef, useState } from "react";
import { formatTimecode } from "../player/timecode.js";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { editGesture, type EditGesture } from "./editGesture.js";
import { shared } from "./mixedValue.js";
import {
  MAX_SPEED,
  MIN_SPEED,
  NORMAL_SPEED,
  SLIDER_MAX,
  SLIDER_MIN,
  SLIDER_STEP,
  factorOf,
  factorToSlider,
  formatFactor,
  formatRate,
  sameRatio,
  sliderToFactor,
  speedRatio,
  speedStatement,
  speedWarnings,
} from "./speed.js";

/**
 * The speed control (#56): a constant factor, by slider and by typed number,
 * with what it does stated while the user is choosing it.
 *
 * Both inputs send `set-speed` with the ratio `speedRatio` makes of the
 * factor, so they cannot produce different graphs. A drag — of the slider,
 * or of the field's label — is one gesture and one undo entry. What the
 * speed does to the pictures is the engine's verdict for each clip
 * (`view.speeds`), which this only phrases.
 */
export function SpeedControl({ clips }: { clips: readonly Placement[] }) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const [dragged, setDragged] = useState<number | null>(null);
  const gesture = useRef<EditGesture | null>(null);
  /* A controlled Radix slider commits a key press before it reports the
     change (the scrub bar meets the same order). The commit therefore sends
     its own value and closes the gesture, and the change that follows it
     for the same position is the echo, not a new drag. */
  const committed = useRef<number | null>(null);

  const speed = shared(
    clips.map((clip) => clip.speed),
    sameRatio,
  );
  const factor = speed.kind === "same" ? factorOf(speed.value) : 1;
  const mixed = speed.kind === "mixed";
  const ids = clips.map((clip) => clip.clip);
  const setSpeed = (ratio: Rational): Edit => ({
    edit: "set-speed",
    clips: [...ids],
    ratio,
  });

  const during = (ratio: Rational) => {
    gesture.current ??= editGesture("Change speed");
    gesture.current.change(setSpeed(ratio));
  };
  const finish = () => {
    const open = gesture.current;
    gesture.current = null;
    setDragged(null);
    if (open) void open.end();
  };
  // Unmounted mid-drag, the gesture it opened is still closed.
  useEffect(
    () => () => {
      const open = gesture.current;
      gesture.current = null;
      if (open) void open.end();
    },
    [],
  );

  const verdicts = clips.flatMap((clip) => {
    const verdict = view?.speeds[clip.clip];
    return verdict ? [verdict] : [];
  });
  const statement = speedStatement(verdicts);
  const warnings = speedWarnings(verdicts);
  const rate = shared(
    verdicts.map((v) => v.outputFrameRate),
    sameRatio,
  );
  const length = shared(clips.map((clip) => clip.length));
  const sequenceRate = view?.project.sequence.settings.frameRate ?? {
    num: 30,
    den: 1,
  };
  const anyway = reencodedAnyway(clips, view?.eligibility ?? {});

  return (
    <div className="speed-control">
      <div className="speed-control__inputs">
        <Slider
          label="Speed"
          min={SLIDER_MIN}
          max={SLIDER_MAX}
          step={SLIDER_STEP}
          value={dragged ?? factorToSlider(factor)}
          onValueChange={(position) => {
            if (gesture.current === null && committed.current === position) {
              committed.current = null;
              return;
            }
            committed.current = null;
            setDragged(position);
            during(speedRatio(sliderToFactor(position)));
          }}
          onValueCommit={(position) => {
            during(speedRatio(sliderToFactor(position)));
            committed.current = position;
            finish();
          }}
          formatValue={(position) =>
            mixed && dragged === null
              ? "Mixed"
              : formatFactor(sliderToFactor(position))
          }
        />
        <div
          className="speed-control__factor"
          onBlur={finish}
          onPointerUp={finish}
          onKeyDown={(event) => {
            if (event.key === "Enter") finish();
          }}
        >
          <NumberInput
            label="Factor"
            value={factor}
            mixed={mixed}
            min={MIN_SPEED}
            max={MAX_SPEED}
            step={0.01}
            precision={2}
            unit="×"
            onValueChange={(value) => during(speedRatio(value))}
          />
          <Button
            size="sm"
            variant="ghost"
            disabled={
              speed.kind === "same" && sameRatio(speed.value, NORMAL_SPEED)
            }
            onClick={() => void edit(setSpeed(NORMAL_SPEED))}
          >
            Reset speed
          </Button>
        </div>
      </div>

      <dl className="inspector-facts">
        <dt>Duration</dt>
        <dd>
          {length.kind === "same"
            ? `${formatTimecode(length.value, sequenceRate)} (${length.value} f)`
            : "Mixed"}
        </dd>
        <dt>Output frame rate</dt>
        <dd>
          {rate.kind === "same"
            ? `${formatRate(rate.value)}${verdicts.some((v) => v.variableFrameRate) ? ", variable" : ""}`
            : rate.kind === "mixed"
              ? "Mixed"
              : "Unknown: the source is offline"}
        </dd>
      </dl>

      {statement ? (
        <p className="inspector-statement" data-tone={statement.tone}>
          {statement.text}
        </p>
      ) : null}
      {warnings.map((warning) => (
        <p key={warning} className="inspector-warning" role="alert">
          {warning}
        </p>
      ))}
      {anyway.map((reason) => (
        <p key={reason} className="inspector-note">
          {reason}
        </p>
      ))}
      {speed.kind === "same" && sameRatio(speed.value, NORMAL_SPEED) ? null : (
        <p className="inspector-note">
          The sound is resampled to the new speed. That is the audio&apos;s own
          decision and never makes the video re-encode.
        </p>
      )}
    </div>
  );
}

/**
 * What re-encodes a selected clip whatever its speed — a reverse, a hold, a
 * source that does not match the sequence — so a "lossless" speed is never
 * read as a lossless clip. Each is the engine's own record, not worked out.
 */
function reencodedAnyway(
  clips: readonly Placement[],
  eligibility: Readonly<Record<number, { readonly eligible: boolean }>>,
): string[] {
  const reasons = new Set<string>();
  for (const clip of clips) {
    if (clip.forced === "reverse")
      reasons.add("A reversed clip is re-encoded whatever its speed.");
    else if (clip.forced === "freeze-frame")
      reasons.add("A held frame is re-encoded whatever its speed.");
    if (eligibility[clip.source]?.eligible === false)
      reasons.add(
        "Its source does not match the sequence settings, so it is re-encoded whatever its speed.",
      );
  }
  return [...reasons];
}
