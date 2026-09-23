import type { TransportCommand } from "@blinkify/types";
import { useEffect } from "react";

/**
 * The transport's keys, as `docs/design/keyboard-shortcuts.md` documents them.
 * CapCut's where CapCut has one: Space plays and pauses, the arrow keys step a
 * frame. The full map is #38's.
 */
export const TRANSPORT_SHORTCUTS = {
  playPause: "Space",
  previousFrame: "←",
  nextFrame: "→",
  jumpToStart: "Home",
  jumpToEnd: "End",
} as const;

/** The command a key press means, or `null` for a key the transport ignores. */
export function commandForKey(event: {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}): TransportCommand | null {
  if (event.ctrlKey || event.altKey || event.metaKey) return null;
  switch (event.key) {
    case " ":
      return { type: "toggle" };
    case "ArrowLeft":
      return { type: "step", frames: -1 };
    case "ArrowRight":
      return { type: "step", frames: 1 };
    case "Home":
      return { type: "jump-to-start" };
    case "End":
      return { type: "jump-to-end" };
    default:
      return null;
  }
}

/** Whether a key press belongs to the control it happened in. */
function typingInto(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target instanceof HTMLTextAreaElement) return true;
  if (target instanceof HTMLInputElement) return true;
  // A slider or a list box uses the arrows and Home/End itself.
  const role = target.getAttribute("role");
  return (
    role === "slider" ||
    role === "listbox" ||
    role === "option" ||
    role === "combobox"
  );
}

/**
 * Drive the transport from the keyboard while `enabled`.
 *
 * Space is taken even from a focused button — in an editor Space means play,
 * whatever was last clicked — by cancelling the key press before the button
 * turns it into a click.
 */
export function useTransportShortcuts(
  enabled: boolean,
  send: (command: TransportCommand) => void,
): void {
  useEffect(() => {
    if (!enabled) return undefined;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || typingInto(event.target)) return;
      const command = commandForKey(event);
      if (!command) return;
      event.preventDefault();
      if (event.repeat && command.type === "toggle") return;
      send(command);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [enabled, send]);
}
