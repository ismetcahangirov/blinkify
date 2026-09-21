/**
 * Every token pair that carries a contrast duty, and the duty it carries.
 *
 * This list is the contract behind `docs/design/colour-tokens.md`. The test
 * measures each pair from `tokens.css` and asserts two things: the pair clears
 * its threshold, and the documented table states the measured number. A pair
 * that is not in this list does not appear in the table, so adding a
 * user-readable colour combination without adding it here is the one way to
 * evade the gate — which is why review looks for it.
 *
 * What is deliberately absent, and why:
 *
 *   `--border` — it separates one dark panel from another, and nothing about
 *   identifying a control depends on seeing it, so WCAG §1.4.11 does not apply.
 *   Forcing it to 3:1 would draw a heavier line than the interface wants.
 *   `--border-strong` is the token that outlines controls, and it is measured.
 *
 *   `--focus-ring` against `--accent` — it measures 1.96:1 and could not be
 *   fixed by choosing a different colour, because a ring bright enough to sit
 *   on a saturated brand fill is one that vanishes on the dark surface next to
 *   it. It is fixed by geometry instead: `--focus-ring-offset` is non-zero, so
 *   the ring is drawn on the surface behind the control rather than against the
 *   control, and the pairs that exist are the four measured below.
 */

/** What a pair is used for, which is what sets its threshold. */
export type ContrastDuty =
  /** Body-sized text a user reads. WCAG §1.4.3, 4.5:1. */
  | "text"
  /** A component boundary, state indicator or meaningful graphic. §1.4.11, 3:1. */
  | "non-text"
  /**
   * Text inside an inactive component. Exempt from §1.4.3 entirely, held to
   * 3:1 here anyway — see `tokens.css`, `--text-disabled`.
   */
  | "disabled-text";

export interface ContrastPair {
  /** Semantic token in the foreground. */
  readonly foreground: string;
  /** Semantic token behind it. */
  readonly background: string;
  /** What the combination is for, in the words a designer would use. */
  readonly usage: string;
  readonly duty: ContrastDuty;
}

/** The floor for each duty, in ratio. */
export const DUTY_THRESHOLD: Record<ContrastDuty, number> = {
  text: 4.5,
  "non-text": 3,
  "disabled-text": 3,
};

export const CONTRAST_PAIRS: readonly ContrastPair[] = [
  /* Primary text, on every surface it can land on. */
  {
    foreground: "--text-primary",
    background: "--background",
    usage: "Body text on the application background",
    duty: "text",
  },
  {
    foreground: "--text-primary",
    background: "--surface",
    usage: "Body text on a panel",
    duty: "text",
  },
  {
    foreground: "--text-primary",
    background: "--surface-raised",
    usage: "Body text on a card, input or toolbar",
    duty: "text",
  },
  {
    foreground: "--text-primary",
    background: "--surface-overlay",
    usage: "Body text in a menu, popover or dialog",
    duty: "text",
  },

  /* Secondary text. The pair most likely to fail quietly on a dark theme. */
  {
    foreground: "--text-secondary",
    background: "--background",
    usage: "Supporting text on the application background",
    duty: "text",
  },
  {
    foreground: "--text-secondary",
    background: "--surface",
    usage: "Supporting text on a panel",
    duty: "text",
  },
  {
    foreground: "--text-secondary",
    background: "--surface-raised",
    usage: "Supporting text on a card or toolbar",
    duty: "text",
  },
  {
    foreground: "--text-secondary",
    background: "--surface-overlay",
    usage: "Supporting text in a menu or dialog",
    duty: "text",
  },

  /* Accent-coloured text and icons. */
  {
    foreground: "--text-accent",
    background: "--background",
    usage: "Link or accent icon on the application background",
    duty: "text",
  },
  {
    foreground: "--text-accent",
    background: "--surface",
    usage: "Link or accent icon on a panel",
    duty: "text",
  },
  {
    foreground: "--text-accent",
    background: "--surface-raised",
    usage: "Link or accent icon on a card or toolbar",
    duty: "text",
  },
  {
    foreground: "--text-accent",
    background: "--surface-overlay",
    usage: "Link or accent icon in a menu or dialog",
    duty: "text",
  },

  /* Text on the accent fill, through the full interaction cycle. */
  {
    foreground: "--accent-foreground",
    background: "--accent",
    usage: "Label on the primary action",
    duty: "text",
  },
  {
    foreground: "--accent-foreground",
    background: "--accent-hover",
    usage: "Label on the primary action, hovered",
    duty: "text",
  },
  {
    foreground: "--accent-foreground",
    background: "--accent-pressed",
    usage: "Label on the primary action, pressed",
    duty: "text",
  },

  /* State text. Measured on the panel and on the raised surface, because a
     status line and a dialog both carry it. */
  {
    foreground: "--success",
    background: "--surface",
    usage: "Success message on a panel",
    duty: "text",
  },
  {
    foreground: "--success",
    background: "--surface-raised",
    usage: "Success message on a card",
    duty: "text",
  },
  {
    foreground: "--warning",
    background: "--surface",
    usage: "Warning message on a panel",
    duty: "text",
  },
  {
    foreground: "--warning",
    background: "--surface-raised",
    usage: "Warning message on a card",
    duty: "text",
  },
  {
    foreground: "--danger",
    background: "--surface",
    usage: "Error message on a panel",
    duty: "text",
  },
  {
    foreground: "--danger",
    background: "--surface-raised",
    usage: "Error message on a card",
    duty: "text",
  },
  {
    foreground: "--lossless",
    background: "--surface",
    usage: "Lossless indicator on a panel",
    duty: "text",
  },
  {
    foreground: "--lossless",
    background: "--surface-raised",
    usage: "Lossless indicator in the export dialog",
    duty: "text",
  },

  /* Timeline text. */
  {
    foreground: "--timeline-clip-label",
    background: "--timeline-clip-video",
    usage: "Clip name on a video clip",
    duty: "text",
  },
  {
    foreground: "--timeline-clip-label",
    background: "--timeline-clip-audio",
    usage: "Clip name on an audio clip",
    duty: "text",
  },
  {
    foreground: "--timeline-ruler-text",
    background: "--timeline-track",
    usage: "Timecode on the timeline ruler",
    duty: "text",
  },

  /* Disabled text. Exempt from §1.4.3; held to 3:1 by project rule. */
  {
    foreground: "--text-disabled",
    background: "--surface",
    usage: "Label on a disabled control on a panel",
    duty: "disabled-text",
  },
  {
    foreground: "--text-disabled",
    background: "--surface-raised",
    usage: "Label on a disabled control on a card",
    duty: "disabled-text",
  },
  {
    foreground: "--text-disabled",
    background: "--surface-overlay",
    usage: "Label on a disabled item in a menu",
    duty: "disabled-text",
  },

  /* Component boundaries and indicators. */
  {
    foreground: "--border-strong",
    background: "--surface",
    usage: "Input or control outline on a panel",
    duty: "non-text",
  },
  {
    foreground: "--border-strong",
    background: "--surface-raised",
    usage: "Input or control outline on a card",
    duty: "non-text",
  },
  {
    foreground: "--focus-ring",
    background: "--background",
    usage: "Focus ring against the application background",
    duty: "non-text",
  },
  {
    foreground: "--focus-ring",
    background: "--surface",
    usage: "Focus ring against a panel",
    duty: "non-text",
  },
  {
    foreground: "--focus-ring",
    background: "--surface-raised",
    usage: "Focus ring against a card or toolbar",
    duty: "non-text",
  },
  {
    foreground: "--focus-ring",
    background: "--surface-overlay",
    usage: "Focus ring against a menu or dialog",
    duty: "non-text",
  },
  {
    foreground: "--accent",
    background: "--background",
    usage: "Edge of the primary action against the background",
    duty: "non-text",
  },
  {
    foreground: "--accent-hover",
    background: "--background",
    usage: "Edge of the primary action, hovered",
    duty: "non-text",
  },
  {
    foreground: "--accent-pressed",
    background: "--background",
    usage: "Edge of the primary action, pressed",
    duty: "non-text",
  },

  /* Timeline geometry. A clip the user cannot pick out of the track is not a
     clip they can edit. */
  {
    foreground: "--timeline-clip-video-border",
    background: "--timeline-track",
    usage: "Video clip edge against the track bed",
    duty: "non-text",
  },
  {
    foreground: "--timeline-clip-audio-border",
    background: "--timeline-track",
    usage: "Audio clip edge against the track bed",
    duty: "non-text",
  },
  {
    foreground: "--timeline-clip-selected-border",
    background: "--timeline-track",
    usage: "Selected clip edge against the track bed",
    duty: "non-text",
  },
  {
    foreground: "--timeline-playhead",
    background: "--timeline-track",
    usage: "Playhead against the track bed",
    duty: "non-text",
  },
];
