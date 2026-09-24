import { useEffect, useRef } from "react";

import { usePreviewStore } from "../player/preview.store.js";
import { useProjectStore } from "../project/project.store.js";
import { placementsIn } from "./draw.js";
import { mountTimeline } from "./mountTimeline.js";
import { layoutRows, RULER_HEIGHT, rowAt, stackHeight } from "./rows.js";
import { useTimelineStore } from "./timeline.store.js";
import { frameAt, ZOOM_STEP } from "./viewport.js";

/**
 * The timeline canvas (#33): two stacked canvases, connected to the stores
 * once by `mountTimeline`, and the pointer and wheel input over them —
 * seeking from the ruler, selecting a clip, zooming about the pointer.
 */
export function TimelineCanvas() {
  const host = useRef<HTMLDivElement>(null);
  const contentCanvas = useRef<HTMLCanvasElement>(null);
  const overlayCanvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const element = host.current;
    const content = contentCanvas.current;
    const overlay = overlayCanvas.current;
    if (!element || !content || !overlay) return;
    return mountTimeline(element, content, overlay).dispose;
  }, []);

  /** Where a pointer event is, in the clip area's CSS pixels. */
  const local = (event: { clientX: number; clientY: number }) => {
    const box = host.current?.getBoundingClientRect();
    return {
      x: event.clientX - (box?.left ?? 0),
      y: event.clientY - (box?.top ?? 0),
    };
  };

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
    if (y < RULER_HEIGHT) {
      event.currentTarget.setPointerCapture?.(event.pointerId);
      seek(x);
      return;
    }
    const view = useTimelineStore.getState().view;
    const rows = layoutRows(useProjectStore.getState().view?.timeline);
    const row = rowAt(rows, y - RULER_HEIGHT + view.scrollTop);
    const frame = frameAt(view, x);
    const hit = row
      ? placementsIn(
          row.placements,
          Math.floor(frame),
          Math.floor(frame) + 1,
        )[0]
      : undefined;
    useProjectStore.getState().select(hit ? [hit.clip] : []);
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture?.(event.pointerId))
      seek(local(event).x);
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
    const rows = layoutRows(useProjectStore.getState().view?.timeline);
    const sideways = event.shiftKey ? event.deltaY : event.deltaX;
    const down = event.shiftKey ? 0 : event.deltaY;
    store.scrollBy(sideways, down, stackHeight(rows));
  };

  return (
    <div
      ref={host}
      id="timeline-canvas"
      className="timeline__canvas"
      data-testid="timeline-canvas"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
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
