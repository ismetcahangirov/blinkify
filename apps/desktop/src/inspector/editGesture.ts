import type { Edit } from "@blinkify/types";
import { useProjectStore } from "../project/project.store.js";

/**
 * A control being dragged, as one undoable step (#37, #56).
 *
 * The first change opens a gesture in the engine; every change after it is
 * applied live, so the timeline, the preview and the verdict follow the
 * pointer; the end closes the gesture, and the whole drag is one history
 * entry. Edits go one at a time and only the latest waiting one is sent — a
 * drag that outruns the engine skips values rather than queueing hundreds,
 * and cannot arrive out of order.
 */
export interface EditGesture {
  change: (edit: Edit) => void;
  /** Resolves once every change has been applied and the gesture closed. */
  end: () => Promise<void>;
}

export function editGesture(label: string): EditGesture {
  const project = () => useProjectStore.getState();
  let opened: Promise<void> | null = null;
  let waiting: Edit | null = null;
  let sending: Promise<void> | null = null;

  const send = async (): Promise<void> => {
    while (waiting) {
      const edit = waiting;
      waiting = null;
      await project().edit(edit);
    }
    sending = null;
  };

  return {
    change: (edit) => {
      opened ??= project().beginGesture(label);
      waiting = edit;
      sending ??= opened.then(send);
    },
    end: async () => {
      if (!opened) return;
      await opened;
      opened = null;
      while (sending) await sending;
      await project().endGesture();
    },
  };
}
