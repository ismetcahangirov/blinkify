import { memo } from "react";

/**
 * The inspector.
 *
 * An empty container. #19 builds the shell; #49 and #56 fills this zone.
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
  return (
    <div className="zone">
      <h2 className="zone__title">Inspector</h2>
      <p className="zone__placeholder">
        Contents follow the timeline selection. Nothing is selected.
      </p>
    </div>
  );
});
