import * as RadixTooltip from "@radix-ui/react-tooltip";
import type { ReactNode } from "react";

/**
 * Tooltip, and the provider that gives a toolbar its group timing.
 *
 * ── The two requirements, and why they do not actually conflict ─────────────
 *
 * #17 asks for both of these:
 *
 *   "the first is delayed, subsequent ones in the same group are immediate"
 *   "moving the pointer quickly across a row of icon buttons does not flicker"
 *
 * Read quickly they look opposed — instant tooltips are exactly what flickers.
 * They are reconciled by when the skip window starts: it opens only after a
 * tooltip has been shown and dismissed. A pointer sweeping across a cold
 * toolbar never rests on anything for `TOOLTIP_DELAY_MS`, so nothing opens, so
 * no skip window ever starts and nothing flickers.
 *
 * Once the user has deliberately rested on one control and read its tooltip,
 * they are exploring, and the neighbours answer immediately. That is the
 * behaviour the first requirement is describing.
 */

/**
 * How long the pointer must rest before the first tooltip appears.
 *
 * 500ms is the width of the gap between "the pointer is passing over this" and
 * "the pointer has stopped on this". Shorter and a sweep across a toolbar
 * starts opening things; much longer and the user who did stop has to wonder
 * whether tooltips exist.
 */
export const TOOLTIP_DELAY_MS = 500;

/**
 * How long after one tooltip closes the next opens with no delay.
 *
 * This is the "group" in group delay. It is deliberately short: it should cover
 * moving to the button next door, not a change of mind three seconds later.
 */
export const TOOLTIP_SKIP_DELAY_MS = 300;

export interface TooltipProviderProps {
  readonly children: ReactNode;
  readonly delayMs?: number;
  readonly skipDelayMs?: number;
}

/**
 * Wrap the application once. Every tooltip beneath shares one delay clock,
 * which is what makes them a group rather than a row of independent timers.
 */
export function TooltipProvider({
  children,
  delayMs = TOOLTIP_DELAY_MS,
  skipDelayMs = TOOLTIP_SKIP_DELAY_MS,
}: TooltipProviderProps) {
  return (
    <RadixTooltip.Provider
      delayDuration={delayMs}
      skipDelayDuration={skipDelayMs}
    >
      {children}
    </RadixTooltip.Provider>
  );
}

/**
 * The gap between a floating surface and the control it belongs to, in pixels.
 *
 * Radix positions in JavaScript and takes a number, so this one measurement
 * cannot be a CSS token. It is `--space-1` written out, and it is a named
 * constant rather than a literal at four call sites so that the four cannot
 * drift apart.
 */
export const FLOATING_OFFSET_PX = 4;

export interface TooltipProps {
  /** What the control does. A noun phrase or a short verb phrase, never a sentence. */
  readonly label: ReactNode;
  /**
   * The keyboard shortcut, shown dimmer and to the right. Second-order
   * information: the user opening a tooltip wanted to know what the control
   * does, and learns the shortcut as a side effect.
   */
  readonly shortcut?: string;
  readonly side?: RadixTooltip.TooltipContentProps["side"];
  readonly children: ReactNode;
}

export function Tooltip({
  label,
  shortcut,
  side = "bottom",
  children,
}: TooltipProps) {
  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          className="bk-tooltip"
          side={side}
          sideOffset={FLOATING_OFFSET_PX}
        >
          {label}
          {shortcut === undefined ? null : (
            <span className="bk-tooltip__shortcut">{shortcut}</span>
          )}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}
