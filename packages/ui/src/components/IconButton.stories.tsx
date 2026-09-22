import type { Meta, StoryObj } from "@storybook/react-vite";
import { IconButton } from "./IconButton.js";
import { StoryRow } from "./StoryRow.js";

/**
 * The glyphs here are story scaffolding, not an icon set. Blinkify's icons
 * arrive with #20; these are two paths that let the button be reviewed at the
 * size it will ship at.
 */
function ScissorsGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" width="16" height="16">
      <path
        d="M4 2l8 9M12 2L4 11M4.5 13.5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0zM14.5 13.5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0z"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

function PlayGlyph() {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true" width="16" height="16">
      <path d="M4 2.5l9 5.5-9 5.5z" fill="currentColor" />
    </svg>
  );
}

const meta = {
  title: "Primitives/IconButton",
  component: IconButton,
  args: { label: "Split at playhead", icon: <ScissorsGlyph /> },
} satisfies Meta<typeof IconButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Default: Story = {};

/**
 * With the shortcut shown in the tooltip. The user opening a tooltip wanted to
 * know what the control does; the shortcut is what they learn on the way.
 */
export const WithShortcut: Story = {
  args: { shortcut: "Ctrl+B" },
};

export const Sizes: Story = {
  render: () => (
    <StoryRow>
      <IconButton size="sm" label="Play" icon={<PlayGlyph />} />
      <IconButton size="md" label="Play" icon={<PlayGlyph />} />
      <IconButton size="lg" label="Play" icon={<PlayGlyph />} />
    </StoryRow>
  ),
};

export const Disabled: Story = {
  args: { disabled: true },
};

/**
 * A toolbar row, which is the shape this component actually ships in — and the
 * case #17's acceptance criterion is about. Sweeping the pointer across it
 * opens nothing, because nothing is rested on for long enough.
 */
export const Toolbar: Story = {
  render: () => (
    <StoryRow>
      <IconButton label="Split" shortcut="S" icon={<ScissorsGlyph />} />
      <IconButton label="Delete" shortcut="Del" icon={<ScissorsGlyph />} />
      <IconButton
        label="Duplicate"
        shortcut="Ctrl+D"
        icon={<ScissorsGlyph />}
      />
      <IconButton label="Freeze frame" icon={<ScissorsGlyph />} />
      <IconButton label="Reverse" icon={<ScissorsGlyph />} />
    </StoryRow>
  ),
};
