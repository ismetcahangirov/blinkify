import type { ComponentPropsWithRef, ReactNode } from "react";
import { Button, type ButtonVariant, type ControlSize } from "./Button.js";
import { classNames } from "./classNames.js";
import { Tooltip } from "./Tooltip.js";

/**
 * A button whose entire content is a glyph.
 *
 * ── Why `label` is required ─────────────────────────────────────────────────
 *
 * An icon button with no accessible name is a button that a screen reader
 * announces as "button", and a timeline toolbar of those is a toolbar with
 * fourteen controls called "button". That is the single most common
 * accessibility defect in an editing interface, and it is not caught by a
 * linter, a contrast table or a human looking at the screen — everything looks
 * fine.
 *
 * So the label is a required prop rather than an optional one, and it does two
 * jobs at once: it becomes the `aria-label`, and it becomes the tooltip. One
 * source, so the sighted user and the screen-reader user are told the same
 * thing. A tooltip that says "Split" over a button labelled "Cut" is its own
 * kind of bug, and it is only possible if the two are typed separately.
 *
 * The tooltip is not optional either. It is how a sighted user learns what the
 * glyph means, and an icon nobody can name is decoration.
 */

export interface IconButtonProps extends Omit<
  ComponentPropsWithRef<"button">,
  "children" | "aria-label"
> {
  /** What the control does. Becomes both the accessible name and the tooltip. */
  readonly label: string;
  /** The keyboard shortcut, shown in the tooltip. */
  readonly shortcut?: string;
  readonly variant?: ButtonVariant;
  readonly size?: ControlSize;
  readonly loading?: boolean;
  /** The glyph. Marked `aria-hidden` here, because `label` is the name. */
  readonly icon: ReactNode;
}

export function IconButton({
  label,
  shortcut,
  icon,
  className,
  variant = "ghost",
  ...rest
}: IconButtonProps) {
  return (
    <Tooltip label={label} {...(shortcut === undefined ? {} : { shortcut })}>
      <Button
        aria-label={label}
        variant={variant}
        className={classNames("bk-icon-button", className)}
        {...rest}
      >
        <span aria-hidden="true">{icon}</span>
      </Button>
    </Tooltip>
  );
}
