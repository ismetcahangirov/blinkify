import { useEffect } from "react";
import { isTyping } from "../project/HistoryControls.js";
import { useProjectStore } from "../project/project.store.js";
import { allClips } from "./interaction.js";
import { layoutRows } from "./rows.js";

/**
 * The timeline's editing keys (#34): Delete removes the selected clips,
 * Shift+Delete removes them and closes the gaps, Ctrl+A selects every clip.
 * Left to a text field when one has focus. The central registry and the
 * reference dialog are #38, which takes these over.
 */
export const TIMELINE_KEYS = {
  delete: "Delete",
  rippleDelete: "Shift+Delete",
  selectAll: "Ctrl+A",
  split: "Ctrl+B",
} as const;

export type TimelineAction =
  "delete" | "ripple-delete" | "select-all" | "split";

export function timelineActionForKey(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): TimelineAction | null {
  if (event.altKey) return null;
  const ctrl = event.ctrlKey || event.metaKey;
  if (ctrl && !event.shiftKey && event.key.toLowerCase() === "a")
    return "select-all";
  if (ctrl && !event.shiftKey && event.key.toLowerCase() === "b")
    return "split";
  if (ctrl) return null;
  if (event.key === "Delete" || event.key === "Backspace")
    return event.shiftKey ? "ripple-delete" : "delete";
  return null;
}

/** Split is the edit toolbar's: it needs the keyframe indicator's answer. */
let splitter: () => Promise<void> = () => Promise.resolve();
export function setSplitter(run: () => Promise<void>): void {
  splitter = run;
}

export function runTimelineAction(action: TimelineAction): void {
  const project = useProjectStore.getState();
  const clips = [...project.selection];
  switch (action) {
    case "select-all":
      project.select(allClips(layoutRows(project.view?.timeline)));
      return;
    case "delete":
      if (clips.length > 0) void project.edit({ edit: "remove-clips", clips });
      return;
    case "split":
      void splitter();
      return;
    case "ripple-delete":
      if (clips.length > 0) void project.edit({ edit: "ripple-delete", clips });
      return;
  }
}

export function useTimelineKeys(): void {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (isTyping(event.target)) return;
      const action = timelineActionForKey(event);
      if (!action || !useProjectStore.getState().view) return;
      event.preventDefault();
      runTimelineAction(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
