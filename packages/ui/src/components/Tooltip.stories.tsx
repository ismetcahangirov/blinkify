import type { Meta, StoryObj } from "@storybook/react-vite";
import { Button } from "./Button.js";
import { StoryRow } from "./StoryRow.js";
import { Tooltip } from "./Tooltip.js";

/**
 * The timing is the thing to review here, and it cannot be seen in a
 * screenshot.
 *
 * Sweep the pointer across the row below without stopping: nothing opens,
 * because nothing is rested on for `TOOLTIP_DELAY_MS`. Now stop on one, read
 * it, and move to its neighbour: that one opens at once, because the
 * `TOOLTIP_SKIP_DELAY_MS` window is open.
 *
 * Those two behaviours are what #17 asks for in two separate sentences that
 * look contradictory. They are not — see `Tooltip.tsx`.
 */
const meta = {
  title: "Primitives/Tooltip",
  component: Tooltip,
  /* Every prop of `Tooltip` is required, so the meta carries a default set.
     Without it each story would have to repeat them just to typecheck, and the
     stories would stop reading as the thing being demonstrated. */
  args: {
    label: "Split the clip at the playhead",
    children: <Button>Split</Button>,
  },
} satisfies Meta<typeof Tooltip>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {
  render: () => (
    <Tooltip label="Split the clip at the playhead">
      <Button>Split</Button>
    </Tooltip>
  ),
};

export const WithShortcut: Story = {
  render: () => (
    <Tooltip label="Split the clip at the playhead" shortcut="S">
      <Button>Split</Button>
    </Tooltip>
  ),
};

export const Sides: Story = {
  render: () => (
    <StoryRow>
      <Tooltip label="Above" side="top">
        <Button>Top</Button>
      </Tooltip>
      <Tooltip label="Right" side="right">
        <Button>Right</Button>
      </Tooltip>
      <Tooltip label="Below" side="bottom">
        <Button>Bottom</Button>
      </Tooltip>
      <Tooltip label="Left" side="left">
        <Button>Left</Button>
      </Tooltip>
    </StoryRow>
  ),
};

/** The group-delay case. Sweep across it; then stop on one and move sideways. */
export const GroupDelay: Story = {
  render: () => (
    <StoryRow>
      {["Split", "Delete", "Duplicate", "Freeze", "Reverse"].map((label) => (
        <Tooltip key={label} label={label}>
          <Button variant="ghost">{label}</Button>
        </Tooltip>
      ))}
    </StoryRow>
  ),
};
