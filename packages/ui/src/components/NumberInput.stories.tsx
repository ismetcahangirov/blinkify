import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { NumberInput } from "./NumberInput.js";
import { StoryRow } from "./StoryRow.js";

const meta = {
  title: "Primitives/NumberInput",
  component: NumberInput,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: { label: "Gain", value: 0, onValueChange: () => undefined },
} satisfies Meta<typeof NumberInput>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  initial,
  ...props
}: { readonly initial: number } & Omit<
  React.ComponentProps<typeof NumberInput>,
  "value" | "onValueChange"
>) {
  const [value, setValue] = useState(initial);
  return <NumberInput value={value} onValueChange={setValue} {...props} />;
}

/**
 * Drag the label sideways. It is the interaction a user arriving from another
 * editor will try within a minute, and a field that ignores it reads as broken
 * rather than as having made a different choice.
 */
export const Default: Story = {
  render: () => (
    <Controlled initial={0} label="Gain" unit="dB" precision={1} step={0.5} />
  ),
};

export const WithBounds: Story = {
  render: () => (
    <Controlled
      initial={100}
      label="Opacity"
      unit="%"
      min={0}
      max={100}
      step={1}
    />
  ),
};

export const Disabled: Story = {
  render: () => <Controlled initial={25} label="Speed" unit="x" disabled />,
};

/**
 * An inspector row, which is where this actually ships. The fields are a
 * reserved width and the figures are tabular, so the numbers stay in a column
 * as they change — the user is watching them precisely because they are
 * changing them.
 */
export const AsAnInspectorRow: Story = {
  render: () => (
    <StoryRow direction="column" align="start">
      <Controlled initial={0} label="Gain" unit="dB" precision={1} step={0.5} />
      <Controlled
        initial={1}
        label="Speed"
        unit="x"
        precision={2}
        step={0.05}
      />
      <Controlled initial={1920} label="Width" unit="px" step={2} />
    </StoryRow>
  ),
};
