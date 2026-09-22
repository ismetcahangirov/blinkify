import type { Meta, StoryObj } from "@storybook/react-vite";
import { ScrollArea } from "./ScrollArea.js";

const meta = {
  title: "Primitives/ScrollArea",
  component: ScrollArea,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: { children: null },
} satisfies Meta<typeof ScrollArea>;

export default meta;
type Story = StoryObj<typeof meta>;

const rows = Array.from({ length: 30 }, (_, index) => index + 1);

/**
 * The thin scrollbar exists because Windows draws a 17px light one, and in a
 * 240px library panel that is seven per cent of the width spent on a control
 * nobody looks at.
 *
 * What it does not give up: the region is still natively scrollable, so the
 * wheel, Page Up, Home, End and `scrollIntoView` all work because none of them
 * was ever intercepted.
 */
export const Vertical: Story = {
  render: () => (
    /* The decorator supplies the height. A scroll area with no constraint on it
       has nothing to scroll, so the constraint is the story rather than the
       component. */
    <ScrollArea>
      {rows.map((row) => (
        <p key={row} style={{ margin: "var(--space-2)" }}>
          Asset {row}
        </p>
      ))}
    </ScrollArea>
  ),
  decorators: [
    (Story) => (
      <div style={{ height: "var(--space-16)", maxHeight: "var(--space-16)" }}>
        <Story />
      </div>
    ),
  ],
};

export const Both: Story = {
  render: () => (
    <ScrollArea orientation="both">
      <div style={{ width: "var(--popover-max-width)" }}>
        {rows.map((row) => (
          <p
            key={row}
            style={{ margin: "var(--space-2)", whiteSpace: "nowrap" }}
          >
            Asset {row} — a filename long enough to need scrolling sideways
          </p>
        ))}
      </div>
    </ScrollArea>
  ),
  decorators: [
    (Story) => (
      <div
        style={{ height: "var(--space-16)", width: "var(--menu-min-width)" }}
      >
        <Story />
      </div>
    ),
  ],
};
