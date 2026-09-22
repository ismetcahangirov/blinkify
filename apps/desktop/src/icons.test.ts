import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * The application icon and the first-paint background (#20).
 *
 * Two properties, and both of them fail silently:
 *
 *  1. The `.ico` actually contains the nine sizes Windows asks for, and the two
 *     smallest are the simplified mark. A missing size does not error — Windows
 *     scales the nearest one and the result is a soft, wrong-looking icon that
 *     nobody traces back to a build script.
 *  2. The three places that decide what colour the window is before the
 *     interface paints all agree. They are separated by a process boundary and
 *     cannot read each other, so nothing but a test can hold them together —
 *     and the symptom of them disagreeing is a white flash on every launch,
 *     which is the sort of thing people stop noticing after a week.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const APP = join(HERE, "..");
const ROOT = join(APP, "..", "..");

const ICO = readFileSync(join(APP, "src-tauri", "icons", "icon.ico"));
const TAURI_CONF = readFileSync(
  join(APP, "src-tauri", "tauri.conf.json"),
  "utf8",
);
const INDEX_HTML = readFileSync(join(APP, "index.html"), "utf8");
const TOKENS = readFileSync(
  join(ROOT, "packages", "ui", "src", "tokens", "tokens.css"),
  "utf8",
);
const GENERATOR = readFileSync(
  join(ROOT, "tools", "generate-icons.mjs"),
  "utf8",
);

/** The sizes the generator commits to, and the two that are simplified. */
const EXPECTED_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const SIMPLIFIED = [16, 20];

interface IconEntry {
  readonly size: number;
  readonly bytes: number;
  readonly isPng: boolean;
}

/** Read the `.ico` directory. The format is a header and one entry per image. */
function readIco(file: Buffer): IconEntry[] {
  expect(file.readUInt16LE(0), "reserved field").toBe(0);
  expect(file.readUInt16LE(2), "resource type, 1 = icon").toBe(1);

  const count = file.readUInt16LE(4);
  const entries: IconEntry[] = [];

  for (let index = 0; index < count; index += 1) {
    const at = 6 + index * 16;
    // A zero in the width byte means 256: the field is one byte wide.
    const size = file.readUInt8(at) === 0 ? 256 : file.readUInt8(at);
    const bytes = file.readUInt32LE(at + 8);
    const offset = file.readUInt32LE(at + 12);
    const signature = file.subarray(offset, offset + 8).toString("hex");
    entries.push({ size, bytes, isPng: signature === "89504e470d0a1a0a" });
  }

  return entries;
}

const entries = readIco(ICO);

/** `--background`, resolved through the neutral ramp it points at. */
function backgroundToken(): string {
  const semantic = /--background:\s*var\((--[\w-]+)\)/.exec(TOKENS)?.[1];
  expect(semantic, "--background is not a var() reference").toBeDefined();
  const value = new RegExp(`${semantic ?? ""}:\\s*(#[0-9a-f]{6})`).exec(
    TOKENS,
  )?.[1];
  expect(value, `${String(semantic)} has no hex value`).toBeDefined();
  return (value ?? "").toLowerCase();
}

describe("the application icon", () => {
  it("carries every size Windows asks for", () => {
    expect(entries.map((entry) => entry.size)).toEqual(EXPECTED_SIZES);
  });

  it("stores each size as a PNG", () => {
    /* Windows has read PNG-compressed icon entries since Vista, and the
       alternative is four times the bytes for the same pixels. If an entry ever
       stops being a PNG it is because something rewrote the container. */
    for (const entry of entries) {
      expect(entry.isPng, `${String(entry.size)}px entry`).toBe(true);
    }
  });

  it("draws the two smallest sizes from the simplified mark", () => {
    /* At 16px the play triangle is four pixels of mud — rendered and looked at,
       not assumed. #20's rule for that case: drop the inner triangle before the
       silhouette.

       The simplified mark has one shape where the full one has two, so it
       compresses smaller at the same dimensions. That is an indirect test, and
       it is the only one available without decoding the pixels: what it really
       guards is somebody pointing both entries at the same file. */
    const bySize = new Map(entries.map((entry) => [entry.size, entry.bytes]));
    const simplified = SIMPLIFIED.map((size) => bySize.get(size) ?? 0);
    const full = bySize.get(24) ?? 0;

    for (const bytes of simplified) {
      expect(bytes).toBeGreaterThan(0);
    }
    expect(Math.max(...simplified)).toBeLessThan(full);
  });

  it("is referenced by the bundler", () => {
    const config = JSON.parse(TAURI_CONF) as {
      bundle: {
        icon: string[];
        windows: { nsis: { headerImage: string; sidebarImage: string } };
      };
    };

    expect(config.bundle.icon).toContain("icons/icon.ico");
    /* The macOS icon is deliberately absent. Blinkify bundles NSIS only, and a
       stale template placeholder for a platform we do not build is worse than
       no file at all. */
    expect(config.bundle.icon.join(" ")).not.toContain(".icns");

    expect(config.bundle.windows.nsis.headerImage).toBe("installer/header.bmp");
    expect(config.bundle.windows.nsis.sidebarImage).toBe(
      "installer/sidebar.bmp",
    );
  });
});

describe("the first paint is dark", () => {
  const background = backgroundToken();

  it("has a token to agree with", () => {
    expect(background).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("is the window's background before the WebView exists", () => {
    /* Tauri paints this while the WebView is still starting. Without it the
       window is white for as long as the process takes to boot, which is the
       longest and most visible part of the flash. */
    const configured = /"backgroundColor":\s*"(#[0-9a-fA-F]{6})"/.exec(
      TAURI_CONF,
    )?.[1];
    expect(configured?.toLowerCase()).toBe(background);
  });

  it("is the document's background before the stylesheet arrives", () => {
    /* And this covers the second gap: the WebView has started, the document is
       parsing, and no CSS file has been fetched yet. */
    const inline = /background:\s*(#[0-9a-fA-F]{6})/.exec(INDEX_HTML)?.[1];
    expect(inline?.toLowerCase()).toBe(background);
  });

  it("is the background the installer artwork is drawn on", () => {
    /* An installer whose artwork is a different black from the application it
       installs is a detail nobody can name and everybody notices. */
    const generator =
      /const BACKGROUND = \{ r: (0x[0-9a-f]{2}), g: (0x[0-9a-f]{2}), b: (0x[0-9a-f]{2})/.exec(
        GENERATOR,
      );
    expect(
      generator,
      "the generator's BACKGROUND is not in the expected shape",
    ).not.toBeNull();

    const hex =
      "#" +
      [generator?.[1], generator?.[2], generator?.[3]]
        .map((part) => Number(part).toString(16).padStart(2, "0"))
        .join("");
    expect(hex).toBe(background);
  });
});
