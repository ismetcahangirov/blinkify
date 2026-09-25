import { useEffect, useRef } from "react";

import { usePreviewStore } from "../player/preview.store.js";
import { useFilesDrop } from "../player/useFileDrop.js";
import { useProjectStore } from "../project/project.store.js";
import { acceptAssetDrops, commitDrop, dropOver } from "./assetDropTarget.js";
import type { TrackRow } from "./draw.js";
import {
  clickSelection,
  withLinked,
  DRAG_THRESHOLD,
  dragTo,
  hitTest,
  neighbourAt,
  type Drag,
  type DragResult,
  type Hit,
  type Modifiers,
} from "./interaction.js";
import { mountTimeline } from "./mountTimeline.js";
import { layoutRows, RULER_HEIGHT, rowAt, stackHeight } from "./rows.js";
import { useTimelineStore } from "./timeline.store.js";
import { frameAt, ZOOM_STEP } from "./viewport.js";

/** A press on the clip area, and the drag it became. */
interface Press {
  readonly x: number;
  readonly hit: Hit | null;
  drag: Drag | null;
  result: DragResult | null;
}

/** The modifiers of a pointer event. `alt` means "do not snap": Alt with
 * snapping on, no Alt with it off (#38) — so Alt always does the opposite
 * of the setting, for one drag. */
const modifiersOf = (event: {
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}): Modifiers => ({
  ctrl: event.ctrlKey || event.metaKey,
  shift: event.shiftKey,
  alt: event.altKey === useTimelineStore.getState().snapping,
});

/** What the pointer would do here, as a cursor. */
function cursorFor(hit: Hit | null): string {
  if (!hit) return "default";
  return hit.zone === "body" ? "grab" : "ew-resize";
}

/** What the timeline says after an edit ran into a bound. */
export const CLAMPED_NOTICE =
  "Stopped at the end of the source or at the next clip.";

/**
 * The timeline canvas (#33, #34): two stacked canvases, connected to the
 * stores once by `mountTimeline`, and the pointer input over them — seeking
 * from the ruler, selecting clips, dragging them, trimming, rolling, and
 * zooming about the pointer.
 *
 * A drag is previewed on the overlay and committed once, on release, as one
 * edit: one undo entry per drag. Shift makes a trim ripple, Ctrl on a shared
 * edge rolls it, Alt suspends snapping, Escape abandons the drag.
 */
export function TimelineCanvas() {
  const host = useRef<HTMLDivElement>(null);
  const contentCanvas = useRef<HTMLCanvasElement>(null);
  const overlayCanvas = useRef<HTMLCanvasElement>(null);
  const press = useRef<Press | null>(null);
  const seeking = useRef(false);

  useEffect(() => {
    const element = host.current;
    const content = contentCanvas.current;
    const overlay = overlayCanvas.current;
    if (!element || !content || !overlay) return;
    return mountTimeline(element, content, overlay).dispose;
  }, []);

  // Library assets dragged here (#53).
  useEffect(() => {
    const element = host.current;
    if (!element) return;
    return acceptAssetDrops(element);
  }, []);

  // Files dropped here from the desktop: imported, then placed where they
  // were dropped — one step to import, one to place.
  useFilesDrop(host, (paths, point) => {
    void useProjectStore
      .getState()
      .importMedia(paths)
      .then((outcome) => {
        const element = host.current;
        if (!outcome || !element) return;
        const [refusal] = outcome.refused;
        if (outcome.imported.length > 0)
          void commitDrop(dropOver(element, outcome.imported, point, false));
        else if (refusal)
          useTimelineStore
            .getState()
            .setNotice(`Not imported: ${refusal.reason}.`);
      });
  });

  // Escape abandons a drag in progress: nothing is committed.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !press.current?.drag) return;
      press.current = null;
      useTimelineStore.getState().setDrag(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  /** Where a pointer event is, in the clip area's CSS pixels. */
  const local = (event: { clientX: number; clientY: number }) => {
    const box = host.current?.getBoundingClientRect();
    return {
      x: event.clientX - (box?.left ?? 0),
      y: event.clientY - (box?.top ?? 0),
    };
  };

  const rows = (): TrackRow[] =>
    layoutRows(
      useProjectStore.getState().view?.timeline,
      useProjectStore.getState().view?.project.sequence.tracks,
    );

  /** The track stack's y for a clip-area y. */
  const stackY = (y: number) =>
    y - RULER_HEIGHT + useTimelineStore.getState().view.scrollTop;

  const seek = (x: number): void => {
    const preview = usePreviewStore.getState();
    const project = useProjectStore.getState().view;
    if (preview.kind !== "project" || !project) return;
    const rate = project.project.sequence.settings.frameRate;
    const frame = Math.max(
      0,
      Math.floor(frameAt(useTimelineStore.getState().view, x)),
    );
    void preview.transport({
      type: "scrub",
      position: Math.ceil((frame * rate.den * 1_000_000) / rate.num),
    });
  };

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const { x, y } = local(event);
    event.currentTarget.setPointerCapture?.(event.pointerId);
    if (y < RULER_HEIGHT) {
      seeking.current = true;
      seek(x);
      return;
    }
    const view = useTimelineStore.getState().view;
    const hit = hitTest(rows(), view, x, stackY(y));
    const project = useProjectStore.getState();
    const modifiers = modifiersOf(event);
    // A press on a selected clip keeps the selection, so the selection can
    // be dragged together; anything else selects as a click does.
    const keeps =
      hit !== null &&
      project.selection.includes(hit.placement.clip) &&
      !modifiers.ctrl &&
      !modifiers.shift;
    if (!keeps)
      project.select(
        withLinked(
          clickSelection(project.selection, hit, modifiers),
          project.view?.project.sequence.tracks ?? [],
        ),
      );
    // A locked track refuses every edit: say so rather than start a drag
    // the engine would refuse anyway.
    if (hit?.row.locked) {
      useTimelineStore
        .getState()
        .setNotice("This track is locked: unlock it to change its clips.");
      press.current = { x, hit: null, drag: null, result: null };
      return;
    }
    press.current = { x, hit, drag: null, result: null };
  };

  const startDrag = (hit: Hit): Drag => {
    if (hit.zone !== "body")
      return {
        kind: "trim",
        placement: hit.placement,
        row: hit.row,
        edge: hit.zone,
        neighbour: neighbourAt(hit.row, hit.placement, hit.zone),
      };
    const selected = new Set(useProjectStore.getState().selection);
    const clips = rows().flatMap((row) =>
      row.placements
        .filter((p) => selected.has(p.clip))
        .map((placement) => ({ placement, row })),
    );
    return { kind: "move", clips, from: hit.row };
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const { x, y } = local(event);
    if (seeking.current) {
      seek(x);
      return;
    }
    const current = press.current;
    const view = useTimelineStore.getState().view;
    if (!current) {
      if (host.current)
        host.current.style.cursor =
          y < RULER_HEIGHT
            ? "default"
            : cursorFor(hitTest(rows(), view, x, stackY(y)));
      return;
    }
    if (!current.hit) return;
    if (!current.drag) {
      if (Math.abs(x - current.x) < DRAG_THRESHOLD) return;
      current.drag = startDrag(current.hit);
    }
    const all = rows();
    current.result = dragTo(current.drag, {
      frames: (x - current.x) / view.scale,
      row: rowAt(all, stackY(y)) ?? null,
      modifiers: modifiersOf(event),
      rows: all,
      view,
      playhead: useTimelineStore.getState().playhead,
      extents: useProjectStore.getState().view?.extents ?? [],
    });
    useTimelineStore.getState().setDrag(current.result);
  };

  const onPointerUp = () => {
    seeking.current = false;
    const current = press.current;
    press.current = null;
    if (!current) return;
    const timeline = useTimelineStore.getState();
    timeline.setDrag(null);
    const result = current.result;
    if (!current.drag) {
      // A click, not a drag: a plain click on one clip of a larger
      // selection selects just that clip.
      const project = useProjectStore.getState();
      if (current.hit && project.selection.length > 1)
        project.select([current.hit.placement.clip]);
      return;
    }
    if (!result) return;
    if (result.invalid) {
      timeline.setNotice(result.invalid);
      return;
    }
    if (!result.edit) return;
    void useProjectStore
      .getState()
      .edit(result.edit)
      .then((refusal) => {
        const clamped = useProjectStore.getState().lastClamped;
        timeline.setNotice(
          refusal ?? (clamped || result.clamped ? CLAMPED_NOTICE : null),
        );
      });
  };

  const onWheel = (event: React.WheelEvent<HTMLDivElement>) => {
    const store = useTimelineStore.getState();
    if (event.ctrlKey || event.metaKey) {
      store.zoomBy(
        event.deltaY < 0 ? ZOOM_STEP : 1 / ZOOM_STEP,
        local(event).x,
      );
      return;
    }
    const sideways = event.shiftKey ? event.deltaY : event.deltaX;
    const down = event.shiftKey ? 0 : event.deltaY;
    store.scrollBy(sideways, down, stackHeight(rows()));
  };

  return (
    <div
      ref={host}
      id="timeline-canvas"
      className="timeline__canvas"
      data-testid="timeline-canvas"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onWheel={onWheel}
    >
      <canvas ref={contentCanvas} className="timeline__layer" />
      <canvas
        ref={overlayCanvas}
        className="timeline__layer"
        aria-hidden="true"
      />
    </div>
  );
}
