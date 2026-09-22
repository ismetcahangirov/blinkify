import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Button } from "./Button.js";
import { Popover } from "./Popover.js";
import { Slider } from "./Slider.js";
import { StoryRow } from "./StoryRow.js";

const meta = {
  title: "Primitives/Popover",
  component: Popover,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    trigger: <Button>Zoom</Button>,
    children: <p>Timeline zoom presets.</p>,
  },
} satisfies Meta<typeof Popover>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <Popover trigger={<Button>Zoom</Button>}>
      <p>Timeline zoom presets.</p>
    </Popover>
  ),
};

/**
 * A popover holding a control, which is what separates it from a tooltip. The
 * interface behind it stays live, and it closes when the user looks away.
 */
export const WithControls: Story = {
  render: function WithControlsStory() {
    const [value, setValue] = useState(1);
    return (
      <Popover trigger={<Button>Speed</Button>}>
        <StoryRow direction="column" align="stretch">
          <Slider
            label="Clip speed"
            value={value}
            onValueChange={setValue}
            min={0.25}
            max={4}
            step={0.05}
            formatValue={(next) => `${next.toFixed(2)}x`}
          />
        </StoryRow>
      </Popover>
    );
  },
};

export const SideTop: Story = {
  render: () => (
    <Popover side="top" trigger={<Button>Opens upward</Button>}>
      <p>Anchored above its trigger.</p>
    </Popover>
  ),
};
