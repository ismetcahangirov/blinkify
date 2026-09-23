import type { Rational } from "@blinkify/types";

/**
 * `HH:MM:SS:FF` for a timeline frame number.
 *
 * The frame number comes from the engine with every frame
 * (`blinkify_engine::playback::timecode::frame_number`), so the timecode is
 * the timecode of the frame on screen. This only lays it out — non-drop-frame
 * at the nominal rate, 30 for 30000/1001 — exactly as
 * `blinkify_engine::playback::timecode::format` does; the tests beside this
 * file assert the same cases as the Rust ones.
 */
export function formatTimecode(frame: number, frameRate: Rational): string {
  const rate =
    frameRate.den > 0
      ? Math.max(1, Math.round(frameRate.num / frameRate.den))
      : 30;
  const whole = Math.max(0, Math.floor(frame));
  const frames = whole % rate;
  const seconds = Math.floor(whole / rate);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(Math.floor(seconds / 3600))}:${pad(Math.floor(seconds / 60) % 60)}:${pad(seconds % 60)}:${pad(frames)}`;
}
