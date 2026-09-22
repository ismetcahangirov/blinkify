import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { StoryRow } from "./StoryRow.js";
import { Switch } from "./Switch.js";

const meta = {
  title: "Primitives/Switch",
  component: Switch,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    label: "Snapping",
    checked: false,
    onCheckedChange: () => undefined,
  },
} satisfies Meta<typeof Switch>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  initial,
  ...props
}: { readonly initial: boolean } & Omit<
  React.ComponentProps<typeof Switch>,
  "checked" | "onCheckedChange"
>) {
  const [checked, setChecked] = useState(initial);
  return <Switch checked={checked} onCheckedChange={setChecked} {...props} />;
}

export const Off: Story = {
  render: () => <Controlled initial={false} label="Snapping" />,
};

export const On: Story = {
  render: () => <Controlled initial label="Snapping" />,
};

export const Disabled: Story = {
  render: () => <Controlled initial label="Linked audio" disabled />,
};

/**
 * Label hidden but still announced, for a dense toolbar where the switch sits
 * beside an icon that already says what it is.
 */
export const LabelHidden: Story = {
  render: () => <Controlled initial label="Magnetic timeline" hideLabel />,
};

/**
 * The timeline toolbar's toggles, which is what this component is for: settings
 * whose effect the user sees the instant they change them. Anything that only
 * applies when a dialog is confirmed is a checkbox, not a switch.
 */
export const TimelineToggles: Story = {
  render: () => (
    <StoryRow direction="column" align="start">
      <Controlled initial label="Snapping" />
      <Controlled initial label="Linked audio" />
      <Controlled initial={false} label="Ripple edit" />
    </StoryRow>
  ),
};
