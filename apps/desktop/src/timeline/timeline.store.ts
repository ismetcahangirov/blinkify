import type { Timeline } from "@blinkify/types";
import { create } from "zustand";
import {
  useProjectStore,
  type DeepReadonly,
} from "../project/project.store.js";
import type { DragPreview } from "./draw.js";
import { RULER_HEIGHT } from "./rows.js";
import {
  fit,
  scrollTo,
  xOf,
  zoomAround,
  ZOOM_STEP,
  type Viewport,
} from "./viewport.js";

/**
 * The timeline's view (#33): zoom, scroll and size. Renderer state only —
 * nothing here is part of the project, and nothing here changes what is
 * exported.
 *
 * The canvas subscribes to this store outside React and marks its content
 * layer dirty on a change; a zoom never re-renders a component.
 */

/** The last sequence frame of `timeline`, exclusive. */
export function timelineLength(
  timeline: DeepReadonly<Timeline> | null | undefined,
): number {
  let end = 0;
  for (const track of timeline?.tracks ?? []) {
    const last = track.placements[track.placements.length - 1];
    if (last) end = Math.max(end, last.start + last.length);
  }
  return end;
}

interface TimelineViewState {
  view: Viewport;
  /** Whether the view has been fitted to a project since it opened. */
  fitted: boolean;
  setSize: (width: number, height: number) => void;
  /** Zoom by `factor`, keeping the frame at `anchorX` in place — the
   * pointer's, or the playhead's when the zoom came from a key or button. */
  zoomBy: (factor: number, anchorX?: number) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  /** The whole project in view. */
  fitAll: () => void;
  scrollBy: (dx: number, dy: number, stackHeight: number) => void;
  /** The frame the playhead is on, in sequence frames — set by the player. */
  playhead: number | null;
  setPlayhead: (frame: number | null) => void;
  /** A drag in progress, for the overlay (#34). Never part of the graph
   * until it is released. */
  drag: DragPreview | null;
  setDrag: (drag: DragPreview | null) => void;
  /** What the last edit ran into, said briefly: a trim that met the end of
   * its source, a drop that could not happen. */
  notice: string | null;
  setNotice: (notice: string | null) => void;
}

const length = (): number =>
  timelineLength(useProjectStore.getState().view?.timeline);

export const useTimelineStore = create<TimelineViewState>((set, get) => ({
  view: { scale: 4, origin: 0, scrollTop: 0, width: 0, height: 0 },
  fitted: false,
  playhead: null,
  drag: null,
  notice: null,

  setSize: (width, height) => {
    const view = { ...get().view, width, height };
    set({ view: scrollTo(view, view.origin, length()) });
  },

  zoomBy: (factor, anchorX) => {
    const { view, playhead } = get();
    const onPlayhead = playhead === null ? null : xOf(view, playhead);
    const anchor =
      anchorX ??
      (onPlayhead !== null && onPlayhead >= 0 && onPlayhead <= view.width
        ? onPlayhead
        : view.width / 2);
    set({ view: zoomAround(view, view.scale * factor, anchor, length()) });
  },

  zoomIn: () => get().zoomBy(ZOOM_STEP),
  zoomOut: () => get().zoomBy(1 / ZOOM_STEP),

  fitAll: () => set({ view: fit(get().view, length()), fitted: true }),

  scrollBy: (dx, dy, stackHeight) => {
    const { view } = get();
    const moved = scrollTo(view, view.origin + dx / view.scale, length());
    const maxTop = Math.max(0, stackHeight - (view.height - RULER_HEIGHT));
    set({
      view: {
        ...moved,
        scrollTop: Math.min(maxTop, Math.max(0, view.scrollTop + dy)),
      },
    });
  },

  setPlayhead: (playhead) => {
    if (playhead !== get().playhead) set({ playhead });
  },

  setDrag: (drag) => set({ drag }),
  setNotice: (notice) => set({ notice }),
}));
