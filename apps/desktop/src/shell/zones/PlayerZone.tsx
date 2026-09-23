import { Button } from "@blinkify/ui";
import { memo, useCallback, useRef, useState } from "react";

import { DecodeStatsOverlay } from "../../player/DecodeStatsOverlay.js";
import { PreviewCanvas } from "../../player/PreviewCanvas.js";
import { usePreviewStore } from "../../player/preview.store.js";
import { useFileDrop } from "../../player/useFileDrop.js";
import { useShellStore } from "../../shell.store.js";

/**
 * The preview player.
 *
 * #27 fills it with the video surface: drop a file on it and the engine
 * decodes it into raw frames this zone draws. Transport controls arrive with
 * #28, seek and scrub with #29, and the edit graph with #30 — until then a
 * dropped file plays from its start.
 *
 * `memo` is not an optimisation guess here — it is the boundary the shell asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". The preview's own state lives in its own store,
 * and a re-render caused by the inspector must not reach the video surface.
 *
 * It is also the zone that reports the engine's status, because the engine is
 * what the player needs before it can show anything, and a preview that is
 * blank because nothing is loaded should not look like a preview that is blank
 * because the engine never started.
 */
export const PlayerZone = memo(function PlayerZone() {
  const engineStatus = useShellStore((state) => state.engineStatus);
  const status = usePreviewStore((state) => state.status);
  const preview = usePreviewStore((state) => state.preview);
  const path = usePreviewStore((state) => state.path);
  const error = usePreviewStore((state) => state.error);
  const open = usePreviewStore((state) => state.open);
  const [showStats, setShowStats] = useState(false);
  const surface = useRef<HTMLDivElement>(null);

  const openDropped = useCallback(
    (dropped: string) => {
      const element = surface.current;
      const ratio = window.devicePixelRatio || 1;
      // Decode at the size the surface can show, not the source's: preview
      // quality is deliberately decoupled from export quality.
      void open(
        dropped,
        (element?.clientWidth ?? 0) * ratio,
        (element?.clientHeight ?? 0) * ratio,
      );
    },
    [open],
  );
  useFileDrop(surface, openDropped);

  return (
    <div className="zone player">
      <div className="player__header">
        <h2 className="zone__title">Player</h2>
        {preview && (
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={showStats}
            onClick={() => {
              setShowStats((shown) => !shown);
            }}
          >
            Stats
          </Button>
        )}
      </div>
      <div
        ref={surface}
        className="player__surface"
        data-testid="player-surface"
      >
        {preview ? (
          <PreviewCanvas
            key={preview.session}
            session={preview.session}
            rotation={preview.info.rotation}
          />
        ) : (
          <p className="zone__placeholder">
            {status === "opening"
              ? `Opening ${path ?? ""}…`
              : "Drop a video file here to preview it."}
          </p>
        )}
        {preview && showStats && <DecodeStatsOverlay />}
      </div>
      {status === "failed" && error !== null && (
        <p className="player__error" role="alert">
          {error}
        </p>
      )}
      {/* The shell must never imply a capability it does not have. An engine
          status that defaulted to "ready" would be the same class of claim as
          an export reporting lossless without having checked. */}
      <p className="zone__placeholder" data-testid="engine-status">
        Engine: {engineStatus}
      </p>
    </div>
  );
});
