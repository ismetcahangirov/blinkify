import { IconButton } from "@blinkify/ui";
import { memo, useEffect } from "react";
import { useTimelineKeys } from "../../timeline/useTimelineKeys.js";
import { EditToolbar } from "../../timeline/EditToolbar.js";
import { TimelineCanvas } from "../../timeline/TimelineCanvas.js";
import { TimelineScrollbar } from "../../timeline/TimelineScrollbar.js";
import { TrackHeaders } from "../../timeline/TrackHeaders.js";
import { useTimelineStore } from "../../timeline/timeline.store.js";
import { useProjectStore } from "../../project/project.store.js";

/**
 * The timeline (#33): a toolbar, the fixed track-header column, the canvas,
 * and the horizontal scrollbar.
 *
 * The canvas draws the evaluated timeline the engine sends with the project
 * (#30, #37) — never the graph's operations. Clip interaction is #34, the
 * track controls in the header column are #36.
 *
 * `memo` is not an optimisation guess here — it is the boundary the issue asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". The canvas goes further and draws from its own
 * subscriptions, so not even this zone re-renders while the playhead moves.
 *
 * The heading is real rather than decorative. The shell is the only thing in
 * the product that builds a whole document, so it is where the landmark and
 * heading structure has to be correct.
 */
export const TimelineZone = memo(function TimelineZone() {
  const hasTimeline = useProjectStore((state) => Boolean(state.view?.timeline));
  const zoomIn = useTimelineStore((state) => state.zoomIn);
  const zoomOut = useTimelineStore((state) => state.zoomOut);
  const fitAll = useTimelineStore((state) => state.fitAll);
  const notice = useTimelineStore((state) => state.notice);
  const setNotice = useTimelineStore((state) => state.setNotice);
  useTimelineKeys();

  // A notice is information for a moment, not a state to dismiss.
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(timer);
  }, [notice, setNotice]);

  return (
    <div className="zone timeline">
      <div className="timeline__toolbar">
        <h2 className="zone__title">Timeline</h2>
        <p
          className="timeline__notice"
          role="status"
          data-testid="timeline-notice"
        >
          {notice}
        </p>
        <EditToolbar />
        <div className="timeline__zoom" role="group" aria-label="Zoom">
          <IconButton
            label="Zoom out"
            icon={<ZoomGlyph minus />}
            disabled={!hasTimeline}
            onClick={zoomOut}
          />
          <IconButton
            label="Fit timeline to window"
            icon={<FitGlyph />}
            disabled={!hasTimeline}
            onClick={fitAll}
          />
          <IconButton
            label="Zoom in"
            icon={<ZoomGlyph />}
            disabled={!hasTimeline}
            onClick={zoomIn}
          />
        </div>
      </div>
      {hasTimeline ? (
        <div className="timeline__body">
          <TrackHeaders />
          <div className="timeline__main">
            <TimelineCanvas />
            <TimelineScrollbar />
          </div>
        </div>
      ) : (
        <p className="zone__placeholder">Open a project to see its timeline.</p>
      )}
    </div>
  );
});

function ZoomGlyph({ minus = false }: { minus?: boolean }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <circle
        cx="7"
        cy="7"
        r="4.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
      />
      <path
        d={minus ? "M5 7h4M10.5 10.5L14 14" : "M5 7h4M7 5v4M10.5 10.5L14 14"}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

function FitGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M2 5V2h3M14 5V2h-3M2 11v3h3M14 11v3h-3M5 8h6"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
