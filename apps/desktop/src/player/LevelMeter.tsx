import type { MonitorLevels } from "@blinkify/types";

import { useMetering } from "./metering.js";
import { usePreviewStore } from "./preview.store.js";

/** The bottom of the meter's scale, in dBFS. */
const FLOOR_DB = -60;

/** Where a level sits on the meter, 0 to 100 percent. */
export function meterPercent(db: number | null): number {
  if (db === null) return 0;
  const clamped = Math.min(0, Math.max(FLOOR_DB, db));
  return ((clamped - FLOOR_DB) / -FLOOR_DB) * 100;
}

/** The meter's colour role for a peak: loud is a warning before it is a clip. */
function band(db: number | null): "ok" | "loud" | "hot" {
  if (db === null || db < -18) return "ok";
  return db < -6 ? "loud" : "hot";
}

function Meter({ levels }: { levels: MonitorLevels | null }) {
  const peaks = levels?.peakDb ?? [null, null];
  const lufs = levels?.shortTermLufs ?? null;
  return (
    <div className="player__meter" data-testid="level-meter">
      {peaks.map((db, channel) => (
        <div
          key={channel}
          className="player__meter-track"
          role="meter"
          aria-label={channel === 0 ? "Left peak" : "Right peak"}
          aria-valuemin={FLOOR_DB}
          aria-valuemax={0}
          aria-valuenow={db ?? FLOOR_DB}
        >
          <div
            className={`player__meter-bar player__meter-bar--${band(db)}`}
            style={{ width: `${String(meterPercent(db))}%` }}
          />
        </div>
      ))}
      <output
        className="player__loudness type-timecode"
        aria-label="Short-term loudness"
      >
        {lufs === null ? "— LUFS" : `${lufs.toFixed(1)} LUFS`}
      </output>
    </div>
  );
}

/**
 * The level meter and its clip indication (#31), wherever it is shown: the
 * player's monitor strip, and the audio inspector (#49), which shows the
 * sound after the clips' chains — what the chain produced, before the
 * monitor volume. A clip stays lit until it is clicked.
 */
export function LevelMeter() {
  useMetering();
  const levels = usePreviewStore((state) => state.levels);
  const monitor = usePreviewStore((state) => state.monitor);
  const clipped = levels?.clipped ?? false;
  return (
    <>
      <Meter levels={levels} />
      <button
        type="button"
        className={`player__clip${clipped ? " player__clip--lit" : ""}`}
        aria-pressed={clipped}
        aria-label={clipped ? "Clipped — reset" : "No clipping"}
        disabled={!clipped}
        onClick={() => {
          void monitor({ type: "reset-clip" });
        }}
      >
        CLIP
      </button>
    </>
  );
}
