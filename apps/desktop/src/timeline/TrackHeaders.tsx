import { useProjectStore } from "../project/project.store.js";
import { layoutRows, RULER_HEIGHT } from "./rows.js";
import { useTimelineStore } from "./timeline.store.js";

/**
 * The track-header column (#33): fixed while the clips scroll sideways, and
 * following them up and down. One header per row, the same height as the
 * row the canvas draws. #36 fills them with name, mute, solo and lock.
 */
export function TrackHeaders() {
  const timeline = useProjectStore((state) => state.view?.timeline ?? null);
  const scrollTop = useTimelineStore((state) => state.view.scrollTop);
  const rows = layoutRows(timeline);
  let video = 0;
  let audio = 0;
  const named = rows.map((row) => ({
    ...row,
    name: row.kind === "video" ? `V${++video}` : `A${++audio}`,
  }));

  return (
    <div className="timeline__headers" aria-label="Tracks" role="list">
      <div className="timeline__corner" style={{ height: RULER_HEIGHT }} />
      <div className="timeline__header-clip">
        <div
          className="timeline__header-stack"
          style={{ transform: `translateY(${-scrollTop}px)` }}
        >
          {named.map((row) => (
            <div
              key={row.id}
              role="listitem"
              className="timeline__header"
              data-kind={row.kind}
              style={{ height: row.height }}
            >
              {row.name}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
