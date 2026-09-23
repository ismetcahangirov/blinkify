import { Slider } from "@blinkify/ui";
import { useRef, useState } from "react";

import { usePreviewStore } from "./preview.store.js";
import { formatTimecode } from "./timecode.js";

/**
 * The playhead under the video (#29): drag it and the picture follows.
 *
 * Every move during a pointer drag is sent as a `scrub`; the engine coalesces
 * them — it serves the latest position and never decodes the ones a newer
 * position overtook — so the picture keeps up with the pointer instead of
 * trailing a queue. Releasing sends a `seek`, which lands on exactly the frame
 * under the playhead. The keys move by a frame and only seek: a key press is
 * one position, not a drag. While dragging, the handle shows where the pointer
 * is; otherwise it shows the frame on screen.
 *
 * The timeline (#33) will have its own playhead over the clips; this one is
 * the player's, and covers the whole timeline.
 */
export function ScrubBar() {
  const playback = usePreviewStore((state) => state.playback);
  const framePosition = usePreviewStore((state) => state.framePosition);
  const transport = usePreviewStore((state) => state.transport);
  const [dragging, setDragging] = useState<number | null>(null);
  // Radix reports a key press's commit before its change, so only a pointer
  // that is down makes a move a drag.
  const pointerDown = useRef(false);
  if (!playback) return null;

  const { frameRate, duration } = playback;
  // One frame at the timeline rate, in microseconds.
  const step =
    frameRate.num > 0
      ? Math.max(1, (1_000_000 * frameRate.den) / frameRate.num)
      : 1;
  const value = dragging ?? framePosition ?? playback.position;
  const max = Math.max(1, duration - 1);

  return (
    <div
      className="player__scrub"
      onPointerDownCapture={() => {
        pointerDown.current = true;
      }}
      onPointerUpCapture={() => {
        pointerDown.current = false;
      }}
    >
      <Slider
        label="Playhead"
        min={0}
        max={max}
        step={step}
        value={Math.min(value, max)}
        formatValue={(position) =>
          formatTimecode(
            Math.floor(
              (position * frameRate.num) / (1_000_000 * frameRate.den),
            ),
            frameRate,
          )
        }
        onValueChange={(position) => {
          if (!pointerDown.current) return;
          setDragging(position);
          void transport({ type: "scrub", position: Math.round(position) });
        }}
        onValueCommit={(position) => {
          pointerDown.current = false;
          setDragging(null);
          void transport({ type: "seek", position: Math.round(position) });
        }}
      />
    </div>
  );
}
