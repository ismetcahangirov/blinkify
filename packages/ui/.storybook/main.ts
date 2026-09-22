import type { StorybookConfig } from "@storybook/react-vite";

/**
 * Storybook for the Blinkify design system (#17).
 *
 * It exists so a primitive can be reviewed in isolation — every variant, every
 * state, without hunting for the one screen that happens to use it. That is an
 * Epic #2 acceptance criterion, and it is also how a component's disabled and
 * loading states stop being the ones nobody ever looks at.
 *
 * It does not build in the pull-request gate. The accessibility check that
 * matters runs the same stories through Vitest (`stories.test.tsx`), which the
 * gate already runs; adding a Storybook build to it would spend a minute of the
 * fifteen-minute budget proving something the story files' own typecheck
 * already proves.
 */
const config: StorybookConfig = {
  stories: ["../src/**/*.stories.tsx"],
  addons: ["@storybook/addon-a11y"],
  framework: {
    name: "@storybook/react-vite",
    options: {},
  },
  core: {
    /*
     * Storybook reports anonymous usage telemetry by default. `CLAUDE.md`
     * section 20 rule 8 is unconditional: no network call except the update
     * check. That rule is written about the shipped application, and it is
     * honoured here too — a developer tool that phones home from a contributor's
     * machine is still a network call nobody asked for, and a repository that
     * makes an exception for its own tooling is a repository where the rule has
     * become a preference.
     */
    disableTelemetry: true,
  },
};

export default config;
