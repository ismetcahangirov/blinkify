import { usePreviewStore } from "../player/preview.store.js";
import { runFileAction } from "../project/ProjectLifecycle.js";
import { useProjectStore } from "../project/project.store.js";
import { perform } from "../timeline/EditToolbar.js";
import { duplicateAction } from "../timeline/editActions.js";
import { runTimelineAction } from "../timeline/timelineCommands.js";
import { useTimelineStore } from "../timeline/timeline.store.js";
import type { ActionId } from "./shortcuts.js";
import { useShortcutsUi } from "./shortcuts.store.js";

/**
 * What each shortcut does (#38). One handler per action, reading the stores
 * at the moment the key is pressed — never a handler registered by a
 * component, which is how a key keeps firing after its component is gone.
 *
 * A handler returns `false` when there is nothing for it to act on — no
 * preview for the transport, no project for an edit. The key is then left
 * alone, so the browser or the focused control can have it.
 */

/** About a second of timeline frames: how far J moves back. */
export function framesPerSecond(frameRate: {
  num: number;
  den: number;
}): number {
  return frameRate.den > 0
    ? Math.max(1, Math.round(frameRate.num / frameRate.den))
    : 30;
}

type Handler = () => boolean;

function transport(
  command: Parameters<
    ReturnType<typeof usePreviewStore.getState>["transport"]
  >[0],
): boolean {
  const preview = usePreviewStore.getState();
  if (preview.session === null) return false;
  void preview.transport(command);
  return true;
}

const hasProject = () => useProjectStore.getState().view !== null;
const hasTimeline = () => Boolean(useProjectStore.getState().view?.timeline);
const notice = (text: string) => useTimelineStore.getState().setNotice(text);

function edit(run: () => void): boolean {
  if (!hasTimeline()) return false;
  run();
  return true;
}

export const HANDLERS: Readonly<Record<ActionId, Handler>> = {
  "play-pause": () => transport({ type: "toggle" }),
  "previous-frame": () => transport({ type: "step", frames: -1 }),
  "next-frame": () => transport({ type: "step", frames: 1 }),
  "jump-to-start": () => transport({ type: "jump-to-start" }),
  "jump-to-end": () => transport({ type: "jump-to-end" }),
  "shuttle-back": () => {
    const playback = usePreviewStore.getState().playback;
    return transport({
      type: "step",
      frames: -(playback ? framesPerSecond(playback.frameRate) : 30),
    });
  },
  "shuttle-pause": () => transport({ type: "pause" }),
  "shuttle-forward": () =>
    transport(
      usePreviewStore.getState().playback?.state === "playing"
        ? { type: "set-speed", speed: "double" }
        : { type: "play" },
    ),
  "mark-in": () => {
    const preview = usePreviewStore.getState();
    if (preview.session === null) return false;
    void preview.mark("in");
    return true;
  },
  "mark-out": () => {
    const preview = usePreviewStore.getState();
    if (preview.session === null) return false;
    void preview.mark("out");
    return true;
  },
  undo: () => {
    if (!hasProject()) return false;
    void useProjectStore.getState().undo();
    return true;
  },
  redo: () => {
    if (!hasProject()) return false;
    void useProjectStore.getState().redo();
    return true;
  },
  copy: () => {
    const { selection } = useProjectStore.getState();
    if (!hasTimeline() || selection.length === 0) return false;
    useTimelineStore.getState().setClipboard(selection);
    notice(
      selection.length === 1
        ? "Copied 1 clip."
        : `Copied ${selection.length} clips.`,
    );
    return true;
  },
  paste: () =>
    edit(() => {
      const { clipboard, playhead } = useTimelineStore.getState();
      if (clipboard.length === 0) {
        notice("Nothing is copied: select clips and press Ctrl+C first.");
        return;
      }
      if (playhead === null) {
        notice("Open the project's preview to paste at the playhead.");
        return;
      }
      void perform({
        edit: { edit: "paste", clips: [...clipboard], at: playhead },
        notice: null,
      });
    }),
  duplicate: () =>
    edit(
      () => void perform(duplicateAction(useProjectStore.getState().selection)),
    ),
  delete: () => edit(() => runTimelineAction("delete")),
  "ripple-delete": () => edit(() => runTimelineAction("ripple-delete")),
  "select-all": () => edit(() => runTimelineAction("select-all")),
  split: () => edit(() => runTimelineAction("split")),
  "zoom-in": () => edit(() => useTimelineStore.getState().zoomIn()),
  "zoom-out": () => edit(() => useTimelineStore.getState().zoomOut()),
  "zoom-to-fit": () => edit(() => useTimelineStore.getState().fitAll()),
  "toggle-snapping": () => {
    const timeline = useTimelineStore.getState();
    timeline.setSnapping(!timeline.snapping);
    notice(timeline.snapping ? "Snapping is off." : "Snapping is on.");
    return true;
  },
  "new-project": () => {
    runFileAction("new");
    return true;
  },
  "open-project": () => {
    runFileAction("open");
    return true;
  },
  save: () => {
    if (!hasProject()) return false;
    runFileAction("save");
    return true;
  },
  "save-as": () => {
    if (!hasProject()) return false;
    runFileAction("save-as");
    return true;
  },
  export: () => {
    // Bound now so the key is not given to anything else; the export
    // itself is Epic #6's.
    notice("Export is not available yet.");
    return true;
  },
  "show-shortcuts": () => {
    useShortcutsUi.getState().setReferenceOpen(true);
    return true;
  },
};
