#!/usr/bin/env node
/**
 * The FFmpeg sidecar gate.
 *
 * Issue #21, and ADR-0002's first gate. The licence boundary is load-bearing:
 * a GPL FFmpeg in the installer makes Blinkify GPL, and that door does not open
 * again. So the binary itself is asked what it is, and the answer is checked —
 * the download page, the file name and the zip's folder name are all ignored.
 *
 * Five rules, each named so a failure says which one:
 *
 * - `no-gpl-configuration` — the configuration string has no `--enable-gpl`,
 *   `--enable-nonfree` or GPL library.
 * - `no-software-h264-hevc-encoder` — ADR-0003: no `openh264`, no software
 *   HEVC encoder, not even unused.
 * - `configuration-is-ours` — the configure line inside the binary is exactly
 *   the one committed in `tools/ffmpeg-sidecar/configure.txt`. A binary built
 *   from any other line is not the binary THIRD_PARTY.md describes.
 * - `required-components` — every encoder and filter Epics #6 and #7 name is
 *   present, so a licence-clean build that is missing `arnndn` fails here
 *   rather than at a user's export.
 * - `provenance` — the SHA-256 of each binary matches `sidecar.lock.json`,
 *   THIRD_PARTY.md names the pinned version, and no FFmpeg binary is tracked
 *   by git.
 *
 * Usage:
 *   node tools/sidecar-gate.mjs [binaries-dir]
 *
 * The directory defaults to `apps/desktop/src-tauri/binaries`, where
 * `pnpm sidecar:fetch` puts them. The rules themselves are the exported
 * `evaluate()`, which the injection test (`pnpm sidecar:check:test`) drives
 * with the output of real GPL and distributor builds.
 */
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");
const SIDECAR = join(HERE, "ffmpeg-sidecar");
export const DEFAULT_BINARIES = join(
  ROOT,
  "apps",
  "desktop",
  "src-tauri",
  "binaries",
);
export const TRIPLE = "x86_64-pc-windows-msvc";

/** Configure flags that make the build GPL or non-redistributable. */
const FORBIDDEN_FLAGS = ["--enable-gpl", "--enable-nonfree"];

/**
 * Libraries that are GPL, or that only build under `--enable-gpl`. Listed even
 * though `--enable-gpl` would also be caught: a patched configure that enables
 * one without the other is exactly what a string check exists to notice.
 */
const GPL_LIBRARIES = [
  "libx264",
  "libx265",
  "libxvid",
  "libxavs",
  "libxavs2",
  "libdavs2",
  "libvidstab",
  "frei0r",
  "librubberband",
  "libcdio",
  "libdvdnav",
  "libdvdread",
];

/** ADR-0003: no software H.264 or HEVC encoder ships, used or not. */
const FORBIDDEN_ENCODER_LIBRARIES = ["libopenh264", "libkvazaar", "libvvenc"];
const FORBIDDEN_ENCODERS = [
  "libx264",
  "libx264rgb",
  "libx265",
  "libopenh264",
  "libkvazaar",
  "h264_mf",
  "hevc_mf",
];

/**
 * What Blinkify depends on, by the issue that depends on it. The sidecar is
 * not allowed to be discovered missing one of these on a user's machine.
 */
export const REQUIRED_ENCODERS = [
  // ADR-0003: the only H.264 and HEVC encode path.
  "h264_nvenc",
  "hevc_nvenc",
  "av1_nvenc",
  "h264_qsv",
  "hevc_qsv",
  "av1_qsv",
  "h264_amf",
  "hevc_amf",
  "av1_amf",
  // Licence-clean software encoders for a user-chosen tier-3 output.
  "libvpx-vp9",
  "libsvtav1",
  "libaom-av1",
  // Audio: Epic #7's output encoders.
  "aac",
  "libopus",
  "libmp3lame",
  "flac",
  "pcm_s16le",
  // #26: the all-intra proxy codec, and the filmstrip image codec.
  "mjpeg",
];

export const REQUIRED_FILTERS = [
  // Epic #7.
  "arnndn",
  "loudnorm",
  "alimiter",
  "volume",
  "ebur128",
  "aresample",
  // #26: filmstrips and proxies.
  "fps",
  "scale",
  "zscale",
  "tile",
  // #22 and #25: decoding to a pipe and analysing it.
  "anull",
  "null",
];

/** The committed configure line: every non-comment token in configure.txt. */
export function committedFlags(text) {
  return text
    .split(/\r?\n/)
    .map((line) => line.replace(/#.*/, "").trim())
    .filter(Boolean)
    .flatMap((line) => line.split(/\s+/));
}

/** The flags between `configuration:` and the end of that line. */
export function configurationFlags(versionText) {
  const line = versionText
    .split(/\r?\n/)
    .find((l) => l.trimStart().startsWith("configuration:"));
  if (line === undefined) return null;
  return line.replace(/^\s*configuration:\s*/, "").match(/--[^\s]+/g) ?? [];
}

/** Names in the second column of `-encoders` / `-filters` output. */
export function listedNames(listingText) {
  // Both listings print a legend, a `------` rule, then one entry per line:
  // a flags column, the name, and a description.
  const names = new Set();
  let pastLegend = false;
  for (const line of listingText.split(/\r?\n/)) {
    if (/^\s*-{3,}\s*$/.test(line)) {
      pastLegend = true;
      continue;
    }
    const match = /^\s*[A-Z.|]{2,6}\s+(\S+)/.exec(line);
    if (pastLegend && match) names.add(match[1]);
  }
  return names;
}

/** Toolchain flags the build image adds, which the committed line does not. */
const TOOLCHAIN_FLAG =
  /^--(prefix|pkg-config-flags|pkg-config|cross-prefix|arch|target-os|extra-[a-z]+|cc|cxx|ar|ranlib|nm|ld|strip|windres|host-cc|disable-cuda-llvm)(=|$)/;

/**
 * Run every rule over what the binaries reported.
 *
 * @param {object} input
 * @param {string} input.versionText     `ffmpeg -version`
 * @param {string} input.probeVersionText `ffprobe -version`
 * @param {string} input.encodersText    `ffmpeg -hide_banner -encoders`
 * @param {string} input.filtersText     `ffmpeg -hide_banner -filters`
 * @param {string} input.configureText   tools/ffmpeg-sidecar/configure.txt
 * @param {object} input.lock            tools/ffmpeg-sidecar/sidecar.lock.json
 * @param {Record<string,string>} input.hashes  SHA-256 per binary name
 * @param {string} input.thirdParty      THIRD_PARTY.md
 * @param {string[]} input.trackedFiles  `git ls-files`
 * @returns {{rule: string, message: string}[]} violations; empty when clean
 */
export function evaluate(input) {
  const violations = [];
  const fail = (rule, message) => violations.push({ rule, message });

  for (const [name, text] of [
    ["ffmpeg", input.versionText],
    ["ffprobe", input.probeVersionText],
  ]) {
    const flags = configurationFlags(text);
    if (flags === null) {
      fail(
        "no-gpl-configuration",
        `${name} -version printed no configuration line`,
      );
      continue;
    }
    for (const flag of FORBIDDEN_FLAGS) {
      if (flags.includes(flag)) {
        fail("no-gpl-configuration", `${name} is configured with ${flag}`);
      }
    }
    for (const library of GPL_LIBRARIES) {
      if (flags.includes(`--enable-${library}`)) {
        fail("no-gpl-configuration", `${name} enables GPL library ${library}`);
      }
    }
    if (/\bthis version of .* is licensed under the gpl\b/i.test(text)) {
      fail("no-gpl-configuration", `${name} reports a GPL licence`);
    }
    for (const library of FORBIDDEN_ENCODER_LIBRARIES) {
      if (flags.includes(`--enable-${library}`)) {
        fail(
          "no-software-h264-hevc-encoder",
          `${name} enables ${library}; ADR-0003 ships no software H.264, HEVC or VVC encoder`,
        );
      }
    }

    const committed = committedFlags(input.configureText);
    const built = flags.filter((flag) => !TOOLCHAIN_FLAG.test(flag));
    const missing = committed.filter((flag) => !built.includes(flag));
    const extra = built.filter((flag) => !committed.includes(flag));
    if (missing.length > 0 || extra.length > 0) {
      fail(
        "configuration-is-ours",
        `${name} was not built from tools/ffmpeg-sidecar/configure.txt` +
          (missing.length ? `; missing ${missing.join(" ")}` : "") +
          (extra.length ? `; unexpected ${extra.join(" ")}` : ""),
      );
    }
  }

  const encoders = listedNames(input.encodersText);
  for (const encoder of FORBIDDEN_ENCODERS) {
    if (encoders.has(encoder)) {
      fail(
        "no-software-h264-hevc-encoder",
        `encoder ${encoder} is present; ADR-0003 forbids shipping it`,
      );
    }
  }
  for (const encoder of REQUIRED_ENCODERS) {
    if (!encoders.has(encoder)) {
      fail("required-components", `required encoder ${encoder} is missing`);
    }
  }
  const filters = listedNames(input.filtersText);
  for (const filter of REQUIRED_FILTERS) {
    if (!filters.has(filter)) {
      fail("required-components", `required filter ${filter} is missing`);
    }
  }

  for (const [name, expected] of Object.entries(input.lock.binaries)) {
    const actual = input.hashes[name];
    if (actual !== expected) {
      fail(
        "provenance",
        `${name} SHA-256 is ${actual ?? "(missing)"}, sidecar.lock.json records ${expected}`,
      );
    }
  }
  if (!input.thirdParty.includes(`FFmpeg ${input.lock.source.tag}`)) {
    fail(
      "provenance",
      `THIRD_PARTY.md does not name FFmpeg ${input.lock.source.tag}, the pinned version`,
    );
  }
  const trackedBinaries = input.trackedFiles.filter((file) =>
    /(^|\/)(ffmpeg|ffprobe|ffplay)[^/]*\.(exe|dll)$/i.test(file),
  );
  for (const file of trackedBinaries) {
    fail(
      "provenance",
      `${file} is committed; the sidecar is fetched by pnpm sidecar:fetch, never tracked`,
    );
  }

  return violations;
}

function run(binary, args) {
  const result = spawnSync(binary, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  return `${result.stdout}${result.stderr}`;
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function binaryPath(dir, name) {
  return join(dir, `${name}-${TRIPLE}.exe`);
}

function main() {
  const dir = process.argv[2] ?? DEFAULT_BINARIES;
  const ffmpeg = binaryPath(dir, "ffmpeg");
  const ffprobe = binaryPath(dir, "ffprobe");
  for (const binary of [ffmpeg, ffprobe]) {
    if (!existsSync(binary)) {
      console.error(
        `sidecar gate: ${binary} is missing. Run pnpm sidecar:fetch.`,
      );
      process.exit(1);
    }
  }

  const tracked = spawnSync("git", ["ls-files"], {
    cwd: ROOT,
    encoding: "utf8",
  });
  const violations = evaluate({
    versionText: run(ffmpeg, ["-hide_banner", "-version"]),
    probeVersionText: run(ffprobe, ["-hide_banner", "-version"]),
    encodersText: run(ffmpeg, ["-hide_banner", "-encoders"]),
    filtersText: run(ffmpeg, ["-hide_banner", "-filters"]),
    configureText: readFileSync(join(SIDECAR, "configure.txt"), "utf8"),
    lock: JSON.parse(readFileSync(join(SIDECAR, "sidecar.lock.json"), "utf8")),
    hashes: {
      "ffmpeg.exe": sha256(ffmpeg),
      "ffprobe.exe": sha256(ffprobe),
    },
    thirdParty: readFileSync(join(ROOT, "THIRD_PARTY.md"), "utf8"),
    trackedFiles: tracked.stdout.split(/\r?\n/).filter(Boolean),
  });

  if (violations.length > 0) {
    for (const { rule, message } of violations) {
      console.error(`sidecar gate [${rule}]: ${message}`);
    }
    process.exit(1);
  }
  console.log(
    `sidecar gate: LGPL-only, built from the committed configuration, ${REQUIRED_ENCODERS.length} encoders and ${REQUIRED_FILTERS.length} filters present, hashes match the lock.`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main();
}
