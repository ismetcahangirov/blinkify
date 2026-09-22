import type { Meta, StoryObj } from "@storybook/react-vite";
import { Button } from "./Button.js";
import { ContextMenu, DropdownMenu, type MenuGroupDefinition } from "./Menu.js";

const meta = {
  title: "Primitives/Menu",
  component: DropdownMenu,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: { trigger: <Button>Edit</Button>, groups: [] },
} satisfies Meta<typeof DropdownMenu>;

export default meta;
type Story = StoryObj<typeof meta>;

const noop = (): void => undefined;

/**
 * The timeline's clip commands, which both menus offer — the Edit menu from the
 * application bar and the context menu from the clip itself. One definition,
 * two renderers: that is the reason the item shape is data rather than markup.
 */
const clipCommands: readonly MenuGroupDefinition[] = [
  {
    items: [
      { id: "split", label: "Split", shortcut: "S", onSelect: noop },
      {
        id: "duplicate",
        label: "Duplicate",
        shortcut: "Ctrl+D",
        onSelect: noop,
      },
    ],
  },
  {
    label: "Transform",
    items: [
      { id: "freeze", label: "Freeze frame", onSelect: noop },
      { id: "reverse", label: "Reverse", onSelect: noop },
      { id: "detach", label: "Detach audio", disabled: true, onSelect: noop },
    ],
  },
  {
    items: [
      {
        id: "delete",
        label: "Delete",
        shortcut: "Del",
        danger: true,
        onSelect: noop,
      },
    ],
  },
];

export const Dropdown: Story = {
  render: () => (
    <DropdownMenu trigger={<Button>Edit</Button>} groups={clipCommands} />
  ),
};

/**
 * Right-click the region. Same items, same styling, different summons — which
 * is the entire difference between the two components.
 */
export const Context: Story = {
  render: () => (
    <ContextMenu groups={clipCommands}>
      <div
        style={{
          padding: "var(--space-6)",
          background: "var(--timeline-clip-video)",
          border: "var(--border-width) solid var(--timeline-clip-video-border)",
          borderRadius: "var(--radius-sm)",
          color: "var(--timeline-clip-label)",
        }}
      >
        Right-click this clip
      </div>
    </ContextMenu>
  ),
};

/**
 * A single group, no separators, no labels — the shape most menus in the
 * product actually have.
 */
export const Simple: Story = {
  render: () => (
    <DropdownMenu
      trigger={<Button>File</Button>}
      groups={[
        {
          items: [
            {
              id: "new",
              label: "New project",
              shortcut: "Ctrl+N",
              onSelect: noop,
            },
            { id: "open", label: "Open…", shortcut: "Ctrl+O", onSelect: noop },
            { id: "save", label: "Save", shortcut: "Ctrl+S", onSelect: noop },
          ],
        },
      ]}
    />
  ),
};
