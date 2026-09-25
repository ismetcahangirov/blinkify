import { useEffect } from "react";
import { HANDLERS } from "./handlers.js";
import { ACTIONS, actionFor, chordOf, type ActionId } from "./shortcuts.js";

/**
 * The one keyboard listener (#38), mounted once by the application.
 *
 * Context decides whether a key is a shortcut at all:
 *
 * - **Typing wins.** In a text field every key is the field's: a Space that
 *   starts playback while a project is being named is the classic bug.
 * - **A control keeps its own keys.** A slider, list, menu or tab list uses
 *   the arrows, Home, End and Space itself.
 * - **A dialog is modal.** Nothing behind it reacts to the keyboard.
 * - **Space plays even from a focused button.** In an editor Space means
 *   play, whatever was clicked last; the key press is taken before the
 *   button can turn it into a click.
 */

/** Whether the key press belongs to a text field rather than the editor. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag !== "INPUT") return false;
  const type = (target as HTMLInputElement).type;
  return !["checkbox", "radio", "range", "button", "submit"].includes(type);
}

/** Keys a focused control uses for itself. */
const CONTROL_KEYS = new Set([
  "Space",
  "ArrowLeft",
  "ArrowRight",
  "ArrowUp",
  "ArrowDown",
  "Home",
  "End",
]);

const OWNING_ROLES = new Set([
  "slider",
  "listbox",
  "option",
  "combobox",
  "menu",
  "menuitem",
  "menuitemcheckbox",
  "menuitemradio",
  "tab",
  "tablist",
  "radio",
  "separator",
  "spinbutton",
]);

function ownsKey(target: EventTarget | null, chord: string): boolean {
  if (!CONTROL_KEYS.has(chord) || !(target instanceof HTMLElement))
    return false;
  const role = target.getAttribute("role");
  return role !== null && OWNING_ROLES.has(role);
}

function inDialog(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    target.closest('[role="dialog"], [role="alertdialog"]') !== null
  );
}

/** The action a key press should run here, or `null` to leave it alone. */
export function shortcutFor(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  target: EventTarget | null;
}): ActionId | null {
  const chord = chordOf(event);
  const action = actionFor(chord);
  if (action === null || chord === null) return null;
  if (isTyping(event.target) || inDialog(event.target)) return null;
  if (ownsKey(event.target, chord)) return null;
  return action;
}

/** Run the shortcut a key press is, if it is one. */
export function dispatch(event: KeyboardEvent): void {
  if (event.defaultPrevented) return;
  const action = shortcutFor(event);
  if (!action) return;
  if (event.repeat && !ACTIONS[action].repeats) {
    event.preventDefault();
    return;
  }
  if (HANDLERS[action]()) event.preventDefault();
}

export function useShortcuts(): void {
  useEffect(() => {
    window.addEventListener("keydown", dispatch);
    return () => window.removeEventListener("keydown", dispatch);
  }, []);
}
