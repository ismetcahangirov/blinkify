import type { Theme } from "./draw.js";

/**
 * The timeline's colours, read from the design tokens (#15) rather than
 * written here: a canvas cannot use `var()`, so the values are resolved once
 * from the document's computed style. No colour is spelled in this file.
 */
const TOKENS: Record<Exclude<keyof Theme, "font">, string> = {
  background: "--background",
  track: "--timeline-track",
  border: "--border",
  clipVideo: "--timeline-clip-video",
  clipVideoBorder: "--timeline-clip-video-border",
  clipAudio: "--timeline-clip-audio",
  clipAudioBorder: "--timeline-clip-audio-border",
  clipLabel: "--timeline-clip-label",
  selected: "--timeline-clip-selected-border",
  playhead: "--timeline-playhead",
  rulerText: "--timeline-ruler-text",
  warning: "--warning",
};

export function readTheme(element: Element = document.documentElement): Theme {
  const style = getComputedStyle(element);
  const read = (name: string) => style.getPropertyValue(name).trim();
  const entries = Object.entries(TOKENS).map(([key, token]) => [
    key,
    read(token),
  ]);
  const size = read("--type-caption-size") || "11px";
  const family = read("--type-family-sans") || "sans-serif";
  return {
    ...(Object.fromEntries(entries) as Omit<Theme, "font">),
    font: `${size} ${family}`,
  };
}
