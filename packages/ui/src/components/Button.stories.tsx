import type { Meta, StoryObj } from "@storybook/react-vite";
import { Button } from "./Button.js";
import { StoryRow } from "./StoryRow.js";

/**
 * Every variant, every size, every state.
 *
 * The states are the point. A variant is looked at constantly during
 * development; `disabled` and `loading` are looked at once, by whoever wrote
 * them, and then never again until a user meets one. Storybook is where they
 * stay visible.
 */
const meta = {
  title: "Primitives/Button",
  component: Button,
  args: { children: "Export" },
} satisfies Meta<typeof Button>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Primary: Story = {
  args: { variant: "primary" },
};

export const Secondary: Story = {
  args: { variant: "secondary" },
};

export const Ghost: Story = {
  args: { variant: "ghost" },
};

export const Danger: Story = {
  args: { variant: "danger", children: "Delete clip" },
};

export const Sizes: Story = {
  render: () => (
    <StoryRow>
      <Button size="sm">Small</Button>
      <Button size="md">Medium</Button>
      <Button size="lg">Large</Button>
    </StoryRow>
  ),
};

export const Disabled: Story = {
  args: { disabled: true },
};

/**
 * The button keeps its width while it works. A control that narrows when
 * clicked moves everything beside it, and in a toolbar that means the next
 * thing the user was about to click has moved.
 */
export const Loading: Story = {
  args: { variant: "primary", loading: true },
};

/**
 * All four variants side by side, which is the only way to see whether they
 * read as one family. `danger` sits quiet until hovered on purpose — a red
 * block in a toolbar draws the eye to the control the user reaches for least.
 */
export const AllVariants: Story = {
  render: () => (
    <StoryRow>
      <Button variant="primary">Primary</Button>
      <Button variant="secondary">Secondary</Button>
      <Button variant="ghost">Ghost</Button>
      <Button variant="danger">Danger</Button>
      <Button disabled>Disabled</Button>
    </StoryRow>
  ),
};
