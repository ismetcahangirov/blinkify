import { IconButton, Slider } from "@blinkify/ui";

import { LevelMeter } from "./LevelMeter.js";
import { usePreviewStore } from "./preview.store.js";

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
  const monitor = usePreviewStore((state) => state.monitor);

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
      <LevelMeter />
    </div>
  );
}
