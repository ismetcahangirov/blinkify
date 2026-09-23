/**
 * The transport's glyphs: 16-unit outlines in the current colour, drawn to
 * the same grid and stroke as the shell's own (`AppBar`).
 */

const STROKE = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.3,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

export function PlayGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M5 3.5v9l7.5-4.5z" fill="currentColor" />
    </svg>
  );
}

export function PauseGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M5.5 3.5v9M10.5 3.5v9" {...STROKE} strokeWidth={2} />
    </svg>
  );
}

export function StopGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <rect x="4" y="4" width="8" height="8" rx="1" fill="currentColor" />
    </svg>
  );
}

export function StepBackGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M10.5 4L6 8l4.5 4" {...STROKE} />
      <path d="M4.5 4v8" {...STROKE} />
    </svg>
  );
}

export function StepForwardGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M5.5 4L10 8l-4.5 4" {...STROKE} />
      <path d="M11.5 4v8" {...STROKE} />
    </svg>
  );
}

export function JumpStartGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M3.5 3.5v9M12.5 4L8 8l4.5 4M8.5 4L4 8l4.5 4" {...STROKE} />
    </svg>
  );
}

export function JumpEndGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path d="M12.5 3.5v9M3.5 4L8 8l-4.5 4M7.5 4L12 8l-4.5 4" {...STROKE} />
    </svg>
  );
}

export function LoopGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M3 8.5V7a2.5 2.5 0 0 1 2.5-2.5H12M10 2.5l2 2-2 2M13 7.5V9a2.5 2.5 0 0 1-2.5 2.5H4M6 13.5l-2-2 2-2"
        {...STROKE}
      />
    </svg>
  );
}
