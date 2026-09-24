import { IconButton, Popover } from "@blinkify/ui";
import { useEffect } from "react";
import { useProjectStore } from "./project.store.js";

/**
 * Undo, redo and the history list (#37), in the app bar where the layout
 * reference puts them.
 *
 * The list is the engine's: every entry is an edit the document applied, in
 * order, and the ones past `applied` can be redone. Choosing an entry steps
 * the history to just after it — the same undo and redo the buttons send, so
 * there is no second way to move through it.
 */

export const HISTORY_SHORTCUTS = {
  undo: "Ctrl+Z",
  redo: "Ctrl+Y",
  redoAlternative: "Ctrl+Shift+Z",
} as const;

/** Whether a key press is undo, redo, or neither. */
export function historyActionForKey(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): "undo" | "redo" | null {
  if (!(event.ctrlKey || event.metaKey) || event.altKey) return null;
  const key = event.key.toLowerCase();
  if (key === "z") return event.shiftKey ? "redo" : "undo";
  if (key === "y" && !event.shiftKey) return "redo";
  return null;
}

/** Whether the key press belongs to a text field rather than the editor. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === "TEXTAREA") return true;
  if (tag !== "INPUT") return false;
  const type = (target as HTMLInputElement).type;
  return !["checkbox", "radio", "range", "button", "submit"].includes(type);
}

function useHistoryShortcuts(): void {
  const undo = useProjectStore((state) => state.undo);
  const redo = useProjectStore((state) => state.redo);
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      // A text field has its own undo, and it is the one the user means.
      if (isTyping(event.target)) return;
      const action = historyActionForKey(event);
      if (!action) return;
      event.preventDefault();
      void (action === "undo" ? undo() : redo());
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [undo, redo]);
}

function UndoGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M6 4.5L3 7.5l3 3M3.5 7.5H10a3 3 0 0 1 0 6H8"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function RedoGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M10 4.5l3 3-3 3M12.5 7.5H6a3 3 0 0 0 0 6h2"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function HistoryGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3 4.5h10M3 8h10M3 11.5h6"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

export function HistoryControls() {
  const history = useProjectStore((state) => state.view?.history ?? null);
  const undo = useProjectStore((state) => state.undo);
  const redo = useProjectStore((state) => state.redo);
  useHistoryShortcuts();

  const applied = history?.applied ?? 0;
  const entries = history?.entries ?? [];

  /** Step until `target` entries are applied. */
  const stepTo = async (target: number): Promise<void> => {
    let at = applied;
    while (at > target) {
      await undo();
      at -= 1;
    }
    while (at < target) {
      await redo();
      at += 1;
    }
  };

  return (
    <div className="shell__history">
      <IconButton
        label={applied > 0 ? `Undo ${entries[applied - 1] ?? ""}` : "Undo"}
        shortcut={HISTORY_SHORTCUTS.undo}
        icon={<UndoGlyph />}
        disabled={applied === 0}
        onClick={() => void undo()}
      />
      <IconButton
        label={
          applied < entries.length ? `Redo ${entries[applied] ?? ""}` : "Redo"
        }
        shortcut={HISTORY_SHORTCUTS.redo}
        icon={<RedoGlyph />}
        disabled={applied >= entries.length}
        onClick={() => void redo()}
      />
      <Popover
        side="bottom"
        align="start"
        trigger={
          <IconButton
            label="History"
            icon={<HistoryGlyph />}
            disabled={history === null}
          />
        }
      >
        <ol className="history-list" aria-label="Edit history">
          <li>
            <button
              type="button"
              className="history-list__entry"
              aria-current={applied === 0 ? "step" : undefined}
              onClick={() => void stepTo(0)}
            >
              Opened project
            </button>
          </li>
          {entries.map((label, index) => (
            <li key={index}>
              <button
                type="button"
                className="history-list__entry"
                data-state={index < applied ? "applied" : "undone"}
                aria-current={index + 1 === applied ? "step" : undefined}
                onClick={() => void stepTo(index + 1)}
              >
                {label}
              </button>
            </li>
          ))}
        </ol>
      </Popover>
    </div>
  );
}
