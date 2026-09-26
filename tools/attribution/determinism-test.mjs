#!/usr/bin/env node
/**
 * Proof that the attribution generator is a pure function of the tree.
 *
 * `pnpm attribution:check` fails on any difference from the committed
 * document. That is only a useful signal if a difference can mean one thing —
 * the dependency set moved. A clock reading, an absolute path out of the
 * cargo registry, or a collection left in directory-listing order would make
 * every run differ, and a gate that always fires is removed within a week.
 *
 * So: run the generator twice, in two separate processes over an unchanged
 * tree, and compare the output byte for byte. It also asserts that no path
 * from this machine reached the document, since the registry and
 * `node_modules` paths differ between a contributor's machine and CI.
 *
 * Run: pnpm attribution:determinism
 */
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { ROOT } from "./attribution.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

function generate(run) {
  const result = spawnSync(
    process.execPath,
    [join(HERE, "generate.mjs"), "--stdout"],
    { cwd: ROOT, maxBuffer: 256 * 1024 * 1024 },
  );
  if (result.status !== 0) {
    console.error(`attribution:determinism: run ${run} failed.`);
    console.error(`${result.stderr ?? ""}`);
    process.exit(1);
  }
  return result.stdout;
}

const first = generate(1);
const second = generate(2);
let failed = false;

if (first.equals(second)) {
  console.log(`  ok   byte-identical across two runs (${first.length} bytes)`);
} else {
  failed = true;
  const shortest = Math.min(first.length, second.length);
  let at = 0;
  while (at < shortest && first[at] === second[at]) at += 1;
  const window = (buf) =>
    JSON.stringify(buf.subarray(Math.max(0, at - 60), at + 60).toString());
  console.error("  FAIL two runs over an unchanged tree differ.");
  console.error(`       first difference at byte ${at}:`);
  console.error(`         run 1: ${window(first)}`);
  console.error(`         run 2: ${window(second)}`);
}

const text = first.toString("utf8");
const leaks = [ROOT, homedir()]
  .flatMap((path) => [path, path.replaceAll("\\", "/")])
  .filter((path) => text.includes(path));
if (leaks.length === 0) {
  console.log("  ok   no path from this machine is in the document");
} else {
  failed = true;
  console.error(
    `  FAIL the document contains a local path: ${[...new Set(leaks)].join(", ")}`,
  );
}

if (failed) {
  console.error(
    "\nattribution:determinism: the generator is not a pure function of the tree.\n" +
      "It must read no clock, print no local path and sort every collection.",
  );
  process.exit(1);
}

console.log(
  "\nattribution:determinism: two runs over an unchanged tree agree byte for byte.",
);
