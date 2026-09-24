import type { CutPoint } from "@blinkify/types";
import { IconButton, Switch } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";
import { useProjectStore } from "../project/project.store.js";
import {
  cutStatement,
  detachAction,
  duplicateAction,
  freezeAction,
  reverseAction,
  splitAction,
  type Action,
} from "./editActions.js";
import { useTimelineStore } from "./timeline.store.js";
import { runTimelineAction, setSplitter } from "./useTimelineKeys.js";

/** How long the playhead must rest before the keyframe index is asked. */
const CUT_DEBOUNCE_MS = 120;

/** Send an action's edit, and say what it did or why it did nothing. */
export async function perform(action: Action): Promise<void> {
  const timeline = useTimelineStore.getState();
  if (!action.edit) {
    timeline.setNotice(action.notice);
    return;
  }
  const refusal = await useProjectStore.getState().edit(action.edit);
  timeline.setNotice(refusal ?? action.notice);
}

setSplitter(() => split());

export function split(): Promise<void> {
  const project = useProjectStore.getState();
  const timeline = useTimelineStore.getState();
  return perform(
    splitAction(
      project.view?.timeline,
      project.selection,
      timeline.playhead,
      timeline.cut,
      timeline.snapToKeyframe,
    ),
  );
}

/**
 * The keyframe indicator (#35): whether a cut at the playhead costs nothing
 * at export. It is what makes the product's premise visible while editing,
 * not only at export — so it follows the playhead, asking the keyframe index
 * once the playhead rests.
 */
function useCutPoint(): CutPoint | null {
  const playhead = useTimelineStore((state) => state.playhead);
  const view = useProjectStore((state) => state.view);
  const cut = useTimelineStore((state) => state.cut);
  const setCut = useTimelineStore((state) => state.setCut);
  useEffect(() => {
    if (playhead === null || !view?.timeline) {
      setCut(null);
      return;
    }
    let stale = false;
    const timer = setTimeout(() => {
      invoke<CutPoint | null>("cut_point_at", { position: playhead })
        .then((answer) => {
          if (!stale) setCut(answer);
        })
        .catch(() => {
          if (!stale) setCut(null);
        });
    }, CUT_DEBOUNCE_MS);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
  }, [playhead, view, setCut]);
  return cut;
}

function Glyph({ d }: { d: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d={d}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function EditToolbar() {
  const hasTimeline = useProjectStore((state) => Boolean(state.view?.timeline));
  const selected = useProjectStore((state) => state.selection.length > 0);
  const snap = useTimelineStore((state) => state.snapToKeyframe);
  const setSnap = useTimelineStore((state) => state.setSnapToKeyframe);
  const cut = useCutPoint();
  const statement = cutStatement(cut);
  const project = () => useProjectStore.getState();

  return (
    <div className="timeline__edit" role="toolbar" aria-label="Edit">
      <IconButton
        label="Split at the playhead"
        shortcut="Ctrl+B"
        icon={<Glyph d="M8 2v12M4 5l-2 3 2 3M12 5l2 3-2 3" />}
        disabled={!hasTimeline}
        onClick={() => void split()}
      />
      <IconButton
        label="Delete"
        shortcut="Delete"
        icon={<Glyph d="M3 4.5h10M6 4.5V3h4v1.5M4.5 4.5l.7 8.5h5.6l.7-8.5" />}
        disabled={!selected}
        onClick={() => runTimelineAction("delete")}
      />
      <IconButton
        label="Duplicate"
        icon={<Glyph d="M5 5h8v8H5zM3 11V3h8" />}
        disabled={!selected}
        onClick={() => void perform(duplicateAction(project().selection))}
      />
      <IconButton
        label="Freeze frame"
        icon={<Glyph d="M8 2v12M3 5l10 6M3 11l10-6" />}
        disabled={!hasTimeline}
        onClick={() =>
          void perform(
            freezeAction(
              project().view?.timeline,
              project().selection,
              useTimelineStore.getState().playhead,
            ),
          )
        }
      />
      <IconButton
        label="Reverse"
        icon={<Glyph d="M12 5H5l2-2M5 5l2 2M4 11h7l-2-2M11 11l-2 2" />}
        disabled={!selected}
        onClick={() =>
          void perform(
            reverseAction(project().view?.timeline, project().selection),
          )
        }
      />
      <IconButton
        label="Detach audio"
        icon={<Glyph d="M3 4h7v5H3zM5 12h8M11 10l2 2-2 2" />}
        disabled={!selected}
        onClick={() =>
          void perform(
            detachAction(project().view?.timeline, project().selection),
          )
        }
      />
      <IconButton
        label="Unlink"
        icon={
          <Glyph d="M6 10l-1.5 1.5a2 2 0 0 1-3-3L3 7M10 6l1.5-1.5a2 2 0 0 1 3 3L13 9M6 6l4 4" />
        }
        disabled={!selected}
        onClick={() =>
          void perform({
            edit: { edit: "unlink", clips: [...project().selection] },
            notice: null,
          })
        }
      />
      <Switch
        label="Snap cuts to keyframes"
        checked={snap}
        onCheckedChange={setSnap}
        disabled={!hasTimeline}
      />
      <span
        className="timeline__cut"
        data-state={statement.state}
        data-testid="cut-indicator"
        role="status"
      >
        {statement.text}
      </span>
    </div>
  );
}
