import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Button } from "./Button.js";
import { Dialog, DialogClose } from "./Dialog.js";

const meta = {
  title: "Primitives/Dialog",
  component: Dialog,
  /* The component's required props, defaulted once here. Without them every
     story would repeat them purely to typecheck, and the stories would stop
     reading as the thing being demonstrated. They also populate Storybook's
     controls, which is what makes a story explorable rather than only
     viewable. */
  args: {
    open: false,
    onOpenChange: () => undefined,
    title: "Overwrite holiday-cut.mp4?",
  },
} satisfies Meta<typeof Dialog>;

export default meta;
type Story = StoryObj<typeof meta>;

/**
 * The overwrite confirmation, which is one of the two dialogs in Blinkify that
 * earns being modal — `CLAUDE.md` section 19: no export target is overwritten
 * without it.
 */
export const Confirmation: Story = {
  render: function ConfirmationStory() {
    const [open, setOpen] = useState(false);
    return (
      <>
        <Button
          onClick={() => {
            setOpen(true);
          }}
        >
          Export
        </Button>
        <Dialog
          open={open}
          onOpenChange={setOpen}
          title="Overwrite holiday-cut.mp4?"
          description="A file with this name already exists. Exporting will replace it."
          actions={
            <>
              <DialogClose>
                <Button>Cancel</Button>
              </DialogClose>
              <DialogClose>
                <Button variant="danger">Overwrite</Button>
              </DialogClose>
            </>
          }
        />
      </>
    );
  },
};

/**
 * With a body between the description and the actions. The description is read
 * out immediately after the title; the body is not, which is why the sentence
 * that matters goes in the description and the detail goes here.
 */
export const WithBody: Story = {
  render: function WithBodyStory() {
    const [open, setOpen] = useState(false);
    return (
      <>
        <Button
          variant="primary"
          onClick={() => {
            setOpen(true);
          }}
        >
          Export
        </Button>
        <Dialog
          open={open}
          onOpenChange={setOpen}
          title="Export"
          description="Three segments will be re-encoded. Everything else is copied."
          actions={
            <>
              <DialogClose>
                <Button>Cancel</Button>
              </DialogClose>
              <Button variant="primary">Start export</Button>
            </>
          }
        >
          <p style={{ color: "var(--text-secondary)" }}>
            Segments 2, 5 and 9 are cropped, which changes the pixels.
          </p>
        </Dialog>
      </>
    );
  },
};

/** Open on load, so the surface itself can be reviewed without a click. */
export const Open: Story = {
  render: function OpenStory() {
    const [open, setOpen] = useState(true);
    return (
      <Dialog
        open={open}
        onOpenChange={setOpen}
        title="Overwrite holiday-cut.mp4?"
        description="A file with this name already exists. Exporting will replace it."
        actions={
          <DialogClose>
            <Button>Cancel</Button>
          </DialogClose>
        }
      />
    );
  },
};
