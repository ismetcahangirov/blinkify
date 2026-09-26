#!/usr/bin/env node
/**
 * Generate the media test corpus.
 *
 * `CLAUDE.md` section 13 and forbidden behaviour 12: the corpus is generated
 * or fetched, never committed. This script generates it into `corpus/`
 * (ignored by git) from recipes below, each producing a file with one property
 * the engine must get right — a portrait rotation, variable frame rate, an
 * edit list, HDR mastering metadata, an open GOP.
 *
 * **The corpus tool is not the sidecar.** Real footage is overwhelmingly H.264
 * and HEVC with B-frames, and producing that needs x264 and x265, which are GPL
 * and which ADR-0002 keeps out of Blinkify. So the recipes run a separate,
 * pinned GPL FFmpeg build (`corpus-tool.lock.json`) that lives in `target/`,
 * is never bundled, never linked, and never run by Blinkify — only by this
 * script, to make test inputs. The code under test only ever runs the bundled
 * LGPL sidecar. See docs/engineering/test-corpus.md.
 *
 * Usage:
 *   node tools/corpus/generate.mjs          generate anything missing or stale
 *   node tools/corpus/generate.mjs --force  regenerate everything
 *   node tools/corpus/generate.mjs --verify generate again elsewhere and
 *                                           require identical bytes
 */
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const CORPUS = join(ROOT, "corpus");
const TOOL_DIR = join(ROOT, "target", "corpus-tool");
const LOCK = JSON.parse(
  readFileSync(join(HERE, "corpus-tool.lock.json"), "utf8"),
);
const FORCE = process.argv.includes("--force");
const VERIFY = process.argv.includes("--verify");

// Windows' own bsdtar, by full path: it reads zip archives, and a GNU tar
// earlier on PATH (Git Bash ships one) reads `C:` as a remote host name.
const TAR =
  process.platform === "win32"
    ? join(process.env.SystemRoot ?? "C:/Windows", "System32", "tar.exe")
    : "tar";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

const VIDEO = (size = "640x360", rate = 30, seconds = 4) => [
  "-f",
  "lavfi",
  "-i",
  `testsrc2=size=${size}:rate=${rate}:duration=${seconds}`,
];
const TONE = (seconds = 4, layout = "stereo") => [
  "-f",
  "lavfi",
  "-i",
  `sine=frequency=440:sample_rate=48000:duration=${seconds},aformat=channel_layouts=${layout}`,
];

/**
 * Recordings the recipes start from, fetched and checked against their
 * SHA-256 rather than synthesised: noise reduction (#47) is trained on
 * speech, and no generator makes speech. Each is public domain, cached in
 * `target/corpus-inputs/`, and never committed.
 */
const INPUTS = {
  // LibriVox, "The Gettysburg Address", read by John Greenman. Public domain.
  speech: {
    url: "https://archive.org/download/gettysburg_johng_librivox/gettysburg_address.mp3",
    sha256: "66ffab650fff988fc9ce55457f6112f828815cf37d263dab49eb10ae5a193346",
  },
};

/**
 * One file per property. `steps` run in order; `{out}` is the file being made,
 * `{corpus}` the corpus directory for inputs made by an earlier recipe.
 */
// prettier-ignore
const RECIPES = [
  {
    file: "h264-high-closed-gop.mp4",
    note: "H.264 High, B-frames, closed GOP, keyframes at irregular times, AAC stereo",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libx264", "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-x264-params", "keyint=300:min-keyint=1:scenecut=0:bframes=3:open-gop=0",
        "-force_key_frames", "0,0.7,1.9,2.2,3.5",
        "-c:a", "aac", "-b:a", "128k",
        "{out}",
      ],
    ],
  },
  {
    file: "h264-open-gop.mp4",
    note: "H.264 High with open GOPs: non-IDR recovery-point keyframes with leading B-frames",
    steps: [
      [
        ...VIDEO(),
        "-c:v", "libx264", "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-x264-params", "keyint=30:min-keyint=30:scenecut=0:bframes=3:b-pyramid=normal:open-gop=1",
        "{out}",
      ],
    ],
  },
  {
    file: "hevc-open-gop.mp4",
    note: "HEVC Main with CRA keyframes and RASL leading pictures",
    steps: [
      [
        ...VIDEO(),
        "-c:v", "libx265", "-pix_fmt", "yuv420p", "-tag:v", "hvc1",
        "-x265-params", "keyint=30:min-keyint=30:scenecut=0:bframes=4:open-gop=1:log-level=error",
        "{out}",
      ],
    ],
  },
  {
    file: "hevc-closed-gop-radl.mp4",
    note: "HEVC Main with IDR_W_RADL keyframes: leading pictures, but a closed GOP",
    steps: [
      [
        ...VIDEO(),
        "-c:v", "libx265", "-pix_fmt", "yuv420p", "-tag:v", "hvc1",
        "-x265-params", "keyint=30:min-keyint=30:scenecut=0:bframes=4:open-gop=0:radl=2:log-level=error",
        "{out}",
      ],
    ],
  },
  {
    file: "hevc-hdr10.mp4",
    note: "HEVC Main 10, BT.2020 / PQ, mastering display and content light level",
    steps: [
      [
        ...VIDEO("1280x720", 30, 2),
        "-c:v", "libx265", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
        "-color_primaries", "bt2020", "-color_trc", "smpte2084",
        "-colorspace", "bt2020nc", "-color_range", "tv",
        "-x265-params",
        "hdr10=1:repeat-headers=1:colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):max-cll=1000,400:log-level=error",
        "{out}",
      ],
    ],
  },
  {
    file: "portrait-phone.mp4",
    note: "A 1280x720 H.264 stream stored landscape with a 90-degree display rotation",
    steps: [
      [
        ...VIDEO("1280x720", 30, 2),
        ...TONE(2),
        "-c:v", "libx264", "-profile:v", "high", "-pix_fmt", "yuv420p",
        "-c:a", "aac",
        "{tmp}",
      ],
      ["-display_rotation:v:0", "90", "-i", "{tmp}", "-c", "copy", "{out}"],
    ],
  },
  {
    file: "vfr-screen.mp4",
    note: "Variable frame rate: 30 fps for two seconds, then 10 fps; keyframes at 0, 1, 2.5 and 3.2 s",
    steps: [
      [
        ...VIDEO("640x360", 30, 4),
        "-vf", "setpts='if(lt(N,60),N/30,2+(N-60)/10)/TB'",
        "-fps_mode", "passthrough",
        // Keyframes on both sides of the rate change, at times the tests know.
        "-force_key_frames", "0,1,2.5,3.2",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
        "-video_track_timescale", "90000",
        "{out}",
      ],
    ],
  },
  {
    file: "edit-list.mp4",
    note: "Stream-copied from 1 s into a GOP: the MP4 edit list hides the pre-roll",
    steps: [
      ["-ss", "1.0", "-i", "{corpus}/h264-high-closed-gop.mp4", "-c", "copy", "{out}"],
    ],
  },
  {
    file: "multi-audio.mkv",
    note: "VP9 video, Opus stereo (aze) and AAC 5.1 (eng) audio, two chapters",
    steps: [
      [
        ...VIDEO(),
        ...TONE(4, "stereo"),
        ...TONE(4, "5.1"),
        "-f", "ffmetadata", "-i", "{chapters}",
        "-map", "0:v", "-map", "1:a", "-map", "2:a", "-map_chapters", "3",
        "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "40", "-deadline", "realtime",
        "-threads", "1",
        "-c:a:0", "libopus", "-c:a:1", "aac",
        "-metadata:s:a:0", "language=aze", "-metadata:s:a:1", "language=eng",
        "-fflags", "+bitexact",
        "{out}",
      ],
    ],
  },
  {
    file: "vp9.webm",
    note: "VP9 profile 0 and Opus",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "40", "-deadline", "realtime",
        "-threads", "1",
        "-c:a", "libopus",
        "-fflags", "+bitexact",
        "{out}",
      ],
    ],
  },
  {
    file: "av1.mp4",
    note: "AV1 Main and AAC",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libsvtav1", "-preset", "12", "-crf", "45",
        "-c:a", "aac",
        "{out}",
      ],
    ],
  },
  {
    file: "hevc-main10.mp4",
    note: "HEVC Main 10, SDR (BT.709): ten bits without HDR, a keyframe every second, AAC",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libx265", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
        "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709",
        "-x265-params", "keyint=30:min-keyint=30:scenecut=0:bframes=4:open-gop=0:frame-threads=1:pools=none:log-level=error",
        "-c:a", "aac",
        "{out}",
      ],
    ],
  },
  {
    file: "h264-high10.mp4",
    note: "H.264 High 10: ten-bit H.264, a keyframe every second",
    steps: [
      [
        ...VIDEO(),
        "-c:v", "libx264", "-profile:v", "high10", "-pix_fmt", "yuv420p10le",
        "-x264-params", "keyint=30:min-keyint=30:scenecut=0:bframes=3:open-gop=0:threads=1",
        "{out}",
      ],
    ],
  },
  {
    file: "vp9-keyframes.webm",
    note: "VP9 with a keyframe every second: GOPs a copy and a smart-cut can cut between",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libvpx-vp9", "-b:v", "0", "-crf", "40", "-deadline", "realtime",
        "-g", "30", "-keyint_min", "30", "-threads", "1",
        "-c:a", "libopus",
        "-fflags", "+bitexact",
        "{out}",
      ],
    ],
  },
  {
    file: "speech-clean.flac",
    note: "Twenty seconds of a man reading, mono 48 kHz: the reference a denoiser is measured against",
    steps: [
      [
        "-i", "{input:speech}",
        "-af", "atrim=10:30,asetpts=PTS-STARTPTS,pan=mono|c0=0.5*c0+0.5*c1,aresample=48000",
        "-c:a", "flac", "-sample_fmt", "s16",
        "-fflags", "+bitexact", "-flags:a", "+bitexact",
        "{out}",
      ],
    ],
  },
  {
    file: "speech-noisy.flac",
    note: "The same reading under steady pink noise: what a room with a fan sounds like",
    steps: [
      [
        "-i", "{corpus}/speech-clean.flac",
        "-f", "lavfi", "-i", "anoisesrc=d=20:c=pink:a=0.08:seed=11:r=48000",
        "-filter_complex", "[0][1]amix=inputs=2:normalize=0:duration=first",
        "-c:a", "flac", "-sample_fmt", "s16",
        "-fflags", "+bitexact", "-flags:a", "+bitexact",
        "{out}",
      ],
    ],
  },
  {
    file: "av1-keyframes.mkv",
    note: "AV1 with a keyframe every second",
    steps: [
      [
        ...VIDEO(),
        ...TONE(),
        "-c:v", "libsvtav1", "-preset", "12", "-crf", "45",
        "-g", "30", "-svtav1-params", "lp=1",
        "-c:a", "libopus",
        "-fflags", "+bitexact",
        "{out}",
      ],
    ],
  },
];

const CHAPTERS = `;FFMETADATA1
[CHAPTER]
TIMEBASE=1/1000
START=0
END=2000
title=Opening
[CHAPTER]
TIMEBASE=1/1000
START=2000
END=4000
title=Closing
`;

// Matroska and WebM outputs carry `-fflags +bitexact`: the muxer otherwise
// writes random segment and track UIDs and the wall-clock date, and the same
// recipe would not give the same bytes twice (`--verify`).

/** The recipes' own fingerprint: a change to any of them regenerates. */
const RECIPE_HASH = sha256(JSON.stringify({ RECIPES, CHAPTERS, LOCK, INPUTS }));

/**
 * Download with retries: GitHub's release CDN answers the occasional 5xx, and
 * a run that fails on one teaches people to re-run without reading. The bytes
 * are checked against the lock either way.
 */
async function download(url) {
  const attempts = 4;
  for (let attempt = 1; ; attempt += 1) {
    try {
      const response = await fetch(url, { redirect: "follow" });
      if (response.ok) return Buffer.from(await response.arrayBuffer());
      if (response.status < 500 || attempt === attempts) {
        throw new Error(`HTTP ${response.status} for ${url}`);
      }
      console.log(`corpus: HTTP ${response.status}, retrying`);
    } catch (error) {
      if (attempt === attempts) throw error;
      console.log(`corpus: ${error.message}, retrying`);
    }
    await new Promise((resolve) => setTimeout(resolve, attempt * 5000));
  }
}

async function ensureTool() {
  const exe = join(TOOL_DIR, LOCK.folder, "bin", "ffmpeg.exe");
  if (existsSync(exe)) return exe;
  console.log(`corpus: fetching the corpus tool (${LOCK.url})`);
  const zip = await download(LOCK.url);
  if (sha256(zip) !== LOCK.sha256) {
    throw new Error("corpus tool SHA-256 does not match corpus-tool.lock.json");
  }
  mkdirSync(TOOL_DIR, { recursive: true });
  const zipPath = join(TOOL_DIR, "tool.zip");
  writeFileSync(zipPath, zip);
  const untar = spawnSync(TAR, ["-xf", zipPath, "-C", TOOL_DIR], {
    stdio: "inherit",
  });
  rmSync(zipPath, { force: true });
  if (untar.status !== 0) throw new Error("could not extract the corpus tool");
  return exe;
}

function run(tool, args) {
  const result = spawnSync(
    tool,
    ["-hide_banner", "-loglevel", "error", "-y", ...args],
    { encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error(`corpus tool failed: ${args.join(" ")}\n${result.stderr}`);
  }
}

/** Every input, fetched once into `target/corpus-inputs/` and checked. */
async function ensureInputs() {
  const dir = join(ROOT, "target", "corpus-inputs");
  mkdirSync(dir, { recursive: true });
  const paths = {};
  for (const [name, input] of Object.entries(INPUTS)) {
    const path = join(
      dir,
      `${name}${input.url.slice(input.url.lastIndexOf("."))}`,
    );
    if (!existsSync(path) || sha256(readFileSync(path)) !== input.sha256) {
      console.log(`corpus: fetching ${input.url}`);
      const bytes = await download(input.url);
      if (sha256(bytes) !== input.sha256) {
        throw new Error(`${name}: SHA-256 does not match the recipe`);
      }
      writeFileSync(path, bytes);
    }
    paths[name] = path;
  }
  return paths;
}

/**
 * Make every recipe's file in `dir`, and return each file's note and SHA-256.
 */
function generate(tool, dir, inputs) {
  mkdirSync(dir, { recursive: true });
  const chapters = join(dir, "chapters.ffmetadata");
  writeFileSync(chapters, CHAPTERS);
  const files = {};
  for (const recipe of RECIPES) {
    const out = join(dir, recipe.file);
    const partial = `${out}.partial${recipe.file.slice(recipe.file.lastIndexOf("."))}`;
    const tmp = join(dir, `tmp-${recipe.file}`);
    for (const step of recipe.steps) {
      run(
        tool,
        step.map((arg) =>
          arg
            .replace("{out}", partial)
            .replace("{tmp}", tmp)
            // Inputs made by an earlier recipe come from this run's own
            // directory, so a verification run reads only what it made.
            .replace("{corpus}", dir)
            .replace(/\{input:(\w+)\}/, (_, name) => inputs[name])
            .replace("{chapters}", chapters),
        ),
      );
    }
    rmSync(tmp, { force: true });
    renameSync(partial, out);
    files[recipe.file] = {
      note: recipe.note,
      sha256: sha256(readFileSync(out)),
    };
    console.log(`corpus: ${recipe.file} — ${recipe.note}`);
  }
  rmSync(chapters, { force: true });
  return files;
}

/**
 * Generate the corpus again, elsewhere, and compare every file with the
 * manifest: the corpus is reproducible only if the same recipes give the same
 * bytes. Fails naming each file that differs.
 */
async function verify() {
  const manifestPath = join(CORPUS, "manifest.json");
  if (!existsSync(manifestPath)) {
    throw new Error("no corpus to verify against; run `pnpm corpus` first");
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  if (manifest.recipeHash !== RECIPE_HASH) {
    throw new Error("the corpus is stale; run `pnpm corpus` first");
  }
  const tool = await ensureTool();
  const inputs = await ensureInputs();
  const scratch = join(ROOT, "target", "corpus-verify");
  rmSync(scratch, { recursive: true, force: true });
  const again = generate(tool, scratch, inputs);
  rmSync(scratch, { recursive: true, force: true });
  const differ = Object.entries(again)
    .filter(([file, entry]) => manifest.files[file]?.sha256 !== entry.sha256)
    .map(([file]) => file);
  if (differ.length > 0) {
    throw new Error(`not reproducible: ${differ.join(", ")}`);
  }
  console.log(`corpus: all ${RECIPES.length} files reproduced byte for byte.`);
}

async function main() {
  if (VERIFY) return verify();
  mkdirSync(CORPUS, { recursive: true });
  const manifestPath = join(CORPUS, "manifest.json");
  const manifest = existsSync(manifestPath)
    ? JSON.parse(readFileSync(manifestPath, "utf8"))
    : {};
  const fresh =
    !FORCE &&
    manifest.recipeHash === RECIPE_HASH &&
    RECIPES.every((recipe) => existsSync(join(CORPUS, recipe.file)));
  if (fresh) {
    console.log(`corpus: ${RECIPES.length} files present and current.`);
    return;
  }

  const tool = await ensureTool();
  const files = generate(tool, CORPUS, await ensureInputs());
  writeFileSync(
    manifestPath,
    `${JSON.stringify({ recipeHash: RECIPE_HASH, files }, null, 2)}\n`,
  );
}

main().catch((error) => {
  console.error(`corpus: ${error.message}`);
  process.exit(1);
});
