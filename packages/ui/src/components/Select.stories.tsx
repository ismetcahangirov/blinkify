import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Select } from "./Select.js";
import { StoryRow } from "./StoryRow.js";

const meta = {
  title: "Primitives/Select",
  component: Select,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    label: "Preview quality",
    value: "full",
    onValueChange: () => undefined,
    options: [{ value: "full", label: "Full" }],
  },
} satisfies Meta<typeof Select>;

export default meta;
type Story = StoryObj<typeof meta>;

/** The player's preview-quality control — a preview decision that can never reach the output file. */
export const PreviewQuality: Story = {
  render: function PreviewQualityStory() {
    const [value, setValue] = useState("full");
    return (
      <Select
        label="Preview quality"
        value={value}
        onValueChange={setValue}
        options={[
          { value: "full", label: "Full" },
          { value: "half", label: "Half" },
          { value: "quarter", label: "Quarter" },
        ]}
      />
    );
  },
};

export const WithDisabledOption: Story = {
  render: function WithDisabledOptionStory() {
    const [value, setValue] = useState("16-9");
    return (
      <Select
        label="Aspect ratio"
        value={value}
        onValueChange={setValue}
        options={[
          { value: "16-9", label: "16:9" },
          { value: "9-16", label: "9:16" },
          { value: "1-1", label: "1:1" },
          { value: "custom", label: "Custom", disabled: true },
        ]}
      />
    );
  },
};

export const Disabled: Story = {
  render: () => (
    <Select
      label="Preview quality"
      value="full"
      onValueChange={() => undefined}
      disabled
      options={[{ value: "full", label: "Full" }]}
    />
  ),
};

/** Two beside each other, as the player's transport row has them. */
export const InATransportRow: Story = {
  render: function InATransportRowStory() {
    const [quality, setQuality] = useState("full");
    const [ratio, setRatio] = useState("16-9");
    return (
      <StoryRow>
        <Select
          label="Preview quality"
          value={quality}
          onValueChange={setQuality}
          options={[
            { value: "full", label: "Full" },
            { value: "half", label: "Half" },
            { value: "quarter", label: "Quarter" },
          ]}
        />
        <Select
          label="Aspect ratio"
          value={ratio}
          onValueChange={setRatio}
          options={[
            { value: "16-9", label: "16:9" },
            { value: "9-16", label: "9:16" },
          ]}
        />
      </StoryRow>
    );
  },
};
