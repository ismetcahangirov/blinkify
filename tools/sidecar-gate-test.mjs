#!/usr/bin/env node
/**
 * Injection test for the FFmpeg sidecar gate.
 *
 * `pnpm sidecar:check` passing proves the binary on this machine is clean. It
 * does not prove the gate would notice a GPL one — and #21 requires exactly
 * that proof: "substitute a GPL build and confirm CI fails". `CLAUDE.md`
 * section 14: a rule you have not seen fail is a rule you have not tested.
 *
 * Each case starts from a clean report that passes, changes one thing, and
 * asserts the named rule fires. The configuration strings are not invented:
 * `DISTRIBUTOR_LGPL` is the line printed by BtbN's `win64-lgpl-8.1` build of
 * FFmpeg n8.1.3, which is the most plausible thing someone would drop in by
 * hand, and it must fail — it carries `openh264` and `--enable-version3`.
 *
 * Run: pnpm sidecar:check:test
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  REQUIRED_ENCODERS,
  REQUIRED_FILTERS,
  committedFlags,
  evaluate,
} from "./sidecar-gate.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");
const CONFIGURE = readFileSync(
  join(HERE, "ffmpeg-sidecar", "configure.txt"),
  "utf8",
);
const LOCK = JSON.parse(
  readFileSync(join(HERE, "ffmpeg-sidecar", "sidecar.lock.json"), "utf8"),
);
const THIRD_PARTY = readFileSync(join(ROOT, "THIRD_PARTY.md"), "utf8");

const TOOLCHAIN =
  "--prefix=/work/prefix --pkg-config-flags=--static --cross-prefix=x86_64-w64-mingw32- --arch=x86_64 --target-os=mingw32 --extra-cflags= --extra-libs=-lgomp --cc=x86_64-w64-mingw32-gcc --extra-version=blinkify";

const DISTRIBUTOR_LGPL =
  "--prefix=/ffbuild/prefix --pkg-config-flags=--static --pkg-config=pkg-config --cross-prefix=x86_64-w64-mingw32- --arch=x86_64 --target-os=mingw32 --enable-version3 --disable-debug --disable-w32threads --enable-pthreads --enable-iconv --enable-zlib --enable-libxml2 --enable-libvmaf --enable-fontconfig --enable-libharfbuzz --enable-libfreetype --enable-libfribidi --enable-vulkan --enable-libshaderc --enable-libdav1d --enable-libvorbis --enable-librav1e --enable-librsvg --disable-libxcb --disable-xlib --disable-libpulse --enable-gmp --enable-lzma --enable-liblcevc-dec --enable-opencl --enable-amf --enable-libaom --disable-avisynth --enable-chromaprint --disable-libdavs2 --disable-libdvdread --disable-libdvdnav --disable-libfdk-aac --enable-ffnvcodec --enable-cuda-llvm --disable-frei0r --enable-libgme --enable-libkvazaar --enable-libaribb24 --enable-libaribcaption --enable-libass --enable-libbluray --enable-libjxl --enable-libmp3lame --enable-libopus --enable-libplacebo --enable-librist --enable-libssh --enable-libtheora --enable-libvpx --enable-libwebp --enable-libzmq --enable-lv2 --enable-libvpl --enable-openal --enable-liboapv --enable-libopencore-amrnb --enable-libopencore-amrwb --enable-libopenh264 --enable-libopenjpeg --enable-libopenmpt --disable-librubberband --enable-schannel --enable-sdl2 --enable-libsnappy --enable-libsoxr --enable-libsrt --enable-libsvtav1 --enable-libtwolame --enable-libuavs3d --disable-libdrm --enable-vaapi --disable-libvidstab --enable-libvvenc --disable-whisper --disable-libx264 --disable-libx265 --disable-libxavs2 --disable-libxvid --enable-libzimg --enable-libzvbi --extra-version=20260922";

const version = (flags) =>
  `ffmpeg version n8.1.3-blinkify Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with gcc 16.2.0\nconfiguration: ${flags}\nlibavutil      60. 26.103 / 60. 26.103\n`;

const listing = (title, flags, names) =>
  [
    `${title}:`,
    ` ${flags} = Legend`,
    " ------",
    ...names.map((name) => ` ${flags} ${name.padEnd(20)} Description`),
  ].join("\n");

const OURS = `${TOOLCHAIN} ${committedFlags(CONFIGURE).join(" ")}`;

function cleanInput() {
  return {
    versionText: version(OURS),
    probeVersionText: version(OURS),
    encodersText: listing("Encoders", "V....D", REQUIRED_ENCODERS),
    filtersText: listing("Filters", "..", REQUIRED_FILTERS),
    configureText: CONFIGURE,
    lock: LOCK,
    hashes: { ...LOCK.binaries },
    thirdParty: THIRD_PARTY,
    trackedFiles: ["README.md", "tools/sidecar-gate.mjs"],
  };
}

/** @type {{name: string, mutate: (input: object) => void, rule: string | null}[]} */
const CASES = [
  {
    name: "control: the committed build passes",
    mutate: () => {},
    rule: null,
  },
  {
    name: "a GPL build (--enable-gpl --enable-libx264) is rejected",
    mutate: (input) => {
      input.versionText = version(
        `${OURS} --enable-gpl --enable-libx264 --enable-libx265`,
      );
    },
    rule: "no-gpl-configuration",
  },
  {
    name: "a non-free build is rejected",
    mutate: (input) => {
      input.probeVersionText = version(`${OURS} --enable-nonfree`);
    },
    rule: "no-gpl-configuration",
  },
  {
    name: "a GPL library enabled without --enable-gpl is still rejected",
    mutate: (input) => {
      input.versionText = version(`${OURS} --enable-libvidstab`);
    },
    rule: "no-gpl-configuration",
  },
  {
    name: "the distributor's own 'LGPL' build is rejected for openh264",
    mutate: (input) => {
      input.versionText = version(DISTRIBUTOR_LGPL);
    },
    rule: "no-software-h264-hevc-encoder",
  },
  {
    name: "the distributor's 'LGPL' build is not our configuration",
    mutate: (input) => {
      input.versionText = version(DISTRIBUTOR_LGPL);
    },
    rule: "configuration-is-ours",
  },
  {
    name: "libx264 in the encoder list is rejected even if the flags look clean",
    mutate: (input) => {
      input.encodersText = listing("Encoders", "V....D", [
        ...REQUIRED_ENCODERS,
        "libx264",
      ]);
    },
    rule: "no-software-h264-hevc-encoder",
  },
  {
    name: "a licence-clean build missing arnndn fails",
    mutate: (input) => {
      input.filtersText = listing(
        "Filters",
        "..",
        REQUIRED_FILTERS.filter((f) => f !== "arnndn"),
      );
    },
    rule: "required-components",
  },
  {
    name: "a licence-clean build missing hevc_nvenc fails",
    mutate: (input) => {
      input.encodersText = listing(
        "Encoders",
        "V....D",
        REQUIRED_ENCODERS.filter((e) => e !== "hevc_nvenc"),
      );
    },
    rule: "required-components",
  },
  {
    name: "a binary that is not the pinned one fails",
    mutate: (input) => {
      input.hashes["ffmpeg.exe"] = "0".repeat(64);
    },
    rule: "provenance",
  },
  {
    name: "a committed ffmpeg.exe fails",
    mutate: (input) => {
      input.trackedFiles.push("apps/desktop/src-tauri/binaries/ffmpeg.exe");
    },
    rule: "provenance",
  },
  {
    name: "THIRD_PARTY.md naming another version fails",
    mutate: (input) => {
      input.thirdParty = input.thirdParty.replaceAll(
        `FFmpeg ${LOCK.source.tag}`,
        "FFmpeg n7.0",
      );
    },
    rule: "provenance",
  },
];

let failures = 0;
for (const testCase of CASES) {
  const input = cleanInput();
  testCase.mutate(input);
  const violations = evaluate(input);
  const rules = new Set(violations.map((v) => v.rule));
  const ok =
    testCase.rule === null ? violations.length === 0 : rules.has(testCase.rule);
  if (ok) {
    console.log(`  ok    ${testCase.name}`);
  } else {
    failures += 1;
    console.error(`  FAIL  ${testCase.name}`);
    console.error(
      `        expected ${testCase.rule ?? "no violation"}, got: ${
        violations.map((v) => `[${v.rule}] ${v.message}`).join("; ") || "none"
      }`,
    );
  }
}

if (failures > 0) {
  console.error(`\nsidecar gate injection test: ${failures} case(s) failed.`);
  process.exit(1);
}
console.log(
  `\nsidecar gate injection test: all ${CASES.length} cases behaved.`,
);
