/**
 * Colour token parsing and WCAG contrast measurement.
 *
 * Issue #15 requires a documented contrast table and an automated test that
 * fails the build when a pair drops below its threshold. Both read the same
 * source — `tokens.css` — through this module, so the table cannot drift from
 * the tokens it describes. A hand-maintained table is a table that is wrong by
 * the second edit.
 *
 * Contrast maths is WCAG 2.2 §1.4.3 / §1.4.11: relative luminance from
 * linearised sRGB, ratio `(L_lighter + 0.05) / (L_darker + 0.05)`.
 */

/** The floor a pair must clear, by the job the pair does. */
export const WCAG_AA = {
  /** §1.4.3 — body text and anything below 18.66px / 24px bold. */
  normalText: 4.5,
  /** §1.4.3 — large text, at or above 18.66px bold / 24px regular. */
  largeText: 3,
  /**
   * §1.4.11 — the boundary or state indicator that makes a user interface
   * component identifiable, and any graphic needed to understand the content.
   */
  nonText: 3,
} as const;

/** A colour resolved to its three 8-bit sRGB channels. */
interface Rgb {
  readonly r: number;
  readonly g: number;
  readonly b: number;
}

const HEX_PATTERN = /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i;
const VAR_PATTERN = /^var\(\s*(--[\w-]+)\s*\)$/;
/* Declarations inside the `:root` block. Comments are stripped before this
   runs, so a commented-out token — the light theme sketch at the foot of
   tokens.css — cannot enter the table. */
const DECLARATION_PATTERN = /(--[\w-]+)\s*:\s*([^;]+);/g;

/**
 * Read every custom property declared in a stylesheet, verbatim.
 *
 * Values are not resolved here: `--surface: var(--neutral-3)` comes back as
 * the `var(...)` text, because the two-layer structure is the thing under test
 * and flattening it on read would hide a layer-2 token that pointed at nothing.
 */
export function parseTokens(css: string): Map<string, string> {
  const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const tokens = new Map<string, string>();

  for (const match of withoutComments.matchAll(DECLARATION_PATTERN)) {
    const [, name, value] = match;
    if (name === undefined || value === undefined) continue;
    tokens.set(name, value.trim());
  }

  return tokens;
}

/**
 * Follow a token through the `var()` chain to the literal colour underneath.
 *
 * Throws rather than returning a fallback. A token that cannot be resolved is
 * a token that renders as nothing in the application, and a test that quietly
 * substituted black for it would report a contrast ratio for a colour no user
 * will ever see.
 */
export function resolveToken(
  tokens: ReadonlyMap<string, string>,
  name: string,
): string {
  const seen = new Set<string>();
  let current = name;

  for (;;) {
    if (seen.has(current)) {
      throw new Error(`Token ${name} resolves in a cycle at ${current}`);
    }
    seen.add(current);

    const value = tokens.get(current);
    if (value === undefined) {
      throw new Error(`Token ${current} is not declared (following ${name})`);
    }

    const reference = VAR_PATTERN.exec(value);
    if (reference?.[1] === undefined) return value;
    current = reference[1];
  }
}

function parseHex(colour: string): Rgb {
  if (!HEX_PATTERN.test(colour)) {
    throw new Error(`Not a hex colour: ${colour}`);
  }

  const digits = colour.slice(1);
  const expanded =
    digits.length === 3
      ? digits
          .split("")
          .map((d) => `${d}${d}`)
          .join("")
      : digits;

  return {
    r: Number.parseInt(expanded.slice(0, 2), 16),
    g: Number.parseInt(expanded.slice(2, 4), 16),
    b: Number.parseInt(expanded.slice(4, 6), 16),
  };
}

/** Linearise one 8-bit sRGB channel. WCAG 2.2, relative luminance. */
function linearise(channel: number): number {
  const value = channel / 255;
  return value <= 0.04045
    ? value / 12.92
    : Math.pow((value + 0.055) / 1.055, 2.4);
}

/** Relative luminance of a hex colour, 0 (black) to 1 (white). */
export function relativeLuminance(colour: string): number {
  const { r, g, b } = parseHex(colour);
  return 0.2126 * linearise(r) + 0.7152 * linearise(g) + 0.0722 * linearise(b);
}

/**
 * WCAG contrast ratio between two hex colours, 1 to 21.
 *
 * Order-independent: the brighter of the two is always the numerator, which is
 * why a foreground/background pair can be passed either way round.
 */
export function contrastRatio(foreground: string, background: string): number {
  const a = relativeLuminance(foreground);
  const b = relativeLuminance(background);
  const lighter = Math.max(a, b);
  const darker = Math.min(a, b);
  return (lighter + 0.05) / (darker + 0.05);
}

/**
 * Round a ratio the way the documented table states it.
 *
 * Truncates rather than rounds to nearest: a pair measuring 4.4996 must not be
 * published as "4.50" next to a 4.5 threshold it does not actually meet.
 */
export function formatRatio(ratio: number): string {
  return (Math.floor(ratio * 100) / 100).toFixed(2);
}
