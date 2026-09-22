/**
 * Advance widths, read from a WOFF2 font binary.
 *
 * Issue #16 requires a test that "asserts timecode glyph width stability across
 * all digits". There are two ways to write that test and only one of them is
 * worth anything.
 *
 * The cheap way asserts that the timecode style names a monospaced family and
 * sets `font-variant-numeric: tabular-nums` — which asserts that someone typed
 * the right words, not that the digits are the same width. Swap the family for
 * one that is monospaced in name only and the test still passes while the
 * timecode still jitters.
 *
 * So this module reads the font file that actually ships and reports what the
 * font itself says each glyph advances. `fonts.test.ts` then asserts the digits
 * and the timecode separators agree. That is a fact about bytes, which is the
 * standard `CLAUDE.md` section 13 sets for anything user-visible.
 *
 * ── Why a parser rather than a library ──────────────────────────────────────
 *
 * Section 10 rule 3: a dependency for something a competent developer writes in
 * forty lines is not worth the supply chain risk. This is nearer two hundred
 * than forty, but it reads three tables of a format that has not changed since
 * 2018, in a build-time test, against two files we commit ourselves. A font
 * toolkit would bring a parser for the other fifty tables and a release cadence
 * we would have to track.
 *
 * ── Node only ───────────────────────────────────────────────────────────────
 *
 * `node:zlib` supplies the Brotli decoder, so this module is deliberately NOT
 * re-exported from the package barrel: importing it from a component would put
 * a Node built-in into the renderer bundle. It is imported by its own path, by
 * tests and tooling.
 *
 * ── Format ──────────────────────────────────────────────────────────────────
 *
 * WOFF2 (W3C Recommendation, March 2018) is an sfnt whose tables are
 * concatenated and Brotli-compressed as one stream, preceded by a directory
 * giving each table's tag and length. `glyf` and `loca` are additionally
 * rewritten by a transform; the three tables read here — `head`, `hhea`,
 * `hmtx`, `cmap` — are not, and the parser refuses rather than guesses if it
 * ever meets one that is.
 */
import { brotliDecompressSync } from "node:zlib";

/** What a caller gets back: the em square, and the advance of each glyph asked for. */
export interface FontAdvances {
  /** Font design units per em. Advances are in these units. */
  readonly unitsPerEm: number;
  /** Advance width in font units, keyed by the code point requested. */
  readonly advanceWidths: ReadonlyMap<number, number>;
}

/**
 * The 63 table tags WOFF2 encodes as an index rather than four characters.
 * Order is normative — it is the flag value — so this array may not be sorted,
 * tidied or deduplicated.
 */
const KNOWN_TABLE_TAGS = [
  "cmap",
  "head",
  "hhea",
  "hmtx",
  "maxp",
  "name",
  "OS/2",
  "post",
  "cvt ",
  "fpgm",
  "glyf",
  "loca",
  "prep",
  "CFF ",
  "VORG",
  "EBDT",
  "EBLC",
  "gasp",
  "hdmx",
  "kern",
  "LTSH",
  "PCLT",
  "VDMX",
  "vhea",
  "vmtx",
  "BASE",
  "GDEF",
  "GPOS",
  "GSUB",
  "EBSC",
  "JSTF",
  "MATH",
  "CBDT",
  "CBLC",
  "COLR",
  "CPAL",
  "SVG ",
  "sbix",
  "acnt",
  "avar",
  "bdat",
  "bloc",
  "bsln",
  "cvar",
  "fdsc",
  "feat",
  "fmtx",
  "fvar",
  "gvar",
  "hsty",
  "just",
  "lcar",
  "mort",
  "morx",
  "opbd",
  "prop",
  "trak",
  "Zapf",
  "Silf",
  "Glat",
  "Gloc",
  "Feat",
  "Sill",
] as const;

/** Flag value 63 means the four-character tag follows inline. */
const ARBITRARY_TAG = 63;

const WOFF2_SIGNATURE = "wOF2";
const WOFF2_HEADER_LENGTH = 48;

/** Thrown for every malformed or unsupported input. Never a range error. */
export class FontParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "FontParseError";
  }
}

/**
 * A cursor that refuses to read past the end.
 *
 * Every length in a font file came from the file, which is exactly the case
 * `CLAUDE.md` section 11 is about: a parser must not index blindly into a
 * buffer whose bounds the input chose. A truncated file must produce a
 * `FontParseError` naming the table, not a `RangeError` from somewhere in the
 * middle of a read.
 */
class Reader {
  private offset = 0;

  constructor(
    private readonly view: DataView,
    private readonly what: string,
  ) {}

  get position(): number {
    return this.offset;
  }

  private require(bytes: number): number {
    const at = this.offset;
    if (at + bytes > this.view.byteLength) {
      throw new FontParseError(
        `${this.what}: read of ${String(bytes)} byte(s) at ${String(at)} runs past the end (${String(this.view.byteLength)})`,
      );
    }
    this.offset = at + bytes;
    return at;
  }

  u8(): number {
    return this.view.getUint8(this.require(1));
  }

  u16(): number {
    return this.view.getUint16(this.require(2));
  }

  u32(): number {
    return this.view.getUint32(this.require(4));
  }

  ascii(length: number): string {
    const at = this.require(length);
    let out = "";
    for (let i = 0; i < length; i += 1)
      out += String.fromCharCode(this.view.getUint8(at + i));
    return out;
  }

  skip(bytes: number): void {
    this.require(bytes);
  }

  /**
   * UIntBase128: up to five 7-bit groups, most significant first, the top bit
   * set on every byte but the last. The spec forbids a leading zero group and
   * forbids overflowing 32 bits, and both rules matter — without them two
   * different encodings mean the same number and a directory can be written to
   * disagree with itself.
   */
  base128(): number {
    let accumulator = 0;
    for (let i = 0; i < 5; i += 1) {
      const byte = this.u8();
      if (i === 0 && byte === 0x80) {
        throw new FontParseError(
          `${this.what}: UIntBase128 with a leading zero group`,
        );
      }
      if (accumulator > 0x01ffffff) {
        throw new FontParseError(`${this.what}: UIntBase128 overflows 32 bits`);
      }
      accumulator = accumulator * 128 + (byte & 0x7f);
      if ((byte & 0x80) === 0) return accumulator;
    }
    throw new FontParseError(
      `${this.what}: UIntBase128 longer than five bytes`,
    );
  }
}

interface TableRecord {
  readonly tag: string;
  /** Offset into the decompressed stream. */
  readonly offset: number;
  readonly length: number;
  /** True when the table is stored in a rewritten form this parser cannot read. */
  readonly transformed: boolean;
}

/** Read the WOFF2 header and table directory, and decompress the table stream. */
function readTables(file: Uint8Array): Map<string, Uint8Array> {
  const reader = new Reader(
    new DataView(file.buffer, file.byteOffset, file.byteLength),
    "woff2 header",
  );

  const signature = reader.ascii(4);
  if (signature !== WOFF2_SIGNATURE) {
    throw new FontParseError(
      `not a WOFF2 file: signature is ${JSON.stringify(signature)}`,
    );
  }

  const flavour = reader.ascii(4);
  if (flavour === "ttcf") {
    throw new FontParseError("WOFF2 font collections are not supported");
  }

  reader.skip(4); // length
  const numTables = reader.u16();
  reader.skip(2); // reserved
  reader.skip(4); // totalSfntSize
  const totalCompressedSize = reader.u32();
  reader.skip(WOFF2_HEADER_LENGTH - reader.position);

  const records: TableRecord[] = [];
  let cursor = 0;

  for (let i = 0; i < numTables; i += 1) {
    const flags = reader.u8();
    const index = flags & 0x3f;
    const transformVersion = (flags >> 6) & 0x03;

    const tag =
      index === ARBITRARY_TAG
        ? reader.ascii(4)
        : (KNOWN_TABLE_TAGS[index] ?? "");
    if (tag === "") {
      throw new FontParseError(
        `table ${String(i)}: unknown known-tag index ${String(index)}`,
      );
    }

    const originalLength = reader.base128();

    /* The one asymmetry in the format. For `glyf` and `loca`, transform version
       0 means the transform IS applied — they are the common case and got the
       cheaper encoding. For every other table, 0 means no transform. */
    const transformed =
      tag === "glyf" || tag === "loca"
        ? transformVersion === 0
        : transformVersion !== 0;
    const length = transformed ? reader.base128() : originalLength;

    records.push({ tag, offset: cursor, length, transformed });
    cursor += length;
  }

  const stream = file.subarray(
    reader.position,
    reader.position + totalCompressedSize,
  );
  if (stream.byteLength !== totalCompressedSize) {
    throw new FontParseError("compressed table stream is truncated");
  }

  const decompressed = brotliDecompressSync(stream);
  if (decompressed.byteLength < cursor) {
    throw new FontParseError(
      `decompressed stream is ${String(decompressed.byteLength)} bytes, directory needs ${String(cursor)}`,
    );
  }

  const tables = new Map<string, Uint8Array>();
  for (const record of records) {
    if (record.transformed) continue; // unreadable here, and unneeded — see below
    tables.set(
      record.tag,
      decompressed.subarray(record.offset, record.offset + record.length),
    );
  }
  return tables;
}

function requireTable(
  tables: Map<string, Uint8Array>,
  tag: string,
): Uint8Array {
  const table = tables.get(tag);
  if (table === undefined) {
    /* Either absent or transformed. Both are a refusal rather than a guess: the
       transformed forms of `hmtx` reconstruct advances from glyph outlines, and
       a parser that quietly returned the wrong number here would make the test
       that depends on it worthless. */
    throw new FontParseError(
      `table '${tag}' is missing or stored in a transformed form`,
    );
  }
  return table;
}

/**
 * Map code points to glyph identifiers through a `cmap` format 4 subtable.
 *
 * Format 4 is the segmented BMP mapping every font carries. Timecode is digits,
 * colon, semicolon and full stop, all of which are BMP, so the format 12
 * subtables a font may also carry are not consulted.
 */
function readCmapFormat4(cmap: Uint8Array): (codePoint: number) => number {
  const view = new DataView(cmap.buffer, cmap.byteOffset, cmap.byteLength);
  const read16 = (at: number): number => {
    if (at + 2 > cmap.byteLength)
      throw new FontParseError(`cmap: read past the end at ${String(at)}`);
    return view.getUint16(at);
  };
  const readSigned16 = (at: number): number => {
    if (at + 2 > cmap.byteLength)
      throw new FontParseError(`cmap: read past the end at ${String(at)}`);
    return view.getInt16(at);
  };

  const subtableCount = read16(2);
  let format4Offset = -1;

  for (let i = 0; i < subtableCount; i += 1) {
    const record = 4 + i * 8;
    const platformId = read16(record);
    const encodingId = read16(record + 2);
    if (record + 8 > cmap.byteLength)
      throw new FontParseError("cmap: subtable record past the end");
    const offset = view.getUint32(record + 4);
    if (offset + 2 > cmap.byteLength)
      throw new FontParseError("cmap: subtable offset past the end");
    if (read16(offset) !== 4) continue;
    // Windows BMP (3,1) or Unicode (0,*). Either is a full Unicode mapping.
    if ((platformId === 3 && encodingId === 1) || platformId === 0)
      format4Offset = offset;
  }

  if (format4Offset === -1) {
    throw new FontParseError("cmap: no Unicode format 4 subtable");
  }

  const base = format4Offset;
  const segCountX2 = read16(base + 6);
  const segmentCount = segCountX2 / 2;
  const endCodes = base + 14;
  const startCodes = endCodes + segCountX2 + 2; // +2 skips the reserved pad
  const idDeltas = startCodes + segCountX2;
  const idRangeOffsets = idDeltas + segCountX2;

  return (codePoint: number): number => {
    for (let segment = 0; segment < segmentCount; segment += 1) {
      const end = read16(endCodes + segment * 2);
      if (codePoint > end) continue;
      const start = read16(startCodes + segment * 2);
      if (codePoint < start) return 0;

      const delta = readSigned16(idDeltas + segment * 2);
      const rangeOffsetAt = idRangeOffsets + segment * 2;
      const rangeOffset = read16(rangeOffsetAt);
      if (rangeOffset === 0) return (codePoint + delta) & 0xffff;

      const glyph = read16(
        rangeOffsetAt + rangeOffset + (codePoint - start) * 2,
      );
      return glyph === 0 ? 0 : (glyph + delta) & 0xffff;
    }
    return 0;
  };
}

/**
 * Read the advance width of each requested code point from a WOFF2 file.
 *
 * @throws {FontParseError} if the file is malformed, truncated, a collection,
 *   stores a needed table in a transformed form, or has no glyph for a
 *   requested code point. A missing glyph throws rather than returning zero
 *   because "the font does not contain a digit" and "the digit is zero units
 *   wide" are very different facts and only one of them is a passing test.
 */
export function readWoff2Advances(
  file: Uint8Array,
  codePoints: Iterable<number>,
): FontAdvances {
  const tables = readTables(file);

  const head = requireTable(tables, "head");
  if (head.byteLength < 20)
    throw new FontParseError("head: shorter than 20 bytes");
  const unitsPerEm = new DataView(
    head.buffer,
    head.byteOffset,
    head.byteLength,
  ).getUint16(18);
  if (unitsPerEm === 0) throw new FontParseError("head: unitsPerEm is zero");

  const hhea = requireTable(tables, "hhea");
  if (hhea.byteLength < 36)
    throw new FontParseError("hhea: shorter than 36 bytes");
  const numberOfHMetrics = new DataView(
    hhea.buffer,
    hhea.byteOffset,
    hhea.byteLength,
  ).getUint16(34);
  if (numberOfHMetrics === 0)
    throw new FontParseError("hhea: numberOfHMetrics is zero");

  const hmtx = requireTable(tables, "hmtx");
  const hmtxView = new DataView(hmtx.buffer, hmtx.byteOffset, hmtx.byteLength);

  /* `hmtx` stores `numberOfHMetrics` (advance, bearing) pairs and then bearings
     alone. Every glyph past the last pair repeats that pair's advance — how a
     monospaced font stores one width for a thousand glyphs. */
  const advanceOf = (glyph: number): number => {
    const entry = Math.min(glyph, numberOfHMetrics - 1);
    const at = entry * 4;
    if (at + 2 > hmtx.byteLength) {
      throw new FontParseError(`hmtx: no metric for glyph ${String(glyph)}`);
    }
    return hmtxView.getUint16(at);
  };

  const lookup = readCmapFormat4(requireTable(tables, "cmap"));
  const advanceWidths = new Map<number, number>();

  for (const codePoint of codePoints) {
    const glyph = lookup(codePoint);
    if (glyph === 0) {
      throw new FontParseError(
        `no glyph for U+${codePoint.toString(16).toUpperCase().padStart(4, "0")}`,
      );
    }
    advanceWidths.set(codePoint, advanceOf(glyph));
  }

  return { unitsPerEm, advanceWidths };
}
