import { useProjectStore } from "../project/project.store.js";
import { allClips } from "./interaction.js";
import { layoutRows } from "./rows.js";

/**
 * The timeline's editing commands (#34): delete, ripple delete, select all
 * and split, as the keyboard (#38, `shortcuts/`) and the edit toolbar send
 * them.
 */
export type TimelineAction =
  "delete" | "ripple-delete" | "select-all" | "split";

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
      project.select(
        allClips(
          layoutRows(
            project.view?.timeline,
            project.view?.project.sequence.tracks,
          ),
        ),
      );
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
