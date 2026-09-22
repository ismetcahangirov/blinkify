import * as RadixPopover from "@radix-ui/react-popover";
import type { ReactNode } from "react";
import { classNames } from "./classNames.js";
import { FLOATING_OFFSET_PX } from "./Tooltip.js";

/**
 * A popover: a small surface of controls anchored to the control that opened
 * it.
 *
 * Not a tooltip and not a dialog, and the difference is what the user can do
 * with it. A tooltip says something and cannot be interacted with. A dialog
 * takes the window and has to be dismissed. A popover holds controls, keeps the
 * interface behind it live, and closes when the user looks away — the colour
 * picker, the speed control, the zoom presets.
 *
 * Radix handles the parts that are easy to get wrong: focus moves in on open
 * and back to the trigger on close, Escape closes, a click outside closes, and
 * the content is portalled so it cannot be clipped by a panel's `overflow`.
 * That last one is the reason a hand-rolled popover fails in exactly this
 * layout, where every zone is a scroll container.
 */

export interface PopoverProps {
  /** The control the popover belongs to. Receives the open state from Radix. */
  readonly trigger: ReactNode;
  readonly children: ReactNode;
  readonly side?: RadixPopover.PopoverContentProps["side"];
  readonly align?: RadixPopover.PopoverContentProps["align"];
  readonly open?: boolean;
  readonly onOpenChange?: (open: boolean) => void;
  readonly className?: string;
}

export function Popover({
  trigger,
  children,
  side = "bottom",
  align = "start",
  open,
  onOpenChange,
  className,
}: PopoverProps) {
  return (
    <RadixPopover.Root
      {...(open === undefined ? {} : { open })}
      {...(onOpenChange === undefined ? {} : { onOpenChange })}
    >
      <RadixPopover.Trigger asChild>{trigger}</RadixPopover.Trigger>
      <RadixPopover.Portal>
        <RadixPopover.Content
          className={classNames("bk-surface", "bk-popover", className)}
          side={side}
          align={align}
          sideOffset={FLOATING_OFFSET_PX}
        >
          {children}
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
