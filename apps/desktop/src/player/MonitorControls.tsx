import type { MonitorLevels } from "@blinkify/types";
import { IconButton, Slider } from "@blinkify/ui";
import { useEffect } from "react";

import { usePreviewStore } from "./preview.store.js";

/** How often the meter is read: often enough to look alive, cheap to ask. */
const METER_MS = 66;

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

function SpeakerGlyph({ muted }: { muted: boolean }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3 6h2.5L9 3v10L5.5 10H3z" fill="currentColor" />
      {muted ? (
        <path
          d="M11 6l4 4M15 6l-4 4"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinecap="round"
        />
      ) : (
        <path
          d="M11 5.5a3.5 3.5 0 0 1 0 5"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinecap="round"
        />
      )}
    </svg>
  );
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
 * Monitoring (#31): mute, the monitor volume, and the level meter with its
 * clip indication.
 *
 * The volume is the editor's listening level. It is sent as a monitor
 * command, kept apart from clip gain in the engine's types, and never reaches
 * an export. The meter measures the programme before that volume, so turning
 * the monitor down does not make a clip look quieter than it is. A clip stays
 * lit until it is clicked.
 */
export function MonitorControls() {
  const monitoring = usePreviewStore((state) => state.monitoring);
  const levels = usePreviewStore((state) => state.levels);
  const monitor = usePreviewStore((state) => state.monitor);
  const refreshLevels = usePreviewStore((state) => state.refreshLevels);

  useEffect(() => {
    const timer = setInterval(() => {
      void refreshLevels();
    }, METER_MS);
    return () => {
      clearInterval(timer);
    };
  }, [refreshLevels]);

  const clipped = levels?.clipped ?? false;
  return (
    <div className="player__monitor">
      <IconButton
        size="sm"
        label={monitoring.muted ? "Unmute monitor" : "Mute monitor"}
        aria-pressed={monitoring.muted}
        icon={<SpeakerGlyph muted={monitoring.muted} />}
        onClick={() => {
          void monitor({ type: "mute", muted: !monitoring.muted });
        }}
      />
      <Slider
        className="player__volume"
        label="Monitor volume"
        min={0}
        max={1}
        step={0.01}
        value={monitoring.volume}
        formatValue={(value) => `${String(Math.round(value * 100))}%`}
        onValueChange={(level) => {
          void monitor({ type: "volume", level });
        }}
      />
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
    </div>
  );
}
