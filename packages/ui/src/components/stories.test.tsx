import { composeStories } from "@storybook/react";
import type { ComponentType } from "react";
import { render } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, it } from "vitest";
import { TooltipProvider } from "./Tooltip.js";

/**
 * An automated accessibility check over every Storybook story (#17).
 *
 * ── Why the stories and not a separate list ─────────────────────────────────
 *
 * The stories already enumerate every variant and every state, because that is
 * what they are for. A second list of cases to audit would be a second list to
 * keep in step, and the one that rots is always the one nobody looks at. So the
 * suite discovers the stories rather than naming them: add a story, and it is
 * audited on the commit that adds it.
 *
 * ── Why axe in Vitest rather than the Storybook test runner ────────────────
 *
 * The test runner drives a real browser through Playwright. That buys real
 * colour-contrast measurement and costs a browser download and a minute of a
 * fifteen-minute pull-request budget (`CLAUDE.md` section 13). Contrast is
 * already measured — `tokens.test.ts` computes all 43 pairs from the token file
 * — so the browser would be bought for something we have.
 *
 * What is left is the structural half: names, roles, labels, ARIA wiring,
 * duplicated ids. That half runs in jsdom, and it is the half that catches the
 * defect this component set is most exposed to — an icon button with no
 * accessible name.
 *
 * ── What this cannot catch ─────────────────────────────────────────────────
 *
 *  - Colour contrast. jsdom computes no styles, so the rule is disabled here
 *    rather than reported as passing. `tokens.test.ts` owns it.
 *  - Focus order and visible focus. Both need layout.
 *  - Anything only reachable by interaction. A closed menu's items are not in
 *    the document; the stories that matter render their surface open.
 */

/* Every story module beside this file. `eager` so the suite is synchronous to
   collect — a lazily imported module produces a test list that depends on
   timing, and a test list that varies is a test list nobody trusts. */
const storyModules = import.meta.glob("./*.stories.tsx", { eager: true });

/**
 * The rules that do not apply to a component rendered on its own.
 *
 * Kept short and argued one by one, because a disable list is how an audit
 * quietly stops auditing. The final test in this file is the guard on that: it
 * renders a deliberately broken control and fails if axe does not catch it.
 */
const RULES_NOT_APPLICABLE = {
  /* Needs computed colour, which jsdom does not do. Every pair is measured
     from the token file by `tokens.test.ts`, so this is covered elsewhere
     rather than skipped. */
  "color-contrast": { enabled: false },

  /* Page-scope rules, all four. They ask whether the *document* is built
     correctly — that its content sits inside landmarks, that there is one
     `main`, that there is an `h1`, that there is a skip link. A story renders
     one control into a bare body, so every one of them fires on every story and
     none of them says anything about the control. They belong to the shell
     (#19), which is what actually builds a document, and they are not silenced
     there. */
  region: { enabled: false },
  "landmark-one-main": { enabled: false },
  "page-has-heading-one": { enabled: false },
  bypass: { enabled: false },
} as const;

interface StoryCase {
  readonly module: string;
  readonly name: string;
  readonly Story: ComponentType;
}

const cases: StoryCase[] = [];

/**
 * Collect the composed stories of one module.
 *
 * The parameter is `Record<string, unknown>` and the narrowing is a real
 * `typeof` check rather than an assertion. `composeStories` is typed per-module
 * and loses its element type through `Object.entries`, and the two tools
 * disagree about what is left: tsc sees `unknown`, the lint project service
 * sees a component and calls a cast redundant. A runtime check is true under
 * both readings, and it also means a module exporting something that is not a
 * story is skipped rather than rendered.
 */
function collectStories(moduleName: string, composed: Record<string, unknown>) {
  for (const [name, Story] of Object.entries(composed)) {
    if (typeof Story !== "function") continue;
    cases.push({
      module: moduleName,
      name,
      Story: Story as ComponentType,
    });
  }
}

for (const [path, module] of Object.entries(storyModules)) {
  collectStories(
    path.replace("./", "").replace(".stories.tsx", ""),
    composeStories(module as Parameters<typeof composeStories>[0]),
  );
}

describe("every story", () => {
  it("found stories to check", () => {
    /* Without this, a glob that silently matched nothing would leave the suite
       green while auditing zero components — the exact shape of hollow gate the
       injection tests elsewhere exist to prevent. */
    expect(cases.length).toBeGreaterThan(20);
  });

  it.each(cases)(
    "$module — $name has no accessibility violation",
    async ({ Story }) => {
      render(
        <TooltipProvider>
          <Story />
        </TooltipProvider>,
      );

      /* `document.body`, not the render container. Tooltips, menus, selects and
       dialogs are portalled outside it by design, and scanning the container
       would audit every component except the ones that float. */
      const results = await axe.run(document.body, {
        rules: RULES_NOT_APPLICABLE,
      });

      expect(
        results.violations.map((violation) => ({
          id: violation.id,
          help: violation.help,
          nodes: violation.nodes.map((node) => node.html),
        })),
      ).toEqual([]);
    },
  );
});

describe("the audit itself", () => {
  /* Every gate in this repository carries a case that proves it fires —
     CLAUDE.md section 14. An accessibility suite is the one most worth proving,
     because "no violations" is exactly what a suite that has been configured
     into silence also reports.

     The defect chosen is the one this component set is most exposed to: a
     button whose whole content is a glyph and which therefore has no accessible
     name. It is the reason `IconButton` makes `label` a required prop, and it
     is invisible on screen — everything looks perfectly fine. */
  it("catches an icon button with no accessible name", async () => {
    render(
      <button type="button">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <path d="M4 2.5l9 5.5-9 5.5z" fill="currentColor" />
        </svg>
      </button>,
    );

    const results = await axe.run(document.body, {
      rules: RULES_NOT_APPLICABLE,
    });

    expect(results.violations.map((violation) => violation.id)).toContain(
      "button-name",
    );
  });
});
