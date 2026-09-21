#!/usr/bin/env node
/**
 * Proof that the generator is a pure function of the source tree.
 *
 * `pnpm graph:check` regenerates the graph and fails on a non-empty diff. That
 * gate is only meaningful if a diff can mean one thing: the source tree moved.
 * The moment the generator reads a clock, or leaves one collection unsorted,
 * every run produces a diff — and a signal that always fires is a signal nobody
 * reads, at which point the gate is removed and the blast-radius report is
 * quietly wrong from then on.
 *
 * So: run the generator twice over an unchanged tree and compare the artifacts
 * byte for byte.
 *
 * The output is regenerated in place, which is what `pnpm graph` does anyway.
 * A failure leaves the second run's output on disk — that is the one to look
 * at, since the difference between the two is the non-determinism.
 *
 * Run: pnpm graph:determinism
 */
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..", "..");
const ARTIFACTS = ["graph.json", "index.json", "GRAPH.md"];

function generate(run) {
  const result = spawnSync(process.execPath, [join(HERE, "generate.mjs")], {
    cwd: ROOT,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    console.error(`graph:determinism: run ${run} failed.`);
    console.error(`${result.stdout ?? ""}${result.stderr ?? ""}`);
    process.exit(1);
  }
  return Object.fromEntries(
    ARTIFACTS.map((name) => [name, readFileSync(join(HERE, "output", name))]),
  );
}

const first = generate(1);
const second = generate(2);

let failed = 0;
for (const name of ARTIFACTS) {
  if (first[name].equals(second[name])) {
    console.log(
      `  ok   ${name} — byte-identical across two runs (${first[name].length} bytes)`,
    );
    continue;
  }

  failed += 1;
  console.error(
    `  FAIL ${name} differs between two runs of an unchanged tree.`,
  );
  console.error(
    `       ${first[name].length} bytes then ${second[name].length} bytes.` +
      (first[name].length === second[name].length
        ? " Same length, so the content is reordered rather than changed —\n" +
          "       an unsorted collection, most likely."
        : " A clock reading or an environment value has leaked in."),
  );

  // Find the first differing byte and show its neighbourhood. A reordered
  // 400 KB JSON file is otherwise unreadable in a diff.
  const shortest = Math.min(first[name].length, second[name].length);
  let at = 0;
  while (at < shortest && first[name][at] === second[name][at]) at += 1;
  const window = (buf) =>
    JSON.stringify(buf.subarray(Math.max(0, at - 60), at + 60).toString());
  console.error(`       first difference at byte ${at}:`);
  console.error(`         run 1: ${window(first[name])}`);
  console.error(`         run 2: ${window(second[name])}`);
}

if (failed > 0) {
  console.error(
    "\ngraph:determinism: the generator is not a pure function of the source tree.\n" +
      "CLAUDE.md section 14: it must read no clock and must sort every collection.",
  );
  process.exit(1);
}

console.log(
  "\ngraph:determinism: two runs over an unchanged tree agree byte for byte.",
);
