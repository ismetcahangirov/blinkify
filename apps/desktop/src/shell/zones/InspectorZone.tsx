import { ScrollArea } from "@blinkify/ui";
import { memo, useMemo } from "react";
import { AudioInspector } from "../../inspector/AudioInspector.js";
import { SequenceSummary } from "../../inspector/SequenceSummary.js";
import { VideoInspector } from "../../inspector/VideoInspector.js";
import { useProjectStore } from "../../project/project.store.js";
import type { Placement } from "../../timeline/draw.js";

/**
 * The inspector.
 *
 * Its content is driven entirely by the timeline selection (#18): nothing
 * selected shows the sequence; video clips show the video section (#56).
 * The audio section is #49's.
 *
 * `memo` is not an optimisation guess here — it is the boundary the issue asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". Each zone will grow its own store, and a re-render
 * caused by the inspector must not reach the timeline's canvas.
 *
 * The heading is real rather than decorative. The shell is the only thing in
 * the product that builds a whole document, so it is where the landmark and
 * heading structure has to be correct — #17's story suite disables the
 * page-scope accessibility rules precisely because a component in isolation
 * cannot satisfy them and this can.
 */
export const InspectorZone = memo(function InspectorZone() {
  const view = useProjectStore((state) => state.view);
  const selection = useProjectStore((state) => state.selection);
  // The selected clips as the evaluator placed them: the inspector is a
  // view of the selection and of the graph, never a store of its own.
  const selected = useMemo(() => {
    const chosen = new Set(selection);
    const placements: Placement[] = [];
    for (const track of view?.timeline?.tracks ?? [])
      for (const p of track.placements)
        if (chosen.has(p.clip)) placements.push(p);
    return placements;
  }, [view, selection]);
  const video = selected.filter((p) => p.kind === "video");
  // A video clip whose sound was detached plays none: its audio clip is
  // the one to change.
  const sounding = selected.filter((p) => !p.silent);

  return (
    <div className="zone">
      <h2 className="zone__title">Inspector</h2>
      <ScrollArea className="inspector">
        {selection.length === 0 ? (
          <SequenceSummary />
        ) : selected.length === 0 ? (
          <p className="zone__placeholder">
            The selected clips are no longer on the timeline.
          </p>
        ) : (
          <div className="inspector__sections">
            {video.length > 0 ? <VideoInspector clips={video} /> : null}
            {sounding.length > 0 ? <AudioInspector clips={sounding} /> : null}
          </div>
        )}
      </ScrollArea>
    </div>
  );
});
