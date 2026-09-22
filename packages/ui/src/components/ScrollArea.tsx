import * as RadixScrollArea from "@radix-ui/react-scroll-area";
import type { ReactNode } from "react";
import { classNames } from "./classNames.js";

/**
 * A scrolling region with a scrollbar that belongs to the interface.
 *
 * Windows draws a 17px light scrollbar. In a 240px library panel that is seven
 * per cent of the width spent on a control nobody looks at, and on a near-black
 * interface it is a bright stripe down the edge of every panel.
 *
 * ── What is not given up to get that ───────────────────────────────────────
 *
 * A custom scrollbar is usually a downgrade, because the usual way to build one
 * is `overflow: hidden` plus a div that follows a scroll offset — which breaks
 * the mouse wheel, keyboard scrolling, and the browser scrolling a focused
 * element into view. Radix keeps the element natively scrollable and only
 * replaces the visible bar, so the wheel, Page Up, Home, End and
 * `scrollIntoView` all still work because they were never intercepted.
 *
 * ── The thumb's hit area ───────────────────────────────────────────────────
 *
 * An 8px thumb is right to look at and wrong to grab. The thumb carries a
 * pseudo-element grown to 24px — WCAG 2.2 §2.5.8 — which widens what the
 * pointer can hit without widening what the eye sees.
 */

export interface ScrollAreaProps {
  readonly children: ReactNode;
  readonly orientation?: "vertical" | "horizontal" | "both";
  readonly className?: string;
}

export function ScrollArea({
  children,
  orientation = "vertical",
  className,
}: ScrollAreaProps) {
  return (
    <RadixScrollArea.Root
      className={classNames("bk-scroll-area", className)}
      /* `hover` rather than `always`: the bar appears when the pointer is in
         the region and stays out of the way otherwise. `scroll` would hide it
         from a user who has not scrolled yet and therefore does not know there
         is more. */
      type="hover"
    >
      <RadixScrollArea.Viewport className="bk-scroll-area__viewport">
        {children}
      </RadixScrollArea.Viewport>
      {orientation === "vertical" || orientation === "both" ? (
        <RadixScrollArea.Scrollbar
          className="bk-scroll-area__scrollbar"
          orientation="vertical"
        >
          <RadixScrollArea.Thumb className="bk-scroll-area__thumb" />
        </RadixScrollArea.Scrollbar>
      ) : null}
      {orientation === "horizontal" || orientation === "both" ? (
        <RadixScrollArea.Scrollbar
          className="bk-scroll-area__scrollbar"
          orientation="horizontal"
        >
          <RadixScrollArea.Thumb className="bk-scroll-area__thumb" />
        </RadixScrollArea.Scrollbar>
      ) : null}
      <RadixScrollArea.Corner />
    </RadixScrollArea.Root>
  );
}
