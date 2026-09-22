import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Slider } from "./Slider.js";
import { StoryRow } from "./StoryRow.js";

/**
 * The four jobs #17 names, each with its own range, step and units — and one
 * component underneath all of them. Seeing them together is the review: if the
 * component had absorbed any one caller's units, one of these would look wrong.
 */
const meta = {
  title: "Primitives/Slider",
  component: Slider,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    label: "Clip volume",
    value: 0,
    onValueChange: () => undefined,
    min: -60,
    max: 12,
  },
} satisfies Meta<typeof Slider>;

export default meta;
type Story = StoryObj<typeof meta>;

function Controlled({
  initial,
  ...props
}: { readonly initial: number } & Omit<
  React.ComponentProps<typeof Slider>,
  "value" | "onValueChange"
>) {
  const [value, setValue] = useState(initial);
  return <Slider value={value} onValueChange={setValue} {...props} />;
}

export const Volume: Story = {
  render: () => (
    <Controlled
      initial={0}
      label="Clip volume"
      min={-60}
      max={12}
      step={0.5}
      formatValue={(value) => `${value.toFixed(1)} dB`}
    />
  ),
};

export const DenoiseStrength: Story = {
  render: () => (
    <Controlled
      initial={40}
      label="Noise reduction"
      min={0}
      max={100}
      step={1}
      formatValue={(value) => `${String(value)}%`}
    />
  ),
};

export const Speed: Story = {
  render: () => (
    <Controlled
      initial={1}
      label="Clip speed"
      min={0.25}
      max={4}
      step={0.05}
      formatValue={(value) => `${value.toFixed(2)}x`}
    />
  ),
};

/**
 * No readout. Correct for timeline zoom: the number is meaningless to the user
 * and the timeline itself is the feedback.
 */
export const NoReadout: Story = {
  render: () => (
    <Controlled initial={50} label="Timeline zoom" min={0} max={100} />
  ),
};

export const Disabled: Story = {
  render: () => (
    <Controlled
      initial={-6}
      label="Clip volume"
      min={-60}
      max={12}
      step={0.5}
      disabled
      formatValue={(value) => `${value.toFixed(1)} dB`}
    />
  ),
};

/**
 * All four at once. The readout column is a reserved width, so the numbers
 * stay in one line down the panel however far apart their values are — which
 * is the whole reason it is reserved.
 */
export const AsAPanel: Story = {
  render: () => (
    <StoryRow direction="column" align="stretch">
      <Controlled
        initial={-6}
        label="Volume"
        min={-60}
        max={12}
        step={0.5}
        formatValue={(value) => `${value.toFixed(1)} dB`}
      />
      <Controlled
        initial={100}
        label="Noise reduction"
        min={0}
        max={100}
        formatValue={(value) => `${String(value)}%`}
      />
      <Controlled
        initial={2}
        label="Speed"
        min={0.25}
        max={4}
        step={0.05}
        formatValue={(value) => `${value.toFixed(2)}x`}
      />
    </StoryRow>
  ),
};
