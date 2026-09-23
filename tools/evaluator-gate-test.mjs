#!/usr/bin/env node
/**
 * Injection test for the evaluator boundary (#30).
 *
 * `pnpm evaluator:check` passing proves nothing interprets the graph today.
 * It does not prove the gate would notice — CLAUDE.md section 14: a rule you
 * have not seen fail is a rule you have not tested. Each case hands the gate
 * one file that does not exist and asserts it is caught, or let through.
 *
 * Run: pnpm evaluator:check:test
 */
import { violations } from "./evaluator-gate.mjs";

const cases = [
  [
    "the shell matching an Operation",
    "apps/desktop/src-tauri/src/media.rs",
    "match op { Operation::Gain { db } => db, _ => 0.0 }",
    true,
  ],
  [
    "the player matching an Operation",
    "crates/blinkify-engine/src/playback/feeder.rs",
    "if let Operation::Speed { ratio } = op {}",
    true,
  ],
  [
    "the renderer reading a clip's operations",
    "apps/desktop/src/timeline/drawClip.ts",
    "const trim = clip.operations.find((o) => o.op === 'trim');",
    true,
  ],
  [
    "the design system reading them",
    "packages/ui/src/components/Clip.tsx",
    "{clip.operations.length}",
    true,
  ],
  [
    "the evaluator itself",
    "crates/blinkify-engine/src/project/evaluate.rs",
    "Operation::Trim { from, to } => trim = Some((from, to)),",
    false,
  ],
  [
    "a test building a graph",
    "crates/blinkify-engine/tests/evaluate.rs",
    "vec![Operation::Trim { from: 0, to: 1 }]",
    false,
  ],
  [
    "the evaluator's own output",
    "crates/blinkify-engine/src/playback/feeder.rs",
    "AudioOperation::Gain { db } => db,",
    false,
  ],
  [
    "a renderer test fixture",
    "apps/desktop/src/project/SourcesBanner.test.tsx",
    "expect(view.operations).toBeUndefined();",
    false,
  ],
];

let failed = 0;
for (const [label, path, line, caught] of cases) {
  const found = violations(path, `// before\n${line}\n`);
  const ok = caught
    ? found.length === 1 && found[0].startsWith(`${path}:2:`)
    : found.length === 0;
  if (ok) console.log(`ok   ${label}`);
  else {
    failed += 1;
    console.error(`FAIL ${label}: ${JSON.stringify(found)}`);
  }
}
if (failed > 0) process.exit(1);
