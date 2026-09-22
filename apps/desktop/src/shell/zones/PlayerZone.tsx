import { memo } from "react";

import { useShellStore } from "../../shell.store.js";

/**
 * The preview player.
 *
 * An empty container. #19 builds the shell; Epic #4 fills this zone.
 *
 * `memo` is not an optimisation guess here — it is the boundary the issue asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". Each zone will grow its own store, and a re-render
 * caused by the inspector must not reach the timeline's canvas.
 *
 * It is also the zone that reports the engine's status, because the engine is
 * what the player needs before it can show anything, and a preview that is
 * blank because nothing is loaded should not look like a preview that is blank
 * because the engine never started.
 *
 * Note what subscribing to a store from inside a zone demonstrates: this zone
 * re-renders when the engine status changes and the other three do not. That is
 * the independent-subtree property the issue asks for, in use rather than
 * asserted.
 *
 * The heading is real rather than decorative. The shell is the only thing in
 * the product that builds a whole document, so it is where the landmark and
 * heading structure has to be correct — #17's story suite disables the
 * page-scope accessibility rules precisely because a component in isolation
 * cannot satisfy them and this can.
 */
export const PlayerZone = memo(function PlayerZone() {
  const engineStatus = useShellStore((state) => state.engineStatus);

  return (
    <div className="zone">
      <h2 className="zone__title">Player</h2>
      <p className="zone__placeholder">
        The video surface and the transport row arrive with the preview player.
      </p>
      {/* The shell must never imply a capability it does not have. An engine
          status that defaulted to "ready" would be the same class of claim as
          an export reporting lossless without having checked. */}
      <p className="zone__placeholder" data-testid="engine-status">
        Engine: {engineStatus}
      </p>
    </div>
  );
});
