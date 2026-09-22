import * as RadixSwitch from "@radix-ui/react-switch";
import { useId } from "react";
import { classNames } from "./classNames.js";

/**
 * A switch: an on and an off that take effect immediately.
 *
 * Not a checkbox. A checkbox is a value that is committed later, with the rest
 * of a form; a switch is a thing that is now on. In Blinkify that means
 * snapping, track lock, mute, solo — settings whose effect the user sees in the
 * timeline the instant they change them. Using a switch for something that only
 * applies when a dialog is confirmed teaches the user to distrust both.
 *
 * The label is required and is wired by `htmlFor`, so clicking the words works
 * — a switch whose label is inert is a 16-pixel target beside twelve characters
 * of text that look like they should do something.
 */

export interface SwitchProps {
  readonly label: string;
  readonly checked: boolean;
  readonly onCheckedChange: (checked: boolean) => void;
  readonly disabled?: boolean;
  /** Hide the visible label, keeping the accessible name. For a dense toolbar. */
  readonly hideLabel?: boolean;
  readonly className?: string;
}

export function Switch({
  label,
  checked,
  onCheckedChange,
  disabled = false,
  hideLabel = false,
  className,
}: SwitchProps) {
  const id = useId();

  return (
    <div className={classNames("bk-field", className)}>
      <RadixSwitch.Root
        id={id}
        className="bk-switch"
        checked={checked}
        onCheckedChange={onCheckedChange}
        disabled={disabled}
        {...(hideLabel ? { "aria-label": label } : {})}
      >
        <RadixSwitch.Thumb className="bk-switch__thumb" />
      </RadixSwitch.Root>
      {hideLabel ? null : (
        <label className="bk-field__label" htmlFor={id}>
          {label}
        </label>
      )}
    </div>
  );
}
