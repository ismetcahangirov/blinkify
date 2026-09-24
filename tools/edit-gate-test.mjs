#!/usr/bin/env node
/**
 * Injection test for the edit boundary (#37).
 *
 * `pnpm edits:check` passing proves nothing bypasses the history today. It
 * does not prove the gate would notice — CLAUDE.md section 14: a rule you
 * have not seen fail is a rule you have not tested. Each case hands the gate
 * one file that does not exist and asserts it is caught, or let through.
 *
 * Run: pnpm edits:check:test
 */
import { violations } from "./edit-gate.mjs";

const cases = [
  [
    "the shell changing a project in place",
    "apps/desktop/src-tauri/src/project.rs",
    "fn rename(project: &mut Project, name: &str) {",
    true,
  ],
  [
    "an engine module binding a mutable project",
    "crates/blinkify-engine/src/export.rs",
    "let mut draft: Project = document.project().clone();",
    true,
  ],
  [
    "a component invoking an edit directly",
    "apps/desktop/src/timeline/TimelineCanvas.tsx",
    'void invoke("edit_project", { edit });',
    true,
  ],
  [
    "a component undoing directly",
    "apps/desktop/src/shell/AppBar.tsx",
    "invoke('undo_edit')",
    true,
  ],
  [
    "the edit layer itself",
    "crates/blinkify-engine/src/project/edit.rs",
    "fn apply(self, project: &mut Project) -> Result<Self, EditError> {",
    false,
  ],
  [
    "an engine integration test building a graph",
    "crates/blinkify-engine/tests/evaluate.rs",
    "let mut project: Project = Project::new(name, settings);",
    false,
  ],
  [
    "the project store",
    "apps/desktop/src/project/project.store.ts",
    'await invoke<EditOutcome>("edit_project", { edit, context });',
    false,
  ],
  [
    "a renderer test asserting the command",
    "apps/desktop/src/project/SourcesBanner.test.tsx",
    'expect(invoked).toHaveBeenCalledWith("edit_project", {});',
    false,
  ],
  [
    "a read-only borrow",
    "apps/desktop/src-tauri/src/project.rs",
    "fn view(project: &Project) -> ProjectView {",
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

// A `#[cfg(test)]` module inside a source file builds graphs too.
const unitTests = violations(
  "apps/desktop/src-tauri/src/project.rs",
  "fn real() {}\n#[cfg(test)]\nmod tests {\n    fn f(p: &mut Project) {}\n}\n",
);
if (unitTests.length === 0) console.log("ok   a unit-test module");
else {
  failed += 1;
  console.error(`FAIL a unit-test module: ${JSON.stringify(unitTests)}`);
}

if (failed > 0) process.exit(1);
