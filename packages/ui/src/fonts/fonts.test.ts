import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { FontParseError, readWoff2Advances } from "./fontMetrics.js";

/**
 * The bundled typefaces, asserted rather than trusted (#16).
 *
 * Three properties, each of which fails silently without a test:
 *
 *  1. Timecode digits are all the same width. This is the acceptance criterion
 *     "timecode digits do not change width during playback", and it is checked
 *     against the shipped font binary rather than against the words in a
 *     stylesheet. Swapping the family for a proportional one fails here.
 *  2. The fonts are local. A `src: url(https://…)` anywhere in `fonts.css`
 *     means an interface with no typeface on a machine with no network, which
 *     is the failure the whole bundling decision exists to prevent.
 *  3. The files are the files whose provenance is written down. A font replaced
 *     without updating its record is a supply-chain change nobody reviewed.
 *
 * The test reaches the font file the way the browser does — by resolving the
 * `src: url(…)` out of `fonts.css` — so renaming a file without updating the
 * stylesheet fails rather than quietly leaving the browser with nothing.
 */

/* Two steps rather than `new URL("./x", import.meta.url)`: Vite rewrites that
   exact pattern into an asset reference at transform time, and the test then
   holds an http URL that no filesystem call can open. Same reason as
   tokens.test.ts, same fix. */
const HERE = dirname(fileURLToPath(import.meta.url));

const FONTS_CSS = readFileSync(join(HERE, "fonts.css"), "utf8");
const FONTS_README = readFileSync(join(HERE, "README.md"), "utf8");
const SCALES_CSS = readFileSync(
  join(HERE, "..", "tokens", "scales.css"),
  "utf8",
);

/** Comments hold example URLs and prose about families; none of it is a rule. */
const withoutComments = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, "");

interface FontFace {
  readonly family: string;
  readonly url: string;
}

/** Every `@font-face` in the stylesheet, as the family it declares and the file it points at. */
function parseFontFaces(css: string): FontFace[] {
  const faces: FontFace[] = [];
  for (const block of withoutComments(css).matchAll(
    /@font-face\s*\{([^}]*)\}/g,
  )) {
    const body = block[1] ?? "";
    const family = /font-family:\s*"([^"]+)"/.exec(body)?.[1];
    const url = /url\(\s*"([^"]+)"\s*\)/.exec(body)?.[1];
    if (family === undefined || url === undefined) continue;
    faces.push({ family, url });
  }
  return faces;
}

const FACES = parseFontFaces(FONTS_CSS);

/** The family a token names first, unquoted. `--type-family-mono` resolves to "JetBrains Mono". */
function firstFamilyOf(token: string): string {
  const declaration = new RegExp(`${token}:\\s*([^;]+);`).exec(
    withoutComments(SCALES_CSS),
  );
  const value = declaration?.[1];
  if (value === undefined)
    throw new Error(`${token} is not declared in scales.css`);
  const first = value.split(",")[0]?.trim() ?? "";
  return first.replace(/^"|"$/g, "");
}

/**
 * Everything a timecode is made of.
 *
 * The digits are the obvious half. The separators matter just as much: a
 * timecode reads `01:23:45;12`, and a colon one unit narrower than a semicolon
 * moves every digit to its right the moment the project changes between
 * drop-frame and non-drop-frame. The full stop is there for the millisecond
 * form the export progress uses.
 */
const TIMECODE_CHARACTERS = [..."0123456789:;."];
const codePointOf = (character: string): number => {
  const point = character.codePointAt(0);
  if (point === undefined)
    throw new Error(`empty character in the timecode set`);
  return point;
};

describe("bundled typefaces", () => {
  it("declares exactly the two families the scales name", () => {
    expect(FACES.map((face) => face.family).sort()).toEqual([
      "Inter",
      "JetBrains Mono",
    ]);
    expect(firstFamilyOf("--type-family-sans")).toBe("Inter");
    expect(firstFamilyOf("--type-family-mono")).toBe("JetBrains Mono");
  });

  it("loads every face from a local file, never from a network", () => {
    expect(FACES).not.toHaveLength(0);
    for (const face of FACES) {
      expect(
        face.url.startsWith("./"),
        `${face.family} is loaded from ${face.url}`,
      ).toBe(true);
    }
    /* Belt and braces over the whole stylesheet, including anything a future
       edit adds outside an `@font-face` block. `tools/offline-assets-gate.mjs`
       makes the same assertion over every stylesheet in the repository; this
       one keeps it local to the file that would hurt most. */
    expect(withoutComments(FONTS_CSS)).not.toMatch(/https?:/);
  });

  it("ships the file each face points at", () => {
    for (const face of FACES) {
      const file = readFileSync(join(HERE, face.url));
      expect(file.byteLength, `${face.family} file is empty`).toBeGreaterThan(
        0,
      );
      // 'wOF2'. A WOFF or a bare TTF renamed to .woff2 would load in no browser.
      expect(file.subarray(0, 4).toString("ascii")).toBe("wOF2");
    }
  });

  it("matches the provenance recorded beside it", () => {
    /* The README states where each file came from and its SHA-256. A font is a
       binary that ships to users, so "which bytes are these" must have an
       answer that is not somebody's memory of a download. */
    for (const face of FACES) {
      const fileName = face.url.replace(/^\.\//, "");
      const digest = createHash("sha256")
        .update(readFileSync(join(HERE, fileName)))
        .digest("hex");
      expect(
        FONTS_README,
        `${fileName}: SHA-256 ${digest} is not recorded in README.md`,
      ).toContain(`${digest}  ${fileName}`);
    }
  });
});

describe("timecode stability", () => {
  const monoFamily = firstFamilyOf("--type-family-mono");
  const monoFace = FACES.find((face) => face.family === monoFamily);

  it("has a bundled face for the family the timecode role names", () => {
    expect(monoFace, `no @font-face declares ${monoFamily}`).toBeDefined();
  });

  it("advances every digit and separator identically", () => {
    /* The acceptance criterion, measured. `01:11:11;11` and `04:23:45;12`
       occupy the same width to the font unit, so a running timecode cannot
       shift the controls beside it. */
    const file = readFileSync(join(HERE, monoFace?.url ?? ""));
    const { unitsPerEm, advanceWidths } = readWoff2Advances(
      file,
      TIMECODE_CHARACTERS.map(codePointOf),
    );

    const measured = TIMECODE_CHARACTERS.map((character) => ({
      character,
      advance: advanceWidths.get(codePointOf(character)),
    }));

    const [first] = measured;
    expect(first).toBeDefined();
    const expected = first?.advance;
    expect(expected).toBeGreaterThan(0);

    for (const { character, advance } of measured) {
      expect(
        advance,
        `'${character}' advances ${String(advance)} of ${String(unitsPerEm)} units, ` +
          `not ${String(expected)} — the timecode will shift as it counts`,
      ).toBe(expected);
    }
  });

  it("sets tabular figures on the timecode role, for the fallback case", () => {
    /* The family already guarantees the advance. This guarantees it survives a
       substitution: a filename outside the Latin subset can push a run of text
       onto a system font, and the system UI font's figures are proportional. */
    const rule =
      /\.type-timecode\s*\{([^}]*)\}/.exec(withoutComments(SCALES_CSS))?.[1] ??
      "";
    expect(rule).toMatch(/font-family:\s*var\(--type-family-mono\)/);
    expect(rule).toMatch(/font-variant-numeric:\s*tabular-nums/);
  });
});

describe("the font parser", () => {
  /* The parser is the instrument every assertion above is made with. An
     instrument that answers confidently on rubbish makes every one of them
     worthless, so it is checked for refusing rather than guessing. */

  it("rejects a file that is not WOFF2", () => {
    expect(() =>
      readWoff2Advances(Buffer.from("not a font at all"), [0x30]),
    ).toThrow(FontParseError);
  });

  it("rejects a truncated file rather than reading past the end", () => {
    const file = readFileSync(join(HERE, FACES[0]?.url ?? ""));
    expect(() => readWoff2Advances(file.subarray(0, 40), [0x30])).toThrow(
      FontParseError,
    );
  });

  it("refuses a code point the font has no glyph for", () => {
    /* A missing glyph must not read as an advance of zero: "the font has no
       digit" and "the digit is zero wide" are different facts, and only one of
       them should let the suite above pass. U+4E00 is CJK and outside the
       Latin subset we ship. */
    const file = readFileSync(join(HERE, FACES[0]?.url ?? ""));
    expect(() => readWoff2Advances(file, [0x4e00])).toThrow(
      /no glyph for U\+4E00/,
    );
  });
});
