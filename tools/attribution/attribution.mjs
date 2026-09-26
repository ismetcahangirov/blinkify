/**
 * The third-party attribution document (#73): what it is built from, and how.
 *
 * MIT, BSD, ISC and Apache-2.0 all ask for the copyright notice and the
 * licence text to travel with the binary. The transitive dependency set is
 * hundreds of packages and moves on every `cargo update`, so the only
 * attribution that stays true is generated — and a generated file is only
 * worth committing if a diff can mean one thing: the dependency set moved.
 * Hence the two properties this module is built around:
 *
 * - **It is a pure function of the tree.** No clock, no absolute path, no
 *   environment value reaches the output, and every collection is sorted with
 *   a code-unit comparator (never the locale-aware one — `CLAUDE.md` section
 *   14 and `docs/engineering/architecture-gates.md` record what that costs).
 * - **It fails rather than skips.** A package it cannot attribute is an error
 *   that names the package, never a missing line. A partial attribution file
 *   reads as complete, which is worse than none.
 *
 * Nothing here touches the network. Rust licence texts are read from each
 * crate's own source in the local cargo registry, where `cargo metadata` has
 * already unpacked it (`manifest_path`); renderer texts are read from the
 * installed `node_modules`, which pnpm materialised from the lockfile. The
 * few packages that ship no licence file at all get the standard SPDX text
 * committed under `licences/`, marked as such in the document.
 *
 * The decision to write this rather than adopt `cargo-about` is ADR-0017.
 */
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  existsSync,
  readFileSync,
  readdirSync,
  realpathSync,
  statSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const ROOT = resolve(HERE, "..", "..");

/** Where the document is written, and bundled from (`tauri.conf.json`). */
export const DOCUMENT = join(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "THIRD-PARTY-NOTICES.txt",
);

/**
 * The one target Blinkify ships for. Filtering to it keeps crates the Windows
 * build never compiles — the GTK stack under Tauri, for one — out of the
 * document, and makes the set independent of the machine generating it.
 */
export const TARGET = "x86_64-pc-windows-msvc";

/**
 * The order in which a choice between licences is made, when a package offers
 * one (`MIT OR Apache-2.0`). Stated in the document itself. MIT first because
 * it is the most common and the shortest notice; the rest in roughly the same
 * spirit. A licence absent from this list can still be attributed — it simply
 * loses every tie.
 */
export const PREFERENCE = [
  "MIT",
  "Apache-2.0",
  "BSD-3-Clause",
  "BSD-2-Clause",
  "ISC",
  "Zlib",
  "0BSD",
  "MIT-0",
  "Unlicense",
  "CC0-1.0",
  "BSL-1.0",
  "Unicode-3.0",
  "Apache-2.0 WITH LLVM-exception",
  "CDLA-Permissive-2.0",
  "MPL-2.0",
];

/** A code-unit comparator. Never `localeCompare`: its order depends on ICU. */
export const byCodeUnit = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** The attribution failed. `problems` names every package that could not be attributed. */
export class AttributionError extends Error {
  constructor(problems) {
    super(
      `${problems.length} package(s) could not be attributed:\n` +
        problems.map((problem) => `  - ${problem}`).join("\n"),
    );
    this.name = "AttributionError";
    this.problems = problems;
  }
}

// ── Text handling ────────────────────────────────────────────────────────────

const BYTE_ORDER_MARK = String.fromCharCode(0xfeff);

/**
 * One canonical form for a text, so that the same licence read from two
 * packages — one with CRLF line endings and a byte-order mark, one without —
 * is recognised as the same text and printed once.
 */
export function normalise(text) {
  return (
    text
      .replace(new RegExp(`^${BYTE_ORDER_MARK}`), "")
      .replace(/\r\n?/g, "\n")
      .split("\n")
      .map((line) => line.replace(/[ \t]+$/, ""))
      .join("\n")
      .replace(/^\n+/, "")
      .replace(/\n+$/, "") + "\n"
  );
}

const MIT_GRANT =
  /permission is hereby granted, free of charge, to any person obtaining a copy/i;
const MIT_CONDITION =
  /above copyright notice and this permission notice shall be included/i;
const BSD_REDISTRIBUTION = /redistribution and use in source and binary forms/i;
const ISC_GRANT =
  /permission to use, copy, modify,? and\/or distribute this software for any\s+purpose with or without fee is hereby granted/i;

/**
 * What licence a file's text is, recognised by the wording each licence
 * cannot be written without. Returns every match: a crate that puts both of
 * its licences in one `LICENSE` file offers both texts.
 *
 * Deliberately by content, not file name. `LICENSE` alone says nothing, and a
 * file named `LICENSE-MIT` that holds a reference to the MIT licence rather
 * than the licence is not a text.
 */
export function classify(original) {
  // Licence files are wrapped at whatever width their author liked, and
  // Markdown ones add emphasis; the wording is what identifies them.
  // Some are source-comment blocks, `//` on every line.
  const text = original
    .replace(/^[ \t]*\/\/+/gm, " ")
    .replace(/[*_#>]/g, " ")
    .replace(/\s+/g, " ");
  const found = new Set();
  const apache =
    /apache license,?\s+version 2\.0/i.test(text) &&
    /terms and conditions for use, reproduction,? and distribution/i.test(text);
  if (apache) found.add("Apache-2.0");
  if (apache && /llvm exceptions to the apache 2\.0 license/i.test(text))
    found.add("Apache-2.0 WITH LLVM-exception");
  if (MIT_GRANT.test(text) && MIT_CONDITION.test(text)) found.add("MIT");
  if (
    MIT_GRANT.test(text) &&
    !MIT_CONDITION.test(text) &&
    /without restriction/i.test(text)
  )
    found.add("MIT-0");
  if (BSD_REDISTRIBUTION.test(text)) {
    if (/neither the name|names of (its|the) contributors/i.test(text))
      found.add("BSD-3-Clause");
    else found.add("BSD-2-Clause");
  }
  if (ISC_GRANT.test(text))
    found.add(
      /provided that the above copyright notice and this permission notice appear/i.test(
        text,
      )
        ? "ISC"
        : "0BSD",
    );
  if (
    /this software is provided ['‘]as-is['’]/i.test(text) &&
    /altered source versions must be plainly marked/i.test(text)
  )
    found.add("Zlib");
  if (
    /this is free and unencumbered software released into the public domain/i.test(
      text,
    )
  )
    found.add("Unlicense");
  if (/unicode license v3|unicode, inc\. license agreement/i.test(text))
    found.add("Unicode-3.0");
  if (/boost software license - version 1\.0/i.test(text)) found.add("BSL-1.0");
  if (
    /mozilla public license,?\s+(version|v\.)\s*2\.0/i.test(text) &&
    /1\. definitions/i.test(text)
  )
    found.add("MPL-2.0");
  if (/cc0 1\.0 universal/i.test(text)) found.add("CC0-1.0");
  if (
    /community data license agreement\s*[-–]\s*permissive\s*[-–]\s*version 2\.0/i.test(
      text,
    )
  )
    found.add("CDLA-Permissive-2.0");
  return found;
}

/** Files a package keeps its licence, copyright or notice in. */
const LICENCE_FILE =
  /^(licen[cs]e|copying|copyright|notice|unlicense|mit|apache|bsd|zlib)([-._ ].*)?$/i;
const NOTICE_FILE = /^(notice|copyright)([-._ ].*)?$/i;
const LICENCE_DIR = /^licen[cs]es?$/i;

/** Every licence-like file at the top of `dir`, and in a `LICENSES/` folder beside them. */
function licenceFiles(dir, declaredFile) {
  const found = new Set();
  const add = (path) => {
    if (existsSync(path) && statSync(path).isFile()) found.add(path);
  };
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (LICENCE_DIR.test(name) && statSync(path).isDirectory()) {
      for (const inner of readdirSync(path)) add(join(path, inner));
    } else if (LICENCE_FILE.test(name) && !name.endsWith(".spdx")) add(path);
  }
  if (declaredFile) add(resolve(dir, declaredFile));
  return [...found].sort(byCodeUnit).map((path) => {
    const text = normalise(readFileSync(path, "utf8"));
    const name = path.slice(dir.length + 1).replaceAll("\\", "/");
    return {
      name,
      text,
      licences: classify(text),
      notice: NOTICE_FILE.test(name.split("/").pop() ?? ""),
    };
  });
}

// ── Licence expressions ──────────────────────────────────────────────────────

/**
 * Parse an SPDX licence expression into its alternatives: a list of options,
 * each the list of licences that option binds you to. `(MIT OR Apache-2.0)
 * AND Unicode-3.0` has two options, `[MIT, Unicode-3.0]` and
 * `[Apache-2.0, Unicode-3.0]`. The legacy `MIT/Apache-2.0` form that older
 * crates still carry is read as `OR`, which is what it always meant.
 */
export function alternatives(expression) {
  const tokens = expression
    .replace(/\s*\/\s*/g, " OR ")
    .replace(/([()])/g, " $1 ")
    .trim()
    .split(/\s+/);
  let at = 0;
  const peek = () => tokens[at];
  const fail = (why) => {
    throw new Error(`cannot read licence expression "${expression}": ${why}`);
  };

  const orExpression = () => {
    let options = andExpression();
    while (peek()?.toUpperCase() === "OR") {
      at += 1;
      options = [...options, ...andExpression()];
    }
    return options;
  };
  const andExpression = () => {
    let options = withExpression();
    while (peek()?.toUpperCase() === "AND") {
      at += 1;
      const right = withExpression();
      options = options.flatMap((left) => right.map((r) => [...left, ...r]));
    }
    return options;
  };
  const withExpression = () => {
    const token = peek();
    if (token === undefined) fail("it ends early");
    if (token === "(") {
      at += 1;
      const inner = orExpression();
      if (peek() !== ")") fail("a bracket is not closed");
      at += 1;
      return inner;
    }
    if (/^(AND|OR|WITH|\))$/i.test(token)) fail(`unexpected "${token}"`);
    at += 1;
    if (peek()?.toUpperCase() === "WITH") {
      at += 1;
      const exception = peek();
      if (exception === undefined) fail("WITH names no exception");
      at += 1;
      return [[`${token} WITH ${exception}`]];
    }
    return [[token]];
  };

  const options = orExpression();
  if (at !== tokens.length) fail(`unexpected "${peek()}"`);
  // One option per distinct licence set, each set sorted.
  const seen = new Map();
  for (const option of options) {
    const set = [...new Set(option)].sort(byCodeUnit);
    seen.set(set.join(" AND "), set);
  }
  return [...seen.values()];
}

const rank = (licence) => {
  const index = PREFERENCE.indexOf(licence);
  return index === -1 ? PREFERENCE.length : index;
};

/** Compare two options: the one whose texts the package ships, then by preference. */
function compareOptions(a, b) {
  if (a.shipped !== b.shipped) return a.shipped ? -1 : 1;
  const ra = a.licences.map(rank).sort((x, y) => x - y);
  const rb = b.licences.map(rank).sort((x, y) => x - y);
  for (let i = 0; i < Math.min(ra.length, rb.length); i += 1)
    if (ra[i] !== rb[i]) return ra[i] - rb[i];
  if (ra.length !== rb.length) return ra.length - rb.length;
  return byCodeUnit(a.licences.join(" AND "), b.licences.join(" AND "));
}

// ── Attribution of one package ──────────────────────────────────────────────

/** The committed standard text of a licence, or `null` if none is committed. */
function standardText(licence) {
  const path = join(HERE, "licences", `${licence}.txt`);
  return existsSync(path) ? normalise(readFileSync(path, "utf8")) : null;
}

/**
 * A standard text with its placeholder copyright lines — the run of
 * `Copyright` lines in its first few lines, `<year> <copyright holders>` and
 * the like — replaced by the holders the package declares, and never by a
 * holder it does not. A text with no such line gets the holders on top.
 */
function withHolders(text, pkg) {
  const holders =
    pkg.authors.length > 0
      ? `Copyright (c) ${pkg.authors.join(", ")}`
      : `Copyright (c) the authors of ${pkg.name}`;
  const lines = text.split("\n");
  const isCopyright = (line) => /^\s*copyright\b/i.test(line ?? "");
  const first = lines.findIndex(isCopyright);
  if (first === -1 || first > 4) return normalise(`${holders}\n\n${text}`);
  let end = first;
  while (isCopyright(lines[end])) end += 1;
  lines.splice(first, end - first, holders);
  return normalise(lines.join("\n"));
}

/**
 * Decide which licence a package is taken under, and gather the texts that
 * carry it. Returns `{ taken, texts }` or throws with the reason, naming the
 * package.
 */
export function attribute(pkg) {
  const files = licenceFiles(pkg.dir, pkg.licenseFile);
  const label = `${pkg.name} ${pkg.version} (${pkg.ecosystem})`;

  // A package that states no SPDX expression at all and points at a file: the
  // file is the licence.
  if (!pkg.declared) {
    const declaredFile = pkg.licenseFile
      ? files.find(
          (file) =>
            resolve(pkg.dir, file.name) === resolve(pkg.dir, pkg.licenseFile),
        )
      : undefined;
    if (!declaredFile)
      throw new Error(`${label} declares no licence and ships no licence file`);
    return {
      taken: [`the licence in ${declaredFile.name}`],
      texts: [
        {
          licence: `see ${declaredFile.name}`,
          origin: declaredFile.name,
          text: declaredFile.text,
        },
      ],
      standard: false,
    };
  }

  let options;
  try {
    options = alternatives(pkg.declared);
  } catch (error) {
    throw new Error(`${label}: ${error.message}`, { cause: error });
  }

  const has = (licence) => files.some((file) => file.licences.has(licence));
  const ranked = options
    .map((licences) => ({ licences, shipped: licences.every(has) }))
    .sort(compareOptions);
  const chosen = ranked[0];
  if (chosen === undefined)
    throw new Error(`${label}: empty licence expression`);

  const texts = [];
  let standard = false;
  for (const licence of chosen.licences) {
    const own = files.filter((file) => file.licences.has(licence));
    if (own.length > 0) {
      for (const file of own)
        texts.push({ licence, origin: file.name, text: file.text });
      continue;
    }
    const fallback = standardText(licence);
    if (fallback === null)
      throw new Error(
        `${label} is taken under ${licence}, ships no text for it, and no standard ` +
          `text is committed at tools/attribution/licences/${licence}.txt`,
      );
    standard = true;
    texts.push({
      licence,
      origin: "standard text (the package ships none)",
      text: withHolders(fallback, pkg),
    });
  }

  // The package's own words are never dropped: a NOTICE or COPYRIGHT file, or
  // a licence-named file whose text is none of the licences recognised, is
  // carried as a notice. Files that hold only the option not taken are not.
  for (const file of files) {
    const other =
      file.licences.size > 0 &&
      !chosen.licences.some((l) => file.licences.has(l));
    const alreadyIn = texts.some((t) => t.origin === file.name);
    if (alreadyIn || other) continue;
    if (file.notice || file.licences.size === 0)
      texts.push({ licence: "notice", origin: file.name, text: file.text });
  }

  return { taken: chosen.licences, texts, standard };
}

// ── Collecting packages ─────────────────────────────────────────────────────

const stripEmail = (person) =>
  person
    .replace(/\s*<[^>]*>\s*/g, " ")
    .replace(/\s*\([^)]*\)\s*$/, "")
    .trim();

/**
 * Every package `cargo metadata` resolves for the Windows target, minus the
 * workspace's own crates. Every one — build-time and test crates included —
 * because the acceptance criterion is that nothing `cargo metadata` names is
 * missing, and a notice that lists too much costs a page where one that lists
 * too little is a compliance failure.
 */
export function collectCrates({ manifestPath, offline = false }) {
  const args = [
    "metadata",
    "--format-version",
    "1",
    "--all-features",
    "--locked",
    "--filter-platform",
    TARGET,
    "--manifest-path",
    manifestPath,
  ];
  if (offline) args.push("--offline");
  const metadata = JSON.parse(
    execFileSync("cargo", args, {
      cwd: dirname(manifestPath),
      encoding: "utf8",
      maxBuffer: 512 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    }),
  );
  const members = new Set(metadata.workspace_members);
  return metadata.packages
    .filter((pkg) => !members.has(pkg.id))
    .map((pkg) => ({
      ecosystem: "Rust crate",
      name: pkg.name,
      version: pkg.version,
      declared: pkg.license ?? null,
      licenseFile: pkg.license_file ?? null,
      dir: dirname(pkg.manifest_path),
      authors: (pkg.authors ?? []).map(stripEmail).filter(Boolean),
      source: pkg.source?.startsWith("registry+")
        ? `https://crates.io/crates/${pkg.name}/${pkg.version}`
        : (pkg.repository ?? null),
    }));
}

/** Node's own resolution: the nearest `node_modules/<name>` walking up from `from`. */
export function resolvePackage(from, name) {
  for (let dir = from; ; dir = dirname(dir)) {
    const candidate = join(dir, "node_modules", name, "package.json");
    if (existsSync(candidate)) return realpathSync(dirname(candidate));
    if (dirname(dir) === dir) return null;
  }
}

function npmLicence(manifest) {
  if (typeof manifest.license === "string") return manifest.license;
  if (manifest.license && typeof manifest.license.type === "string")
    return manifest.license.type;
  if (Array.isArray(manifest.licenses))
    return manifest.licenses
      .map((l) => (typeof l === "string" ? l : l.type))
      .join(" OR ");
  return null;
}

function npmAuthor(manifest) {
  const author = manifest.author;
  if (typeof author === "string") return [stripEmail(author)].filter(Boolean);
  if (author && typeof author.name === "string") return [author.name.trim()];
  return [];
}

/**
 * Every package in the production dependency tree of the given workspace
 * packages — `dependencies` and installed `optionalDependencies`, followed
 * transitively through `node_modules` as pnpm laid it out from the lockfile.
 * Workspace packages (`workspace:`) are Blinkify's own and are walked through
 * but not listed. `devDependencies` never reach the bundle and are not
 * followed; `peerDependencies` are supplied by a package already in the tree.
 */
export function collectNpm({ roots }) {
  const found = new Map();
  const failures = [];
  const visit = (dir, own) => {
    const manifest = JSON.parse(
      readFileSync(join(dir, "package.json"), "utf8"),
    );
    const key = `${manifest.name}@${manifest.version}`;
    if (found.has(key)) return;
    found.set(key, own ? null : { dir, manifest });
    const required = manifest.dependencies ?? {};
    const optional = manifest.optionalDependencies ?? {};
    for (const name of Object.keys({ ...required, ...optional }).sort(
      byCodeUnit,
    )) {
      const target = resolvePackage(dir, name);
      if (target === null) {
        if (!(name in optional))
          failures.push(`${name}, required by ${key}, is not installed`);
        continue;
      }
      const spec = required[name] ?? optional[name] ?? "";
      visit(target, spec.startsWith("workspace:"));
    }
  };
  for (const root of roots) visit(realpathSync(root), true);
  if (failures.length > 0)
    throw new AttributionError(failures.sort(byCodeUnit));

  return [...found.values()]
    .filter((entry) => entry !== null)
    .map(({ dir, manifest }) => ({
      ecosystem: "npm package",
      name: manifest.name,
      version: manifest.version,
      declared: npmLicence(manifest),
      licenseFile: null,
      dir,
      authors: npmAuthor(manifest),
      source: `https://www.npmjs.com/package/${manifest.name}/v/${manifest.version}`,
    }));
}

// ── Bundled components that no package manager describes ───────────────────

/**
 * FFmpeg, the libraries built into it, the noise model, the typefaces and the
 * ported smart-cut code. None of them has package metadata, so each is
 * described in `components.json` — *why* each ships is `THIRD_PARTY.md`'s
 * job; this file only says where its texts are. FFmpeg's version and
 * configure line are read from the sidecar's own lock and configure files, so
 * a sidecar bump cannot leave its notice behind.
 */
export function collectComponents({ file, root = ROOT }) {
  const spec = JSON.parse(readFileSync(file, "utf8"));
  return spec.components.map((component) => {
    let version = component.version;
    const notice = [...(component.notice ?? [])];
    if (component.sidecar) {
      const lock = JSON.parse(
        readFileSync(join(root, component.sidecar.lock), "utf8"),
      );
      version = `${lock.source.tag} (commit ${lock.source.commit}), Blinkify build ${lock.version}`;
      const flags = readFileSync(
        join(root, component.sidecar.configure),
        "utf8",
      )
        .split(/\r?\n/)
        .map((line) => line.trim())
        .filter((line) => line !== "" && !line.startsWith("#"));
      notice.push(
        `Source: ${lock.source.repository}, tag ${lock.source.tag}, commit ${lock.source.commit}.`,
      );
      notice.push(`Configured with: ${flags.join(" ")}`);
    }
    const texts = component.texts.map(({ licence, path }) => {
      const full = join(root, path);
      if (!existsSync(full))
        throw new AttributionError([
          `${component.name}: its text ${path} does not exist`,
        ]);
      return {
        licence,
        origin: path.split("/").pop(),
        text: normalise(readFileSync(full, "utf8")),
      };
    });
    return {
      name: component.name,
      version,
      licence: component.licence,
      notice,
      texts,
    };
  });
}

// ── The document ─────────────────────────────────────────────────────────────

const RULE = "=".repeat(78);
const THIN = "-".repeat(78);

/** Wrap prose at 78 columns. Words are never broken. */
function wrap(text, indent = "") {
  const lines = [];
  let line = indent;
  for (const word of text.split(/\s+/).filter(Boolean)) {
    if (line.trim() !== "" && line.length + 1 + word.length > 78) {
      lines.push(line);
      line = indent + word;
    } else line = line.trim() === "" ? indent + word : `${line} ${word}`;
  }
  if (line.trim() !== "") lines.push(line);
  return lines.join("\n");
}

const textId = (text) =>
  createHash("sha256").update(text).digest("hex").slice(0, 10);

/**
 * Attribute every package and render the document. Throws an
 * `AttributionError` naming every package that could not be attributed —
 * all of them at once, so a contributor fixes the list in one pass.
 */
export function render({ components, packages }) {
  const problems = [];
  const attributed = [];
  const sorted = [...packages].sort(
    (a, b) =>
      byCodeUnit(a.name, b.name) ||
      byCodeUnit(a.version, b.version) ||
      byCodeUnit(a.ecosystem, b.ecosystem),
  );
  for (const pkg of sorted) {
    try {
      attributed.push({ pkg, ...attribute(pkg) });
    } catch (error) {
      problems.push(error.message);
    }
  }
  if (problems.length > 0) throw new AttributionError(problems);

  // Texts, deduplicated by content, in order of first reference.
  const texts = new Map();
  const cite = (entry, text) => {
    const id = textId(text.text);
    if (!texts.has(id))
      texts.set(id, {
        text: text.text,
        licences: new Set(),
        origins: new Set(),
        covers: [],
      });
    const record = texts.get(id);
    record.licences.add(text.licence);
    record.origins.add(text.origin);
    if (!record.covers.includes(entry)) record.covers.push(entry);
    return id;
  };

  const out = [];
  const crates = attributed.filter(
    (a) => a.pkg.ecosystem === "Rust crate",
  ).length;
  const npm = attributed.length - crates;
  const fallbacks = attributed.filter((a) => a.standard).length;

  out.push(RULE, "Blinkify: third-party notices", RULE, "");
  for (const paragraph of [
    "Blinkify is built from, and ships, the work of the projects listed here. " +
      "This document carries the copyright notice and licence text each of them asks to travel with it.",
    "It is generated by tools/attribution in the Blinkify repository from `cargo metadata` and the " +
      "installed renderer dependencies, and a build fails if it is out of date. Do not edit it by hand.",
    `Part 1 is the programs and data installed beside Blinkify. Part 2 is every Rust crate the Windows build ` +
      `resolves (${crates}) and every package in the interface's production dependency tree (${npm}), one entry ` +
      `each, with the licence the package declares and the one Blinkify takes it under. Part 3 is the texts: ` +
      `packages that share an identical text share one copy, and each text is found by its reference, for ` +
      `example [text 0123456789].`,
    'Where a package offers a choice of licences ("MIT OR Apache-2.0"), Blinkify takes the option whose ' +
      "text the package itself ships, and among those prefers, in this order: " +
      PREFERENCE.join(", ") +
      ". The option taken is recorded against each package.",
    "Part 2 deliberately includes crates used only to build or test Blinkify, which do not ship: a notice " +
      "that lists too much costs a page, and one that lists too little is a compliance failure.",
    `Where a package ships no licence file (${fallbacks} of them), the standard text of the licence it ` +
      "declares is used, with the authors the package names, and the entry says so.",
  ]) {
    out.push(wrap(paragraph), "");
  }

  out.push(RULE, "Part 1. Programs and data installed with Blinkify", RULE, "");
  for (const component of components) {
    const ids = component.texts.map((text) =>
      cite(`${component.name} ${component.version}`, text),
    );
    out.push(
      component.name,
      wrap(`Version: ${component.version}`, "  "),
      wrap(`Licence: ${component.licence}`, "  "),
    );
    for (const paragraph of component.notice) out.push(wrap(paragraph, "  "));
    out.push(`  Texts: ${ids.map((id) => `[text ${id}]`).join(" ")}`, "");
  }

  out.push(RULE, "Part 2. Rust crates and interface packages", RULE, "");
  for (const { pkg, taken, texts: own, standard } of attributed) {
    const name = `${pkg.name} ${pkg.version}`;
    const ids = [
      ...new Set(own.map((text) => cite(`${name} (${pkg.ecosystem})`, text))),
    ];
    out.push(`${name} (${pkg.ecosystem})`);
    out.push(
      wrap(`Declared: ${pkg.declared ?? "(none; a licence file)"}`, "  "),
    );
    out.push(
      wrap(
        `Taken under: ${taken.join(" AND ")}${standard ? " (standard text; the package ships none)" : ""}`,
        "  ",
      ),
    );
    if (pkg.source) out.push(`  Source: ${pkg.source}`);
    out.push(`  Texts: ${ids.map((id) => `[text ${id}]`).join(" ")}`, "");
  }

  out.push(RULE, "Part 3. Licence texts and notices", RULE, "");
  for (const [id, record] of texts) {
    out.push(
      THIN,
      `[text ${id}] ${[...record.licences].sort(byCodeUnit).join(", ")}`,
    );
    out.push(wrap(`From: ${[...record.origins].sort(byCodeUnit).join(", ")}`));
    out.push("Covers:");
    for (const entry of record.covers) out.push(`  ${entry}`);
    out.push(THIN, "", record.text);
  }

  return normalise(out.join("\n"));
}

/**
 * Inputs from the command line, each defaulting to Blinkify's own. The
 * injection test points them at a throwaway tree, the only way to watch the
 * gate fail without editing the repository.
 *
 *   --manifest-path <Cargo.toml>   the cargo workspace
 *   --npm-root <dir>               a renderer package (repeatable)
 *   --components <file|none>       the bundled components
 *   --document <file>              the document to write or check
 *   --offline                      pass --offline to cargo
 */
export function optionsFromArgs(argv) {
  const options = { document: DOCUMENT };
  const npmRoots = [];
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    const value = () => {
      i += 1;
      if (argv[i] === undefined) throw new Error(`${flag} needs a value`);
      return resolve(argv[i]);
    };
    if (flag === "--manifest-path") options.manifestPath = value();
    else if (flag === "--npm-root") npmRoots.push(value());
    else if (flag === "--components") {
      if (argv[i + 1] === "none") {
        i += 1;
        options.componentsFile = null;
      } else options.componentsFile = value();
    } else if (flag === "--document") options.document = value();
    else if (flag === "--offline") options.offline = true;
  }
  if (npmRoots.length > 0) options.npmRoots = npmRoots;
  return options;
}

/** The document for the Blinkify tree: every input at its usual place. */
export function generate({
  manifestPath = join(ROOT, "Cargo.toml"),
  npmRoots = [join(ROOT, "apps", "desktop")],
  componentsFile = join(HERE, "components.json"),
  offline = false,
} = {}) {
  const packages = [
    ...collectCrates({ manifestPath, offline }),
    ...collectNpm({ roots: npmRoots }),
  ];
  const components = componentsFile
    ? collectComponents({ file: componentsFile })
    : [];
  return render({ components, packages });
}
