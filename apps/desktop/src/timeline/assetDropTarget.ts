import { registerDropTarget, type Point } from "../library/assetDrag.js";
import { useProjectStore } from "../project/project.store.js";
import { dropAt, droppedAssets, type DropResult } from "./assetDrop.js";
import { layoutRows, RULER_HEIGHT, rowAt } from "./rows.js";
import { useTimelineStore } from "./timeline.store.js";

/**
 * The timeline as a place to drop media (#53): library assets dragged onto
 * it, and files dropped on it from the desktop once they are imported.
 */

/** Where `sources` would land if released at `point`, over `element`. */
export function dropOver(
  element: HTMLElement,
  sources: readonly number[],
  point: Point,
  noSnap: boolean,
): DropResult {
  const project = useProjectStore.getState().view;
  const timeline = useTimelineStore.getState();
  const box = element.getBoundingClientRect();
  const rows = layoutRows(project?.timeline, project?.project.sequence.tracks);
  const y = point.y - box.top;
  const row =
    y < RULER_HEIGHT
      ? null
      : (rowAt(rows, y - RULER_HEIGHT + timeline.view.scrollTop) ?? null);
  return dropAt({
    assets: droppedAssets(project, sources),
    rows,
    row,
    x: point.x - box.left,
    view: timeline.view,
    playhead: timeline.playhead,
    extents: project?.extents ?? [],
    frameRate: project?.project.sequence.settings.frameRate ?? {
      num: 30,
      den: 1,
    },
    noSnap,
  });
}

/**
 * Send a drop's edits: one undo entry however many clips it places, and
 * what went wrong, if anything, in the timeline's notice.
 */
export async function commitDrop(result: DropResult): Promise<void> {
  const timeline = useTimelineStore.getState();
  if (result.invalid) {
    timeline.setNotice(result.invalid);
    return;
  }
  const project = useProjectStore.getState();
  const [only] = result.edits;
  if (result.edits.length === 1 && only) {
    timeline.setNotice(await project.edit(only));
    return;
  }
  await project.beginGesture("Add clips");
  let refusal: string | null = null;
  for (const edit of result.edits) {
    refusal = await project.edit(edit);
    if (refusal) break;
  }
  await project.endGesture();
  timeline.setNotice(refusal);
}

/** Accept library drags on `element` until the returned function is called. */
export function acceptAssetDrops(element: HTMLElement): () => void {
  const contains = (point: Point) => {
    const box = element.getBoundingClientRect();
    return (
      point.x >= box.left &&
      point.x < box.right &&
      point.y >= box.top &&
      point.y < box.bottom
    );
  };
  return registerDropTarget({
    contains,
    hover: (payload, point, modifiers) =>
      useTimelineStore
        .getState()
        .setDrag(dropOver(element, payload.sources, point, modifiers.alt)),
    leave: () => useTimelineStore.getState().setDrag(null),
    drop: (payload, point, modifiers) => {
      useTimelineStore.getState().setDrag(null);
      void commitDrop(dropOver(element, payload.sources, point, modifiers.alt));
    },
  });
}
