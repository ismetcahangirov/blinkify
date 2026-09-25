import { useId, useRef, useState, type PointerEvent } from "react";
import { classNames } from "./classNames.js";

/**
 * A numeric field whose label is a drag handle.
 *
 * ── Why the label scrubs ────────────────────────────────────────────────────
 *
 * #17: "a NumberInput in an editor needs drag-to-scrub on its label; it is
 * expected behaviour in this class of tool". It is — every editor in the
 * category has it, and a user coming from one will try it within a minute. A
 * field that does not respond to the drag reads as a field that is broken, not
 * as one that made a different choice.
 *
 * ── Three ways in, and all three are real ──────────────────────────────────
 *
 *   type      the field is an ordinary text input
 *   drag      the label scrubs, with `ew-resize` saying so
 *   keyboard  arrows step, shift-arrow steps ten times, Home and End go to the
 *             bounds
 *
 * The keyboard path is not an afterthought to satisfy an audit. The drag is a
 * pointer-only affordance, and a control whose primary interaction is
 * pointer-only is a control a keyboard user cannot reach at all.
 *
 * ── Why the field holds a string while it is being edited ──────────────────
 *
 * Typing "-" or "1." or "" are all states on the way to a number and none of
 * them parses. Committing on every keystroke means the field fights the user:
 * they type "-", it becomes 0, and the minus sign vanishes. So the draft text
 * is local state and the number is only committed when it parses.
 */

export interface NumberInputProps {
  /** The accessible name, and the visible label the user drags. */
  readonly label: string;
  readonly value: number;
  readonly onValueChange: (value: number) => void;
  readonly min?: number;
  readonly max?: number;
  readonly step?: number;
  /** Decimal places shown when not being edited. */
  readonly precision?: number;
  /** Shown after the field: `dB`, `%`, `fps`. Not part of the editable text. */
  readonly unit?: string;
  readonly disabled?: boolean;
  /**
   * The field stands for several values that differ — an inspector showing
   * a multi-clip selection. It reads "Mixed" rather than any one of them,
   * so no value is shown as if it were everyone's; typing, stepping or
   * scrubbing still commits a single value, starting from `value`.
   */
  readonly mixed?: boolean;
  readonly className?: string;
}

/** Pixels of pointer travel per step. Four is the speed every editor uses. */
const PIXELS_PER_STEP = 4;
/** Shift multiplies the step, on the keyboard and in the drag alike. */
const COARSE_MULTIPLIER = 10;

export function NumberInput({
  label,
  value,
  onValueChange,
  min = Number.NEGATIVE_INFINITY,
  max = Number.POSITIVE_INFINITY,
  step = 1,
  precision = 0,
  unit,
  disabled = false,
  mixed = false,
  className,
}: NumberInputProps) {
  const fieldId = useId();
  const [draft, setDraft] = useState<string | null>(null);
  const dragOrigin = useRef<{ x: number; value: number } | null>(null);

  const clamp = (next: number): number => Math.min(max, Math.max(min, next));

  /* Rounding to the step keeps a drag from accumulating a long tail of floating
     point — 0.1 + 0.2 arrives eventually, and a field reading 0.30000000000004
     is the kind of defect that survives to a release. */
  const quantise = (next: number): number =>
    Number.parseFloat((Math.round(next / step) * step).toFixed(10));

  const commit = (next: number): void => {
    if (!Number.isFinite(next)) return;
    onValueChange(clamp(quantise(next)));
  };

  const handlePointerDown = (event: PointerEvent<HTMLLabelElement>): void => {
    if (disabled) return;
    /* Pointer capture, so the drag survives the pointer leaving the label —
       which it does immediately, because the label is a few characters wide and
       the gesture is tens of pixels long. Without it the scrub stops the moment
       it becomes useful. */
    event.currentTarget.setPointerCapture(event.pointerId);
    dragOrigin.current = { x: event.clientX, value };
  };

  const handlePointerMove = (event: PointerEvent<HTMLLabelElement>): void => {
    const origin = dragOrigin.current;
    if (origin === null) return;
    const travelled = event.clientX - origin.x;
    const size = step * (event.shiftKey ? COARSE_MULTIPLIER : 1);
    commit(origin.value + (travelled / PIXELS_PER_STEP) * size);
  };

  const endDrag = (event: PointerEvent<HTMLLabelElement>): void => {
    if (dragOrigin.current === null) return;
    dragOrigin.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  };

  return (
    <div className={classNames("bk-number-input", className)}>
      <label
        className="bk-number-input__label"
        htmlFor={fieldId}
        data-disabled={disabled ? "true" : undefined}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        {label}
      </label>
      <input
        id={fieldId}
        className="bk-number-input__field"
        /* `text` with `inputMode="decimal"`, not `type="number"`. A number input
           scrolls its value when the wheel passes over it, which in a scrolling
           inspector panel silently edits the clip the user was scrolling past. */
        type="text"
        inputMode="decimal"
        role="spinbutton"
        aria-valuenow={value}
        {...(Number.isFinite(min) ? { "aria-valuemin": min } : {})}
        {...(Number.isFinite(max) ? { "aria-valuemax": max } : {})}
        {...(mixed
          ? { "aria-valuetext": "Mixed" }
          : unit === undefined
            ? {}
            : { "aria-valuetext": `${value.toFixed(precision)} ${unit}` })}
        disabled={disabled}
        placeholder={mixed ? "Mixed" : undefined}
        value={draft ?? (mixed ? "" : value.toFixed(precision))}
        onChange={(event) => {
          setDraft(event.target.value);
          const parsed = Number.parseFloat(event.target.value);
          if (Number.isFinite(parsed)) commit(parsed);
        }}
        onBlur={() => {
          setDraft(null);
        }}
        onKeyDown={(event) => {
          const size = step * (event.shiftKey ? COARSE_MULTIPLIER : 1);
          if (event.key === "ArrowUp") {
            event.preventDefault();
            commit(value + size);
          } else if (event.key === "ArrowDown") {
            event.preventDefault();
            commit(value - size);
          } else if (event.key === "Home" && Number.isFinite(min)) {
            event.preventDefault();
            commit(min);
          } else if (event.key === "End" && Number.isFinite(max)) {
            event.preventDefault();
            commit(max);
          } else if (event.key === "Enter") {
            setDraft(null);
          }
        }}
      />
      {unit === undefined ? null : (
        <span className="bk-number-input__unit" aria-hidden="true">
          {unit}
        </span>
      )}
    </div>
  );
}
