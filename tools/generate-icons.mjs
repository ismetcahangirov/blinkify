#!/usr/bin/env node
/**
 * Generate every icon Windows asks for, from two SVGs.
 *
 * Issue #20: "source artwork committed as SVG, with the raster sizes generated
 * by a committed script rather than by hand", and "re-running the generation
 * script reproduces every raster byte-identically".
 *
 * The reason is one commit versus nine exported files. A mark that is refined
 * six months from now should be a change to one vector and a re-run of this,
 * not an afternoon in an image editor followed by the discovery that the 40px
 * variant was missed.
 *
 * ── Two source files, and why ───────────────────────────────────────────────
 *
 *   blinkify-mark.svg        the full mark: eye, iris, play triangle
 *   blinkify-mark-small.svg  the eye alone, 10% larger
 *
 * At 16px the play triangle is four pixels of mud — this was rendered and
 * looked at rather than assumed. #20 states the rule for that case: if
 * something must be dropped at small sizes, drop the inner play triangle
 * before the silhouette. So the two smallest entries in the `.ico` are drawn
 * from the simplified mark, and everything from 24px up uses the full one.
 *
 * ── Determinism ─────────────────────────────────────────────────────────────
 *
 * Nothing here reads a clock, a random source or the filesystem's ordering.
 * libvips renders the same SVG to the same bytes, the ICO container is
 * assembled field by field, and the sizes are a literal list. `pnpm
 * icons:check` re-runs the whole thing and fails if a single byte moved, which
 * is what makes "regenerate and commit" a safe instruction rather than a
 * source of noise in every diff.
 *
 * Usage:
 *   node tools/generate-icons.mjs            write the icons
 *   node tools/generate-icons.mjs --check    fail if what is committed differs
 */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");

const BRAND = join(ROOT, "packages", "ui", "src", "brand");
const FULL_MARK = join(BRAND, "blinkify-mark.svg");
const SMALL_MARK = join(BRAND, "blinkify-mark-small.svg");

const ICONS = join(ROOT, "apps", "desktop", "src-tauri", "icons");
const INSTALLER = join(ROOT, "apps", "desktop", "src-tauri", "installer");

/**
 * The application background, `--background` in the token file.
 *
 * Duplicated here as a literal because a build script cannot read a CSS custom
 * property, and `icons.test.ts` asserts the two agree. An installer whose
 * artwork is a different black from the application it installs is a detail
 * nobody can name and everybody notices.
 */
const BACKGROUND = { r: 0x0a, g: 0x0c, b: 0x10, alpha: 1 };

/**
 * The sizes Windows asks an `.ico` for.
 *
 * 16 and 20 come from the simplified mark. The rest of the list is not
 * arbitrary: 16 and 32 are the file list and the title bar, 24 and 48 are
 * Explorer's medium views, 40 is the 125% scaling of 32, 64 and 128 are large
 * icons, and 256 is what the Store and the extra-large view use.
 */
const ICO_SIZES = [
  { size: 16, simplified: true },
  { size: 20, simplified: true },
  { size: 24, simplified: false },
  { size: 32, simplified: false },
  { size: 40, simplified: false },
  { size: 48, simplified: false },
  { size: 64, simplified: false },
  { size: 128, simplified: false },
  { size: 256, simplified: false },
];

/** The PNG set Tauri's bundler and the Store manifests expect. */
const PNG_SET = [
  ["32x32.png", 32],
  ["128x128.png", 128],
  ["128x128@2x.png", 256],
  ["icon.png", 512],
  ["Square30x30Logo.png", 30],
  ["Square44x44Logo.png", 44],
  ["Square71x71Logo.png", 71],
  ["Square89x89Logo.png", 89],
  ["Square107x107Logo.png", 107],
  ["Square142x142Logo.png", 142],
  ["Square150x150Logo.png", 150],
  ["Square284x284Logo.png", 284],
  ["Square310x310Logo.png", 310],
  ["StoreLogo.png", 50],
];

/** NSIS wants bitmaps, at exactly these dimensions. */
const NSIS_HEADER = { name: "header.bmp", width: 150, height: 57 };
const NSIS_SIDEBAR = { name: "sidebar.bmp", width: 164, height: 314 };

/**
 * Render an SVG to a transparent PNG buffer at one size.
 *
 * `palette: false` is load-bearing and was found by the build failing rather
 * than by reading a specification. libvips picks an indexed palette whenever it
 * saves bytes, which it does for every icon under 48px — and Tauri's icon
 * decoder rejects an indexed PNG inside an `.ico` outright:
 *
 *     failed to decode icon icon.ico: Unsupported PNG color type: Indexed
 *
 * So every entry is written as 8-bit truecolour with alpha. It costs a few
 * hundred bytes across the whole file and it is the difference between the
 * application building and not.
 */
async function renderPng(source, size) {
  return sharp(source)
    .resize(size, size, {
      fit: "contain",
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    })
    .png({ compressionLevel: 9, effort: 10, palette: false })
    .toBuffer();
}

/**
 * Assemble a multi-resolution `.ico`.
 *
 * Written out by hand rather than with a library: the container is a six-byte
 * header, a sixteen-byte directory entry per image and the PNG payloads, and
 * `CLAUDE.md` section 10 rule 3 is explicit that a dependency for what fits in
 * forty lines is not worth the supply chain.
 *
 * Every entry is a PNG rather than a DIB. Windows has read PNG-compressed icon
 * entries since Vista, and the alternative — a bottom-up BGRA bitmap plus an
 * AND mask per size — is four times the bytes for the same pixels.
 */
function buildIco(images) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // 1 = icon
  header.writeUInt16LE(images.length, 4);

  const directory = Buffer.alloc(16 * images.length);
  let offset = header.length + directory.length;

  images.forEach((image, index) => {
    const at = index * 16;
    // 256 is stored as 0: the field is one byte and 256 does not fit in it.
    directory.writeUInt8(image.size === 256 ? 0 : image.size, at);
    directory.writeUInt8(image.size === 256 ? 0 : image.size, at + 1);
    directory.writeUInt8(0, at + 2); // palette size, 0 for truecolour
    directory.writeUInt8(0, at + 3); // reserved
    directory.writeUInt16LE(1, at + 4); // colour planes
    directory.writeUInt16LE(32, at + 6); // bits per pixel
    directory.writeUInt32LE(image.data.length, at + 8);
    directory.writeUInt32LE(offset, at + 12);
    offset += image.data.length;
  });

  return Buffer.concat([header, directory, ...images.map((i) => i.data)]);
}

/**
 * Write a 24-bit BMP, bottom-up, which is the only thing NSIS reads.
 *
 * Also hand-written, and for a better reason than the ICO: nothing in the Node
 * ecosystem writes the specific dialect NSIS wants without bringing an image
 * library along, and the format is a 54-byte header followed by padded BGR
 * rows.
 */
function buildBmp(rgb, width, height) {
  const rowStride = Math.ceil((width * 3) / 4) * 4;
  const pixels = Buffer.alloc(rowStride * height);

  for (let y = 0; y < height; y += 1) {
    // Bottom-up: the first row in the file is the last row of the image.
    const sourceRow = (height - 1 - y) * width * 3;
    const targetRow = y * rowStride;
    for (let x = 0; x < width; x += 1) {
      pixels[targetRow + x * 3] = rgb[sourceRow + x * 3 + 2] ?? 0; // B
      pixels[targetRow + x * 3 + 1] = rgb[sourceRow + x * 3 + 1] ?? 0; // G
      pixels[targetRow + x * 3 + 2] = rgb[sourceRow + x * 3] ?? 0; // R
    }
  }

  const header = Buffer.alloc(54);
  header.write("BM", 0, "ascii");
  header.writeUInt32LE(54 + pixels.length, 2);
  header.writeUInt32LE(0, 6);
  header.writeUInt32LE(54, 10);
  header.writeUInt32LE(40, 14); // BITMAPINFOHEADER
  header.writeInt32LE(width, 18);
  header.writeInt32LE(height, 22);
  header.writeUInt16LE(1, 26); // planes
  header.writeUInt16LE(24, 28); // bits per pixel
  header.writeUInt32LE(0, 30); // BI_RGB, uncompressed
  header.writeUInt32LE(pixels.length, 34);
  header.writeInt32LE(2835, 38); // 72 DPI, in pixels per metre
  header.writeInt32LE(2835, 42);
  header.writeUInt32LE(0, 46);
  header.writeUInt32LE(0, 50);

  return Buffer.concat([header, pixels]);
}

/**
 * The mark on an opaque brand-dark panel, at an arbitrary rectangle.
 *
 * The installer's artwork is opaque because NSIS bitmaps have no alpha. Dark
 * rather than white is a decision: Blinkify is a dark application and an
 * installer that flashes white before it is a small broken promise.
 */
async function renderPanel(source, width, height, markFraction) {
  const markSize = Math.round(Math.min(width, height) * markFraction);
  const mark = await sharp(source)
    .resize(markSize, markSize, {
      fit: "contain",
      background: { r: 0, g: 0, b: 0, alpha: 0 },
    })
    .png()
    .toBuffer();

  return sharp({
    create: { width, height, channels: 4, background: BACKGROUND },
  })
    .composite([{ input: mark, gravity: "centre" }])
    .removeAlpha()
    .raw()
    .toBuffer();
}

/** Everything this script produces, as path → bytes. */
async function generate() {
  const output = new Map();

  const icoImages = [];
  for (const { size, simplified } of ICO_SIZES) {
    icoImages.push({
      size,
      data: await renderPng(simplified ? SMALL_MARK : FULL_MARK, size),
    });
  }
  output.set(join(ICONS, "icon.ico"), buildIco(icoImages));

  for (const [name, size] of PNG_SET) {
    output.set(join(ICONS, name), await renderPng(FULL_MARK, size));
  }

  const header = await renderPanel(
    FULL_MARK,
    NSIS_HEADER.width,
    NSIS_HEADER.height,
    0.78,
  );
  output.set(
    join(INSTALLER, NSIS_HEADER.name),
    buildBmp(header, NSIS_HEADER.width, NSIS_HEADER.height),
  );

  const sidebar = await renderPanel(
    FULL_MARK,
    NSIS_SIDEBAR.width,
    NSIS_SIDEBAR.height,
    0.82,
  );
  output.set(
    join(INSTALLER, NSIS_SIDEBAR.name),
    buildBmp(sidebar, NSIS_SIDEBAR.width, NSIS_SIDEBAR.height),
  );

  return output;
}

async function main() {
  const check = process.argv.includes("--check");
  const files = await generate();

  const stale = [];
  for (const [path, bytes] of files) {
    if (check) {
      if (!existsSync(path) || !readFileSync(path).equals(bytes)) {
        stale.push(relative(ROOT, path).split("\\").join("/"));
      }
      continue;
    }
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, bytes);
  }

  if (check) {
    if (stale.length > 0) {
      console.error(
        `generate-icons: ${String(stale.length)} committed icon(s) do not match the source artwork\n`,
      );
      for (const path of stale) console.error(`  ${path}`);
      console.error(
        "\nThe mark changed and the rasters were not regenerated, or they were" +
          "\nedited by hand. Run `pnpm icons` and commit the result.",
      );
      process.exit(1);
    }
    console.log(
      `generate-icons: ${String(files.size)} icon(s) match the source artwork.`,
    );
    return;
  }

  console.log(
    `generate-icons: wrote ${String(files.size)} file(s) from the committed SVGs.`,
  );
}

await main();
