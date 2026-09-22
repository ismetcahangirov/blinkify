import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Tabs } from "./Tabs.js";

const meta = {
  title: "Primitives/Tabs",
  component: Tabs,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    label: "Library",
    value: "media",
    onValueChange: () => undefined,
    tabs: [{ value: "media", label: "Media", content: "The asset grid." }],
  },
} satisfies Meta<typeof Tabs>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The library panel's tab row, as
 * `docs/design/capcut-layout-reference.md` specifies it: Media and Audio ship,
 * and the row is built to hold more without being rewritten.
 */
export const LibraryTabs: Story = {
  render: function LibraryTabsStory() {
    const [value, setValue] = useState("media");
    return (
      <Tabs
        label="Library"
        value={value}
        onValueChange={setValue}
        tabs={[
          { value: "media", label: "Media", content: "The asset grid." },
          { value: "audio", label: "Audio", content: "Audio assets." },
        ]}
      />
    );
  },
};

/**
 * With a disabled tab. Radix keeps it in the roving focus order and refuses to
 * activate it, which is the correct behaviour: a tab that cannot be reached is
 * a tab whose existence the user cannot discover.
 */
export const WithDisabled: Story = {
  render: function WithDisabledStory() {
    const [value, setValue] = useState("media");
    return (
      <Tabs
        label="Library"
        value={value}
        onValueChange={setValue}
        tabs={[
          { value: "media", label: "Media", content: "The asset grid." },
          { value: "audio", label: "Audio", content: "Audio assets." },
          {
            value: "effects",
            label: "Effects",
            content: "Not in Blinkify.",
            disabled: true,
          },
        ]}
      />
    );
  },
};
