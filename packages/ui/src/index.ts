/**
 * The Blinkify design system.
 *
 * Boundary rule (`CLAUDE.md` section 2, enforced by `ui-package-stays-shared`
 * in #11): this package may not import from `apps/`. A design system that
 * reaches back into an application is not a design system — it is that
 * application with extra steps.
 *
 * The tokens themselves are CSS, not TypeScript, and are consumed by importing
 * `@blinkify/ui/tokens.css`. What is exported here is the machinery that proves
 * they are usable: contrast measurement, and the list of pairs that have to
 * hold. Primitives land in #17.
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
