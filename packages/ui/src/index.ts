/**
 * The Blinkify design system.
 *
 * Boundary rule (`CLAUDE.md` section 2, enforced by `ui-package-stays-shared`
 * in #11): this package may not import from `apps/`. A design system that
 * reaches back into an application is not a design system — it is that
 * application with extra steps.
 *
 * The tokens themselves are CSS, not TypeScript, and are consumed by importing
 * `@blinkify/ui/styles.css` — one import that brings the bundled typefaces
 * (#16), the colour tokens (#15) and the type, spacing, radius, elevation and
 * motion scales (#16), in that order. The individual stylesheets are also
 * exported, for a consumer that genuinely needs one of them alone.
 *
 * What is exported here is the machinery that proves the tokens are usable:
 * contrast measurement, and the list of pairs that have to hold.
 *
 * `fonts/fontMetrics.ts` is deliberately absent from this barrel. It reads a
 * WOFF2 binary through `node:zlib` to assert the timecode digits are all one
 * width, and re-exporting it would put a Node built-in on the path of anything
 * that imports the design system. Tests import it by its own path.
 *
 * Primitives land in #17.
 */

export {
  WCAG_AA,
  contrastRatio,
  formatRatio,
  parseTokens,
  relativeLuminance,
  resolveToken,
} from "./tokens/contrast.js";

export {
  CONTRAST_PAIRS,
  DUTY_THRESHOLD,
  type ContrastDuty,
  type ContrastPair,
} from "./tokens/contrastPairs.js";

export const DESIGN_SYSTEM_VERSION = "0.1.0";
