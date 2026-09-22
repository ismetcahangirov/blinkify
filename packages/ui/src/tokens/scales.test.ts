import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseTokens } from "./contrast.js";

/**
 * The scales, asserted rather than trusted (#16).
 *
 * What the file next door cannot enforce by existing:
 *
 *  1. The tokens each scale promises are all there, spelled the way components
 *     will spell them. A missing `--radius-sm` is not a build error; it is a
 *     square corner nobody notices until a screenshot.
 *  2. The spacing scale is actually a 4px grid. One 6px step turns a scale into
 *     a list of values, and it will be added in good faith by someone who needs
 *     6px this once.
 *  3. Elevation is built from semantic colour roles and nothing else — the
 *     mechanism that makes a light theme a value swap rather than a rewrite.
 *  4. Motion stays inside the ceiling the issue sets, and every duration is
 *     zeroed under `prefers-reduced-motion`. This is the one that decays
 *     fastest: a new duration token is added, the reduced-motion block is three
 *     hundred lines away, and a user who asked for less motion silently gets
 *     some back.
 *  5. The documentation still describes the values that are in the file.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = join(HERE, "..", "..", "..", "..");

const SCALES_CSS = readFileSync(join(HERE, "scales.css"), "utf8");
const TOKENS_CSS = readFileSync(join(HERE, "tokens.css"), "utf8");
const SCALES_DOC = readFileSync(
  join(REPOSITORY_ROOT, "docs", "design", "scales.md"),
  "utf8",
);

const stripComments = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, "");

/**
 * Tokens from one block, not from the whole file.
 *
 * `parseTokens` reads every declaration in a stylesheet, which here would let
 * the `prefers-reduced-motion` overrides win and report `--duration-slow` as
 * 0ms. The two blocks are the thing being compared, so they are read apart.
 */
function tokensOfBlock(css: string, from: number): Map<string, string> {
  const opening = css.indexOf("{", from);
  const closing = css.indexOf("\n}", opening);
  expect(opening, "no block found").toBeGreaterThan(-1);
  expect(closing, "block is unterminated").toBeGreaterThan(opening);
  return parseTokens(css.slice(opening, closing));
}

const bare = stripComments(SCALES_CSS);
const ROOT = tokensOfBlock(bare, bare.indexOf(":root"));

const REDUCED_MOTION_AT = bare.indexOf(
  "@media (prefers-reduced-motion: reduce)",
);
const REDUCED = tokensOfBlock(bare, bare.indexOf(":root", REDUCED_MOTION_AT));

/** Semantic colour roles — layer 2 of `tokens.css`, the only colour a component may name. */
const SEMANTIC_COLOUR_TOKENS = new Set(
  [...parseTokens(stripComments(TOKENS_CSS)).keys()].filter(
    (name) => !/^--(?:neutral|brand|hue)-/.test(name),
  ),
);

/** rem, px or a unitless number, in px. Anything else is a failure to parse, not a zero. */
function toPixels(value: string): number {
  const rem = /^(-?[\d.]+)rem$/.exec(value);
  if (rem?.[1] !== undefined) return Number.parseFloat(rem[1]) * 16;
  const px = /^(-?[\d.]+)px$/.exec(value);
  if (px?.[1] !== undefined) return Number.parseFloat(px[1]);
  throw new Error(`cannot read ${JSON.stringify(value)} as a length`);
}

function toMilliseconds(value: string): number {
  const ms = /^([\d.]+)ms$/.exec(value);
  if (ms?.[1] === undefined)
    throw new Error(`cannot read ${JSON.stringify(value)} as a duration`);
  return Number.parseFloat(ms[1]);
}

const TYPE_ROLES = [
  "display",
  "title",
  "body",
  "label",
  "caption",
  "timecode",
] as const;
const SPACING_STEPS = [0, 1, 2, 3, 4, 5, 6, 8, 10, 12, 16] as const;
const RADIUS_STEPS = ["none", "sm", "md", "lg", "full"] as const;
const ELEVATION_LEVELS = [0, 1, 2, 3] as const;
const DURATIONS = ["instant", "fast", "normal", "slow"] as const;
const EASINGS = ["standard", "entrance", "exit", "linear"] as const;

describe("the type scale", () => {
  it("declares all four properties of every role", () => {
    for (const role of TYPE_ROLES) {
      for (const property of ["size", "line", "weight", "tracking"]) {
        expect(
          ROOT.has(`--type-${role}-${property}`),
          `--type-${role}-${property}`,
        ).toBe(true);
      }
    }
  });

  it("descends in size from display to caption", () => {
    /* Not decoration: a scale whose `label` outgrew its `body` is a scale
       nobody can reason about from the name alone, and the names are all a
       component author has. */
    const readable = ["display", "title", "body", "label", "caption"] as const;
    const sizes = readable.map((role) =>
      toPixels(ROOT.get(`--type-${role}-size`) ?? ""),
    );
    for (let i = 1; i < sizes.length; i += 1) {
      expect(
        sizes[i],
        `${readable[i]} is not smaller than ${readable[i - 1]}`,
      ).toBeLessThan(sizes[i - 1] ?? 0);
    }
  });

  it("gives every role a line height at least its own size", () => {
    for (const role of TYPE_ROLES) {
      const size = toPixels(ROOT.get(`--type-${role}-size`) ?? "");
      const line = toPixels(ROOT.get(`--type-${role}-line`) ?? "");
      expect(
        line,
        `--type-${role}-line clips its own glyphs`,
      ).toBeGreaterThanOrEqual(size);
    }
  });

  it("holds the caption at the legibility floor", () => {
    /* 11px is the smallest text in the product and the ruler numbers are set in
       it. Anything below this on a dark surface stops being readable at a
       normal viewing distance regardless of its contrast ratio, which is the
       part a contrast table cannot tell you. */
    expect(
      toPixels(ROOT.get("--type-caption-size") ?? ""),
    ).toBeGreaterThanOrEqual(11);
  });

  it("sets the timecode role in the mono family", () => {
    expect(ROOT.get("--type-family-mono")).toMatch(/^"JetBrains Mono"/);
    expect(bare).toMatch(
      /\.type-timecode\s*\{[^}]*font-family:\s*var\(--type-family-mono\)/,
    );
  });
});

describe("the spacing scale", () => {
  it("declares every documented step", () => {
    for (const step of SPACING_STEPS) {
      expect(
        ROOT.has(`--space-${String(step)}`),
        `--space-${String(step)}`,
      ).toBe(true);
    }
  });

  it("is a 4px grid, with no exceptions", () => {
    for (const step of SPACING_STEPS) {
      const pixels = toPixels(ROOT.get(`--space-${String(step)}`) ?? "");
      expect(
        pixels % 4,
        `--space-${String(step)} is ${String(pixels)}px, not a multiple of 4`,
      ).toBe(0);
      /* The step number IS the multiple. `--space-3` that is not 12px makes
         every mental calculation in every component wrong. */
      expect(
        pixels,
        `--space-${String(step)} should be ${String(step * 4)}px`,
      ).toBe(step * 4);
    }
  });
});

describe("the radius scale", () => {
  it("declares every documented step", () => {
    for (const step of RADIUS_STEPS) {
      expect(ROOT.has(`--radius-${step}`), `--radius-${step}`).toBe(true);
    }
  });

  it("starts at zero and ascends", () => {
    expect(toPixels(ROOT.get("--radius-none") ?? "")).toBe(0);
    const ascending = ["none", "sm", "md", "lg", "full"] as const;
    const values = ascending.map((step) =>
      toPixels(ROOT.get(`--radius-${step}`) ?? ""),
    );
    for (let i = 1; i < values.length; i += 1) {
      expect(values[i], `--radius-${ascending[i]}`).toBeGreaterThan(
        values[i - 1] ?? -1,
      );
    }
  });

  it("keeps the clickable default small enough for a tool", () => {
    /* CLAUDE.md section 17: the visual language is Blinkify's own, and it is a
       tool. A large radius on the default control is the single change that
       most makes an editor look like a consumer toy. */
    expect(toPixels(ROOT.get("--radius-md") ?? "")).toBeLessThanOrEqual(6);
  });
});

describe("the elevation scale", () => {
  it("declares a background and a border for every level", () => {
    for (const level of ELEVATION_LEVELS) {
      expect(ROOT.has(`--elevation-${String(level)}-background`)).toBe(true);
      expect(ROOT.has(`--elevation-${String(level)}-border`)).toBe(true);
    }
  });

  it("is built out of semantic colour roles and nothing else", () => {
    /* The mechanism, not the appearance. An elevation that reached for
       `--neutral-4` would keep working and would silently not follow a theme,
       which is exactly the failure the two layers exist to make impossible. */
    for (const level of ELEVATION_LEVELS) {
      for (const part of ["background", "border"] as const) {
        const name = `--elevation-${String(level)}-${part}`;
        const value = ROOT.get(name) ?? "";
        if (value === "transparent") continue;
        const referenced = /^var\((--[\w-]+)\)$/.exec(value)?.[1];
        expect(
          referenced,
          `${name} is ${value}, not a var() reference`,
        ).toBeDefined();
        expect(
          SEMANTIC_COLOUR_TOKENS.has(referenced ?? ""),
          `${name} reads ${String(referenced)}, which is not a semantic role in tokens.css`,
        ).toBe(true);
      }
    }
  });

  it("expresses depth without a shadow", () => {
    /* The issue's requirement, and it has to be asserted because adding a
       shadow is the obvious thing to reach for. On a near-black background a
       shadow darkens nothing and costs a compositing layer per raised element.
       If Blinkify ever gains a shadow it will be a decision with an ADR, not a
       property someone added to a card. */
    expect(bare).not.toMatch(/box-shadow/);
    expect(
      [...ROOT.keys()].filter((name) => name.startsWith("--shadow")),
    ).toEqual([]);
  });
});

describe("the motion scale", () => {
  it("declares every duration and every easing", () => {
    for (const duration of DURATIONS) {
      expect(ROOT.has(`--duration-${duration}`), `--duration-${duration}`).toBe(
        true,
      );
    }
    for (const easing of EASINGS) {
      expect(ROOT.has(`--easing-${easing}`), `--easing-${easing}`).toBe(true);
    }
  });

  it("stays under the ceiling an editing tool can afford", () => {
    /* The issue: anything over about 150ms on a panel or menu reads as lag,
       because the user operates the tool continuously rather than reading it.
       `--duration-slow` is allowed past that only because it is reserved for a
       dialog taking over the window — and 180ms is where that stops being
       motion and starts being a wait. */
    expect(toMilliseconds(ROOT.get("--duration-instant") ?? "")).toBe(0);
    expect(
      toMilliseconds(ROOT.get("--duration-normal") ?? ""),
    ).toBeLessThanOrEqual(150);
    expect(
      toMilliseconds(ROOT.get("--duration-slow") ?? ""),
    ).toBeLessThanOrEqual(180);

    const ordered = DURATIONS.map((name) =>
      toMilliseconds(ROOT.get(`--duration-${name}`) ?? ""),
    );
    for (let i = 1; i < ordered.length; i += 1) {
      expect(ordered[i], `--duration-${DURATIONS[i]}`).toBeGreaterThan(
        ordered[i - 1] ?? -1,
      );
    }
  });
});

describe("reduced motion", () => {
  const declaredDurations = [...ROOT.keys()].filter((name) =>
    name.startsWith("--duration-"),
  );

  it("zeroes every duration token, including ones added later", () => {
    /* Derived from the root block rather than from a list, so a new duration
       token that nobody remembered to override fails here on the commit that
       adds it. A list would need the same memory the override needs. */
    expect(declaredDurations).not.toHaveLength(0);
    for (const name of declaredDurations) {
      expect(
        REDUCED.has(name),
        `${name} is not overridden under prefers-reduced-motion`,
      ).toBe(true);
      expect(
        toMilliseconds(REDUCED.get(name) ?? ""),
        `${name} under reduced motion`,
      ).toBe(0);
    }
  });

  it("also stops motion that never read a token", () => {
    /* Radix animations, third-party markup, a keyframe written in a hurry. A
       user who asked the operating system for less motion did not ask only the
       parts of the interface that went through our tokens. */
    const block = bare.slice(REDUCED_MOTION_AT);
    expect(block).toMatch(/transition-duration:\s*1ms\s*!important/);
    expect(block).toMatch(/animation-duration:\s*1ms\s*!important/);
    expect(block).toMatch(/animation-iteration-count:\s*1\s*!important/);
  });

  it("leaves an exit for motion that carries information", () => {
    /* An indeterminate progress indicator says work is still happening and a
       frozen one says the application hung. `data-motion="essential"` is how a
       component claims that, and the claim is reviewable precisely because it
       has to be written down. */
    expect(bare.slice(REDUCED_MOTION_AT)).toContain(
      '[data-motion="essential"]',
    );
  });

  it("does not use a zero duration, which would strand a component", () => {
    /* 1ms, not 0s. A zero-duration animation never fires `animationend`, and a
       component that unmounts on that event stays on screen forever — a
       reduced-motion setting that breaks menus is one the user turns off. */
    expect(bare.slice(REDUCED_MOTION_AT)).not.toMatch(
      /animation-duration:\s*0m?s\s*!important/,
    );
  });
});

describe("the documentation", () => {
  it("states the value of every token in the scale", () => {
    /* Same contract as the colour table: the document is checked against the
       file rather than maintained beside it. A hand-kept table is wrong by its
       second edit, and a design system documented wrongly is worse than one
       documented not at all, because people believe it. */
    const undocumented: string[] = [];
    for (const [name, value] of ROOT) {
      if (!SCALES_DOC.includes(`\`${name}\``)) {
        undocumented.push(`${name} — not mentioned`);
        continue;
      }
      if (!SCALES_DOC.includes(`\`${value}\``)) {
        undocumented.push(
          `${name} — documented without its value \`${value}\``,
        );
      }
    }
    expect(undocumented, "docs/design/scales.md is out of date").toEqual([]);
  });
});
