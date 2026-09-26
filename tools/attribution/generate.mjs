#!/usr/bin/env node
/**
 * Write the third-party attribution document (#73).
 *
 * Reads `cargo metadata` and the installed renderer dependencies, attributes
 * every package, and writes `apps/desktop/src-tauri/THIRD-PARTY-NOTICES.txt`,
 * which the installer bundles and the application shows under Help. Fails —
 * naming every package it could not attribute — rather than write a document
 * with a package missing.
 *
 * Run: pnpm attribution
 *      pnpm attribution --stdout     print instead of writing
 *
 * The inputs can be pointed elsewhere; see `optionsFromArgs`.
 */
import { writeFileSync } from "node:fs";
import { relative } from "node:path";
import {
  AttributionError,
  ROOT,
  generate,
  optionsFromArgs,
} from "./attribution.mjs";

const args = process.argv.slice(2);
let options;
let document;
try {
  options = optionsFromArgs(args);
  document = generate(options);
} catch (error) {
  console.error(
    error instanceof AttributionError
      ? `attribution: ${error.message}`
      : `attribution: generation failed.\n${error.message}`,
  );
  process.exit(1);
}

if (args.includes("--stdout")) {
  process.stdout.write(document);
} else {
  writeFileSync(options.document, document);
  console.log(
    `attribution: wrote ${relative(ROOT, options.document).replaceAll("\\", "/")} ` +
      `(${Buffer.byteLength(document)} bytes).`,
  );
}
