#!/usr/bin/env node
/**
 * Fetch the pinned FFmpeg sidecar into `apps/desktop/src-tauri/binaries/`.
 *
 * The binaries are never committed (`CLAUDE.md` section 5; the sidecar gate
 * fails if one is tracked). They are a release asset, pinned by SHA-256 in
 * `tools/ffmpeg-sidecar/sidecar.lock.json`, and this script is the only way
 * they arrive on a developer's machine or a CI runner.
 *
 * Tauri requires every `externalBin` to exist before the shell crate compiles,
 * so this runs before `cargo build`, `cargo clippy` and `pnpm tauri build`.
 *
 * Every byte is checked twice: the downloaded zip against the lock, and each
 * extracted binary against the lock. A mismatch deletes what was written and
 * fails; nothing unverified is left where Tauri would bundle it.
 *
 * Usage:
 *   node tools/fetch-sidecar.mjs              download from the pinned URL
 *   node tools/fetch-sidecar.mjs --from <zip> use a local build of the same zip
 *
 * A developer machine is a build machine, not the user's: this download is
 * tooling, like `pnpm install`, and has nothing to do with the application's
 * rule against network calls.
 */
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { DEFAULT_BINARIES, binaryPath } from "./sidecar-gate.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const LOCK = JSON.parse(
  readFileSync(join(HERE, "ffmpeg-sidecar", "sidecar.lock.json"), "utf8"),
);
const NAMES = ["ffmpeg", "ffprobe"];

// Windows' own bsdtar, by full path: it reads zip archives, and a GNU tar
// earlier on PATH (Git Bash ships one) reads `C:` as a remote host name.
const TAR =
  process.platform === "win32"
    ? join(process.env.SystemRoot ?? "C:/Windows", "System32", "tar.exe")
    : "tar";

const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

function alreadyPresent() {
  return NAMES.every((name) => {
    const path = binaryPath(DEFAULT_BINARIES, name);
    return (
      existsSync(path) &&
      sha256(readFileSync(path)) === LOCK.binaries[`${name}.exe`]
    );
  });
}

async function download(url) {
  console.log(`sidecar: downloading ${url}`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    throw new Error(`download failed: HTTP ${response.status} for ${url}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

async function main() {
  if (alreadyPresent()) {
    console.log("sidecar: pinned binaries already present and verified.");
    return;
  }

  const fromIndex = process.argv.indexOf("--from");
  const zip =
    fromIndex > 0 && process.argv[fromIndex + 1]
      ? readFileSync(process.argv[fromIndex + 1])
      : await download(LOCK.asset.url);

  const zipHash = sha256(zip);
  if (zipHash !== LOCK.asset.sha256) {
    throw new Error(
      `zip SHA-256 is ${zipHash}, sidecar.lock.json records ${LOCK.asset.sha256}`,
    );
  }

  const work = mkdtempSync(join(tmpdir(), "blinkify-sidecar-"));
  try {
    const zipPath = join(work, "sidecar.zip");
    writeFileSync(zipPath, zip);
    // `tar` reads zip archives on Windows 10+ and on every CI image; it avoids
    // a zip dependency for one extraction.
    const untar = spawnSync(TAR, ["-xf", zipPath, "-C", work], {
      stdio: "inherit",
    });
    if (untar.status !== 0)
      throw new Error("could not extract the sidecar zip");

    mkdirSync(DEFAULT_BINARIES, { recursive: true });
    const folder = join(work, LOCK.asset.folder);
    for (const name of NAMES) {
      const source = join(folder, `${name}.exe`);
      const bytes = readFileSync(source);
      const hash = sha256(bytes);
      if (hash !== LOCK.binaries[`${name}.exe`]) {
        throw new Error(
          `${name}.exe SHA-256 is ${hash}, sidecar.lock.json records ${LOCK.binaries[`${name}.exe`]}`,
        );
      }
      copyFileSync(source, binaryPath(DEFAULT_BINARIES, name));
    }
    for (const file of ["LICENSE.txt", "SOURCE.md"]) {
      copyFileSync(
        join(folder, file),
        join(DEFAULT_BINARIES, `FFMPEG-${file}`),
      );
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  console.log(
    `sidecar: FFmpeg ${LOCK.source.tag} verified into ${DEFAULT_BINARIES}`,
  );
}

main().catch((error) => {
  for (const name of NAMES) {
    rmSync(binaryPath(DEFAULT_BINARIES, name), { force: true });
  }
  console.error(`sidecar: ${error.message}`);
  process.exit(1);
});
