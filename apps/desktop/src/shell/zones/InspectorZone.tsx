import { memo } from "react";
import { ClipTimingFields } from "../../inspector/ClipTimingFields.js";
import { useProjectStore } from "../../project/project.store.js";

/**
 * The inspector.
 *
 * #19 builds the shell; #49 and #56 fill this zone. A single selected clip
 * shows its timing (#34), which #56 builds the rest of the inspector around.
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
  // The one selected clip, as the evaluator placed it: the inspector is a
  // view of the selection and of the graph, never a store of its own.
  const placement = useProjectStore((state) => {
    if (state.selection.length !== 1) return null;
    const [clip] = state.selection;
    for (const track of state.view?.timeline?.tracks ?? [])
      for (const p of track.placements) if (p.clip === clip) return p;
    return null;
  });
  const count = useProjectStore((state) => state.selection.length);
  return (
    <div className="zone">
      <h2 className="zone__title">Inspector</h2>
      {placement ? (
        <ClipTimingFields placement={placement} />
      ) : (
        <p className="zone__placeholder">
          {count > 1
            ? `${count} clips are selected.`
            : "Contents follow the timeline selection. Nothing is selected."}
        </p>
      )}
    </div>
  );
});
