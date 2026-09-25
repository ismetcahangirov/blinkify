import type { PreviewSpeed } from "@blinkify/types";
import { IconButton, Select } from "@blinkify/ui";

import { keyFor } from "../shortcuts/shortcuts.js";
import { loopRange, usePreviewStore } from "./preview.store.js";
import { formatTimecode } from "./timecode.js";
import {
  JumpEndGlyph,
  JumpStartGlyph,
  LoopGlyph,
  PauseGlyph,
  PlayGlyph,
  StepBackGlyph,
  StepForwardGlyph,
  StopGlyph,
} from "./transportGlyphs.js";

const SPEEDS: ReadonlyArray<{ value: PreviewSpeed; label: string }> = [
  { value: "quarter", label: "0.25×" },
  { value: "half", label: "0.5×" },
  { value: "normal", label: "1×" },
  { value: "double", label: "2×" },
];

function isSpeed(value: string): value is PreviewSpeed {
  return SPEEDS.some((speed) => speed.value === value);
}

/**
 * The transport row under the video (#28): jump, step, play and pause, stop,
 * the timecode of the frame on screen over the total, preview speed, loop —
 * and, when the engine is playing into silence because there is no audio
 * device, that fact and its reason, so silence is never mistaken for a quiet
 * clip.
 *
 * Every control sends a command and shows what the engine answered; nothing
 * here keeps time. The timecode is formatted from the frame number the
 * engine sent with the frame being shown.
 *
 * Loop plays between the in and out points (I and O, #38), or the whole
 * timeline without them; the points are shown beside the loop button.
 */
export function TransportBar() {
  const playback = usePreviewStore((state) => state.playback);
  const frameNumber = usePreviewStore((state) => state.frameNumber);
  const transport = usePreviewStore((state) => state.transport);
  const marks = usePreviewStore((state) => state.marks);
  if (!playback) return null;
  const markTimecode = (position: number) =>
    formatTimecode(
      Math.floor(
        (position * playback.frameRate.num) /
          (1_000_000 * playback.frameRate.den),
      ),
      playback.frameRate,
    );

  const playing = playback.state === "playing";
  const current =
    frameNumber === null
      ? playback.timecode
      : formatTimecode(frameNumber, playback.frameRate);
  const looping = playback.loopRange !== null;
  const send = (command: Parameters<typeof transport>[0]) => () => {
    void transport(command);
  };

  return (
    <div className="player__transport" role="toolbar" aria-label="Transport">
      <IconButton
        size="sm"
        label="Jump to start"
        shortcut={keyFor("jump-to-start")}
        icon={<JumpStartGlyph />}
        onClick={send({ type: "jump-to-start" })}
      />
      <IconButton
        size="sm"
        label="Previous frame"
        shortcut={keyFor("previous-frame")}
        icon={<StepBackGlyph />}
        onClick={send({ type: "step", frames: -1 })}
      />
      <IconButton
        size="sm"
        label={playing ? "Pause" : "Play"}
        shortcut={keyFor("play-pause")}
        icon={playing ? <PauseGlyph /> : <PlayGlyph />}
        onClick={send({ type: "toggle" })}
      />
      <IconButton
        size="sm"
        label="Next frame"
        shortcut={keyFor("next-frame")}
        icon={<StepForwardGlyph />}
        onClick={send({ type: "step", frames: 1 })}
      />
      <IconButton
        size="sm"
        label="Jump to end"
        shortcut={keyFor("jump-to-end")}
        icon={<JumpEndGlyph />}
        onClick={send({ type: "jump-to-end" })}
      />
      <IconButton
        size="sm"
        label="Stop"
        icon={<StopGlyph />}
        onClick={send({ type: "stop" })}
      />
      <output
        className="player__timecode type-timecode"
        data-testid="timecode"
        aria-label="Timecode"
      >
        {current} / {playback.durationTimecode}
      </output>
      <div className="player__transport-end">
        {playback.audio.kind === "silent" && (
          <span
            className="player__silent"
            data-testid="audio-silent"
            title={playback.audio.reason}
          >
            No sound: {playback.audio.reason}
          </span>
        )}
        <Select
          label="Preview speed"
          value={playback.speed}
          options={SPEEDS}
          onValueChange={(value) => {
            if (isSpeed(value))
              void transport({ type: "set-speed", speed: value });
          }}
        />
        {marks.in === null && marks.out === null ? null : (
          <span className="player__marks" data-testid="marks">
            {marks.in === null ? "" : `In ${markTimecode(marks.in)}`}
            {marks.in !== null && marks.out !== null ? " · " : ""}
            {marks.out === null ? "" : `Out ${markTimecode(marks.out)}`}
          </span>
        )}
        <IconButton
          size="sm"
          label={looping ? "Stop looping" : "Loop playback"}
          aria-pressed={looping}
          icon={<LoopGlyph />}
          onClick={send({
            type: "set-loop",
            range: looping ? null : loopRange(marks, playback),
          })}
        />
      </div>
    </div>
  );
}
