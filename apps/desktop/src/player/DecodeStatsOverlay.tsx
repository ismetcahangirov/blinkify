import { useEffect } from "react";

import { usePreviewStore } from "./preview.store.js";

/** How often the overlay asks the engine for fresh numbers. */
const REFRESH_MS = 500;

/**
 * Decode statistics for diagnosis (#27): buffer depth against its bound,
 * frames dropped, decode rate. The numbers are the engine's; this only
 * displays them.
 */
export function DecodeStatsOverlay() {
  const stats = usePreviewStore((state) => state.stats);
  const refreshStats = usePreviewStore((state) => state.refreshStats);

  useEffect(() => {
    void refreshStats();
    const timer = setInterval(() => {
      void refreshStats();
    }, REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  }, [refreshStats]);

  if (!stats) return null;
  const megabytes = (stats.bufferedBytes / (1024 * 1024)).toFixed(1);
  return (
    <dl className="player__stats type-timecode" data-testid="decode-stats">
      <dt>Buffer</dt>
      <dd>
        {stats.bufferedFrames}/{stats.capacityFrames} frames · {megabytes} MB
      </dd>
      <dt>Decoded</dt>
      <dd>
        {stats.decodedFrames} · {stats.decodeFps.toFixed(1)} fps
      </dd>
      <dt>Shown</dt>
      <dd>{stats.presentedFrames}</dd>
      <dt>Dropped</dt>
      <dd>{stats.droppedFrames}</dd>
      <dt>Resyncs</dt>
      <dd>{stats.resyncs}</dd>
      {stats.error !== null && (
        <>
          <dt>Error</dt>
          <dd>{stats.error}</dd>
        </>
      )}
    </dl>
  );
}
