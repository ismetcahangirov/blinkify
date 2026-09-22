import type { Decorator, Preview } from "@storybook/react-vite";
import { TooltipProvider } from "../src/components/Tooltip.js";
import "../src/index.css";

/**
 * Every story renders against the real design system: the bundled typefaces,
 * the token files and the component stylesheet, imported exactly as the
 * application imports them. A Storybook with its own approximation of the
 * theme is a Storybook that agrees with nothing.
 */

/**
 * One tooltip provider around every story.
 *
 * Not decoration — it is the group delay clock. A tooltip rendered without a
 * provider still works and uses its own timer, which means the behaviour being
 * reviewed in Storybook would not be the behaviour that ships.
 */
const withTooltipProvider: Decorator = (Story) => (
  <TooltipProvider>
    <Story />
  </TooltipProvider>
);

const preview: Preview = {
  decorators: [withTooltipProvider],
  parameters: {
    /* The application background, so a component is judged against the surface
       it will actually sit on. A design system reviewed on white is one whose
       borders all turn out to be invisible. */
    backgrounds: {
      options: {
        application: { name: "Application", value: "var(--background)" },
        panel: { name: "Panel", value: "var(--surface)" },
      },
    },
    a11y: {
      /* Fail the addon's panel rather than only colouring it. The same rules
         run in `stories.test.tsx`, which is what actually blocks a merge. */
      test: "error",
    },
  },
  initialGlobals: {
    backgrounds: { value: "application" },
  },
};

export default preview;
