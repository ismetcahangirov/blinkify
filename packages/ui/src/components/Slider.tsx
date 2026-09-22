import * as RadixSlider from "@radix-ui/react-slider";
import { classNames } from "./classNames.js";

/**
 * One slider, for four jobs with nothing in common but their shape.
 *
 * #17 names them: volume, denoise strength, speed and timeline zoom. Their
 * ranges differ by more than an order of magnitude and their steps are not even
 * the same kind of number — volume is decibels around a meaningful zero, speed
 * is a multiplier, zoom is exponential in pixels-per-second.
 *
 * So the component knows none of that. It takes a range, a step and a
 * formatter, and the caller owns the meaning:
 *
 *   volume   min -60  max 12   step 0.5  format (v) => `${v.toFixed(1)} dB`
 *   denoise  min 0    max 100  step 1    format (v) => `${v}%`
 *   speed    min 0.25 max 4    step 0.05 format (v) => `${v.toFixed(2)}x`
 *
 * Designing for that spread now is the difference between one slider and four
 * near-identical ones, which is what happens when the first caller's units leak
 * into the primitive.
 *
 * ── Keyboard ───────────────────────────────────────────────────────────────
 *
 * Radix supplies it: arrows step, Page Up and Page Down jump, Home and End go
 * to the ends. Nothing here re-implements any of that, which is the reason to
 * use Radix rather than style an `<input type="range">`.
 *
 * ── The value readout ──────────────────────────────────────────────────────
 *
 * Monospaced, tabular, and at a reserved width. The user is dragging precisely
 * because they are watching that number, and a number that changes width as it
 * changes value moves everything to its right while they do it.
 */

export interface SliderProps {
  /** The accessible name. Required — a slider announced as "slider" is unusable. */
  readonly label: string;
  readonly value: number;
  readonly onValueChange: (value: number) => void;
  /** Called once when the drag ends, for a caller that should not commit on every frame. */
  readonly onValueCommit?: (value: number) => void;
  readonly min: number;
  readonly max: number;
  readonly step?: number;
  readonly disabled?: boolean;
  readonly orientation?: "horizontal" | "vertical";
  /**
   * Turns the value into what the user reads, units and all. Omit it and no
   * readout is shown — correct for a timeline zoom, where the number is
   * meaningless and the timeline itself is the feedback.
   */
  readonly formatValue?: (value: number) => string;
  readonly className?: string;
}

export function Slider({
  label,
  value,
  onValueChange,
  onValueCommit,
  min,
  max,
  step = 1,
  disabled = false,
  orientation = "horizontal",
  formatValue,
  className,
}: SliderProps) {
  const handleChange = (next: number[]): void => {
    const [first] = next;
    if (first !== undefined) onValueChange(first);
  };

  return (
    <div className={classNames("bk-slider", className)}>
      <RadixSlider.Root
        className="bk-slider__root"
        value={[value]}
        onValueChange={handleChange}
        {...(onValueCommit === undefined
          ? {}
          : {
              onValueCommit: (next: number[]) => {
                const [first] = next;
                if (first !== undefined) onValueCommit(first);
              },
            })}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        orientation={orientation}
      >
        <RadixSlider.Track className="bk-slider__track">
          <RadixSlider.Range className="bk-slider__range" />
        </RadixSlider.Track>
        {/* The name goes on the thumb, not on the root. Radix puts
            `role="slider"` on the thumb, so a label on the root names a group
            and leaves the slider itself announced as "slider" — which the story
            accessibility suite caught, and which no amount of looking at the
            screen would have.

            `aria-valuetext` is read in place of the raw number. Without it a
            volume slider announces "-6" rather than "-6.0 dB", and the unit is
            the half that carries the meaning. */}
        <RadixSlider.Thumb
          className="bk-slider__thumb"
          aria-label={label}
          {...(formatValue === undefined
            ? {}
            : { "aria-valuetext": formatValue(value) })}
        />
      </RadixSlider.Root>
      {formatValue === undefined ? null : (
        <output className="bk-slider__value">{formatValue(value)}</output>
      )}
    </div>
  );
}
