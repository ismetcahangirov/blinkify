import { useRef } from "react";
import { useProjectStore } from "../project/project.store.js";
import { timelineLength, useTimelineStore } from "./timeline.store.js";
import { maxOrigin, scrollTo } from "./viewport.js";

/**
 * The horizontal scrollbar (#33). Drawn by hand rather than by the browser:
 * a native scroll area would need a spacer as wide as the timeline at the
 * current zoom, and an hour at single-frame zoom is millions of pixels —
 * past what a browser lays out reliably. The thumb is the visible share of
 * the scrollable range.
 */
export function TimelineScrollbar() {
  const track = useRef<HTMLDivElement>(null);
  const view = useTimelineStore((state) => state.view);
  const length = useProjectStore((state) =>
    timelineLength(state.view?.timeline),
  );
  const drag = useRef<{ x: number; origin: number } | null>(null);

  const span = maxOrigin(view, length) + view.width / view.scale;
  const share = span > 0 ? Math.min(1, view.width / view.scale / span) : 1;
  const offset = span > 0 ? view.origin / span : 0;

  const move = (clientX: number): void => {
    const start = drag.current;
    const width = track.current?.clientWidth ?? 0;
    if (!start || width === 0) return;
    const frames = ((clientX - start.x) / width) * span;
    useTimelineStore.setState({
      view: scrollTo(view, start.origin + frames, length),
    });
  };

  return (
    <div
      ref={track}
      className="timeline__scrollbar"
      role="scrollbar"
      aria-orientation="horizontal"
      aria-label="Scroll the timeline"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(offset * 100)}
      aria-controls="timeline-canvas"
    >
      <div
        className="timeline__thumb"
        style={{ left: `${offset * 100}%`, width: `${share * 100}%` }}
        onPointerDown={(event) => {
          event.currentTarget.setPointerCapture?.(event.pointerId);
          drag.current = { x: event.clientX, origin: view.origin };
        }}
        onPointerMove={(event) => move(event.clientX)}
        onPointerUp={() => {
          drag.current = null;
        }}
      />
    </div>
  );
}
