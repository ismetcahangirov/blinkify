import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import {
  contrastRatio,
  parseTokens,
  resolveToken,
  WCAG_AA,
} from "../tokens/contrast.js";
import { Button } from "./Button.js";
import { Dialog, DialogClose } from "./Dialog.js";
import { IconButton } from "./IconButton.js";
import { DropdownMenu } from "./Menu.js";
import { NumberInput } from "./NumberInput.js";
import { Popover } from "./Popover.js";
import { Select } from "./Select.js";
import { Slider } from "./Slider.js";
import { Switch } from "./Switch.js";
import { Tabs } from "./Tabs.js";
import { Tooltip, TooltipProvider } from "./Tooltip.js";

/**
 * Keyboard operation, component by component (#17).
 *
 * ── Why these tests exist when Radix already works ─────────────────────────
 *
 * Radix supplies the keyboard behaviour; what these check is that we did not
 * take it away. Every one of the ways to lose it is a small, reasonable-looking
 * edit: an `onClick` that should have been `onSelect`, a `div` that should have
 * been a `Trigger`, a `tabIndex={-1}` added to stop a focus ring appearing
 * somewhere it was not wanted. None of them changes how anything looks, and all
 * of them are caught here.
 *
 * So these are tests of the wiring, not of the library. They assert what the
 * user can do, never which component rendered it — `CLAUDE.md` section 13.
 *
 * Note the deliberate absence of a `Tab`-order test. jsdom computes no layout
 * and no visibility, so its idea of the focus order is not the browser's.
 * Asserting it would produce a test that passes here and proves nothing there.
 */

const HERE = dirname(fileURLToPath(import.meta.url));
const tokens = parseTokens(
  readFileSync(join(HERE, "..", "tokens", "tokens.css"), "utf8"),
);

/** Every story and every test runs under one provider, as the application does. */
function renderWithProvider(ui: React.ReactElement) {
  return render(<TooltipProvider>{ui}</TooltipProvider>);
}

describe("Button", () => {
  it("activates on Enter and on Space", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    renderWithProvider(<Button onClick={onClick}>Export</Button>);

    const button = screen.getByRole("button", { name: "Export" });
    button.focus();
    await user.keyboard("{Enter}");
    await user.keyboard(" ");

    expect(onClick).toHaveBeenCalledTimes(2);
  });

  it("does not activate while loading", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    renderWithProvider(
      <Button loading onClick={onClick}>
        Export
      </Button>,
    );

    await user.click(screen.getByRole("button", { name: "Export" }));

    expect(onClick).not.toHaveBeenCalled();
  });

  it("says it is busy rather than only refusing to answer", () => {
    /* `disabled` alone tells a screen reader the control is unavailable, which
       is a different claim from "it is working on what you asked". */
    renderWithProvider(<Button loading>Export</Button>);
    expect(screen.getByRole("button", { name: "Export" })).toHaveAttribute(
      "aria-busy",
      "true",
    );
  });

  it("keeps its label in the layout while loading", () => {
    /* The label is hidden, not removed. A button that narrows when clicked
       moves every control beside it, and in a toolbar the next thing the user
       was about to click has then moved. */
    renderWithProvider(<Button loading>Export</Button>);
    expect(screen.getByText("Export")).toBeInTheDocument();
  });

  it("is a button and not a submit", async () => {
    /* A primitive that submits a form nobody knew it was in is a defect that
       only appears once there is a form — long after the component was
       reviewed. */
    const onSubmit = vi.fn((event: React.FormEvent) => {
      event.preventDefault();
    });
    const user = userEvent.setup();
    renderWithProvider(
      <form onSubmit={onSubmit}>
        <Button>Not a submit</Button>
      </form>,
    );

    await user.click(screen.getByRole("button", { name: "Not a submit" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

describe("IconButton", () => {
  it("is announced by its label rather than as 'button'", () => {
    /* The defect this component exists to make impossible. A toolbar of icon
       buttons with no names is announced as fourteen controls called "button",
       and nothing on screen looks wrong. */
    renderWithProvider(
      <IconButton
        label="Split at playhead"
        icon={<svg aria-hidden="true" />}
      />,
    );
    expect(
      screen.getByRole("button", { name: "Split at playhead" }),
    ).toBeInTheDocument();
  });

  it("shows its tooltip on keyboard focus, not only on hover", async () => {
    /* A tooltip that only answers a pointer is a tooltip a keyboard user
       cannot read, on a control whose only other label is a glyph. */
    const user = userEvent.setup();
    renderWithProvider(
      <IconButton
        label="Split at playhead"
        icon={<svg aria-hidden="true" />}
      />,
    );

    await user.tab();

    expect(await screen.findByRole("tooltip")).toHaveTextContent(
      "Split at playhead",
    );
  });
});

describe("Slider", () => {
  it("steps with the arrow keys", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <Slider
        label="Clip volume"
        value={0}
        onValueChange={onValueChange}
        min={-60}
        max={12}
        step={1}
      />,
    );

    screen.getByRole("slider", { name: "Clip volume" }).focus();
    await user.keyboard("{ArrowRight}");

    expect(onValueChange).toHaveBeenCalledWith(1);
  });

  it("goes to its bounds with Home and End", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <Slider
        label="Clip volume"
        value={0}
        onValueChange={onValueChange}
        min={-60}
        max={12}
      />,
    );

    screen.getByRole("slider", { name: "Clip volume" }).focus();
    await user.keyboard("{Home}");
    expect(onValueChange).toHaveBeenLastCalledWith(-60);

    await user.keyboard("{End}");
    expect(onValueChange).toHaveBeenLastCalledWith(12);
  });

  it("announces the value with its unit", () => {
    /* "-6" and "-6.0 dB" are different facts. The unit is the half that says
       which of them the user is looking at. */
    renderWithProvider(
      <Slider
        label="Clip volume"
        value={-6}
        onValueChange={vi.fn()}
        min={-60}
        max={12}
        formatValue={(value) => `${value.toFixed(1)} dB`}
      />,
    );

    expect(screen.getByRole("slider", { name: "Clip volume" })).toHaveAttribute(
      "aria-valuetext",
      "-6.0 dB",
    );
  });
});

describe("NumberInput", () => {
  it("steps with the arrow keys and multiplies the step with Shift", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <NumberInput
        label="Gain"
        value={0}
        onValueChange={onValueChange}
        step={1}
      />,
    );

    const field = screen.getByRole("spinbutton", { name: "Gain" });
    field.focus();

    await user.keyboard("{ArrowUp}");
    expect(onValueChange).toHaveBeenLastCalledWith(1);

    await user.keyboard("{Shift>}{ArrowUp}{/Shift}");
    expect(onValueChange).toHaveBeenLastCalledWith(10);
  });

  it("reads as mixed rather than as any one value", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <NumberInput
        label="Speed"
        value={1}
        onValueChange={onValueChange}
        step={0.01}
        precision={2}
        mixed
      />,
    );
    const field = screen.getByRole("spinbutton", { name: "Speed" });
    expect(field).toHaveValue("");
    expect(field).toHaveAttribute("placeholder", "Mixed");
    expect(field).toHaveAttribute("aria-valuetext", "Mixed");

    // Typing still commits one value.
    await user.type(field, "2");
    expect(onValueChange).toHaveBeenLastCalledWith(2);
  });

  it("clamps to its bounds", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <NumberInput
        label="Opacity"
        value={100}
        onValueChange={onValueChange}
        min={0}
        max={100}
      />,
    );

    screen.getByRole("spinbutton", { name: "Opacity" }).focus();
    await user.keyboard("{ArrowUp}");

    expect(onValueChange).toHaveBeenLastCalledWith(100);
  });

  it("scrubs when its label is dragged", () => {
    /* The interaction a user arriving from another editor tries within a
       minute. A field that ignores it reads as broken rather than as having
       made a different choice. */
    const onValueChange = vi.fn();
    renderWithProvider(
      <NumberInput
        label="Gain"
        value={0}
        onValueChange={onValueChange}
        step={1}
      />,
    );

    const handle = screen.getByText("Gain");
    handle.dispatchEvent(
      new PointerEvent("pointerdown", { bubbles: true, clientX: 0 }),
    );
    handle.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, clientX: 40 }),
    );

    expect(onValueChange).toHaveBeenLastCalledWith(10);
  });

  it("lets a partial number be typed without fighting the user", async () => {
    /* Typing "-" on the way to "-6" must not become 0 and swallow the minus
       sign. The draft is local until it parses. */
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <NumberInput label="Gain" value={0} onValueChange={onValueChange} />,
    );

    const field = screen.getByRole("spinbutton", { name: "Gain" });
    await user.clear(field);
    await user.type(field, "-");

    expect(field).toHaveValue("-");
    expect(onValueChange).not.toHaveBeenCalled();
  });
});

describe("Switch", () => {
  it("toggles with Space", async () => {
    const user = userEvent.setup();
    const onCheckedChange = vi.fn();
    renderWithProvider(
      <Switch
        label="Snapping"
        checked={false}
        onCheckedChange={onCheckedChange}
      />,
    );

    screen.getByRole("switch", { name: "Snapping" }).focus();
    await user.keyboard(" ");

    expect(onCheckedChange).toHaveBeenCalledWith(true);
  });

  it("is operated by clicking its label", async () => {
    /* A 20px control beside twelve characters of inert text is a control that
       looks bigger than it is. */
    const user = userEvent.setup();
    const onCheckedChange = vi.fn();
    renderWithProvider(
      <Switch
        label="Snapping"
        checked={false}
        onCheckedChange={onCheckedChange}
      />,
    );

    await user.click(screen.getByText("Snapping"));

    expect(onCheckedChange).toHaveBeenCalledWith(true);
  });
});

describe("Tabs", () => {
  it("moves between tabs with the arrow keys, not with Tab", async () => {
    /* Roving focus. A row of eight buttons puts eight stops between the user
       and the panel below it; a tab row puts one. */
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <Tabs
        label="Library"
        value="media"
        onValueChange={onValueChange}
        tabs={[
          { value: "media", label: "Media", content: "Assets" },
          { value: "audio", label: "Audio", content: "Audio" },
        ]}
      />,
    );

    screen.getByRole("tab", { name: "Media" }).focus();
    await user.keyboard("{ArrowRight}");

    expect(onValueChange).toHaveBeenCalledWith("audio");
  });
});

describe("Select", () => {
  it("opens and chooses with the keyboard alone", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    renderWithProvider(
      <Select
        label="Preview quality"
        value="full"
        onValueChange={onValueChange}
        options={[
          { value: "full", label: "Full" },
          { value: "half", label: "Half" },
        ]}
      />,
    );

    screen.getByRole("combobox", { name: "Preview quality" }).focus();
    await user.keyboard("{Enter}");

    const option = await screen.findByRole("option", { name: "Half" });
    await user.click(option);

    expect(onValueChange).toHaveBeenCalledWith("half");
  });
});

describe("DropdownMenu", () => {
  it("opens from the keyboard and selects an item", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    renderWithProvider(
      <DropdownMenu
        trigger={<Button>Edit</Button>}
        groups={[{ items: [{ id: "split", label: "Split", onSelect }] }]}
      />,
    );

    screen.getByRole("button", { name: "Edit" }).focus();
    await user.keyboard("{Enter}");

    const item = await screen.findByRole("menuitem", { name: "Split" });
    await user.click(item);

    expect(onSelect).toHaveBeenCalledTimes(1);

    /* Wait for the menu to finish closing before the test ends. Radix's
       dismissable-layer keeps module-level bookkeeping about which layer owns
       the document, and unmounting a menu mid-close leaves that bookkeeping
       holding a layer that no longer exists — after which the next menu in the
       file opens and is dismissed again immediately. That failure appears in a
       different test from the one that caused it, which is the worst kind. */
    await waitFor(() => {
      expect(screen.queryByRole("menuitem", { name: "Split" })).toBeNull();
    });
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    renderWithProvider(
      <DropdownMenu
        trigger={<Button>Edit</Button>}
        groups={[
          { items: [{ id: "split", label: "Split", onSelect: vi.fn() }] },
        ]}
      />,
    );

    screen.getByRole("button", { name: "Edit" }).focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menuitem", { name: "Split" });

    await user.keyboard("{Escape}");

    await waitFor(() => {
      expect(screen.queryByRole("menuitem", { name: "Split" })).toBeNull();
    });
  });

  it("refuses a disabled item", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    renderWithProvider(
      <DropdownMenu
        trigger={<Button>Edit</Button>}
        groups={[
          {
            items: [
              { id: "detach", label: "Detach", disabled: true, onSelect },
            ],
          },
        ]}
      />,
    );

    screen.getByRole("button", { name: "Edit" }).focus();
    await user.keyboard("{Enter}");
    await user.click(await screen.findByRole("menuitem", { name: "Detach" }));

    expect(onSelect).not.toHaveBeenCalled();

    /* Clicking a disabled item does not close the menu, which is correct
       behaviour and leaves a modal open at the end of the test. Radix marks
       everything outside an open modal `aria-hidden`, and Testing Library
       honours that — so the next test in the file would query a document where
       its own markup is invisible to `getByRole`. Closing it here keeps each
       test independent of the one before it. */
    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByRole("menuitem", { name: "Detach" })).toBeNull();
    });
  });
});

describe("Popover", () => {
  it("opens from its trigger and closes on Escape", async () => {
    const user = userEvent.setup();
    renderWithProvider(
      <Popover trigger={<Button>Zoom</Button>}>
        <p>Zoom presets</p>
      </Popover>,
    );

    await user.click(screen.getByRole("button", { name: "Zoom" }));
    expect(await screen.findByText("Zoom presets")).toBeInTheDocument();

    await user.keyboard("{Escape}");
    await waitFor(() => {
      expect(screen.queryByText("Zoom presets")).toBeNull();
    });
  });
});

describe("Dialog", () => {
  it("is named by its title", () => {
    /* A dialog announced as "dialog" leaves a user who cannot see it with no
       idea what has taken over the window. */
    renderWithProvider(
      <Dialog open onOpenChange={vi.fn()} title="Overwrite holiday-cut.mp4?" />,
    );

    expect(
      screen.getByRole("dialog", { name: "Overwrite holiday-cut.mp4?" }),
    ).toBeInTheDocument();
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    renderWithProvider(
      <Dialog open onOpenChange={onOpenChange} title="Export" />,
    );

    await user.keyboard("{Escape}");

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("closes from a button wrapped in DialogClose", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    renderWithProvider(
      <Dialog
        open
        onOpenChange={onOpenChange}
        title="Export"
        actions={
          <DialogClose>
            <Button>Cancel</Button>
          </DialogClose>
        }
      />,
    );

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe("Tooltip", () => {
  it("does not open without the pointer resting", () => {
    /* The acceptance criterion: sweeping across a toolbar must not flicker
       tooltips. Nothing has rested, so nothing is shown. */
    renderWithProvider(
      <Tooltip label="Split">
        <Button>Split</Button>
      </Tooltip>,
    );

    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});

describe("the brand gradient cannot carry a label", () => {
  /*
   * The measurement behind `Button`'s primary variant not being a gradient,
   * asserted rather than remembered.
   *
   * #17 asks for a gradient primary. No label colour clears WCAG AA across the
   * sweep: white fails at the azure stop, near-black fails at the indigo stop.
   * If the brand ever moves far enough for one of them to pass, this test fails
   * and the decision is reopened deliberately — which is the opposite of a
   * comment, which would go on being true-looking forever.
   */
  const stops = ["--brand-azure", "--brand-indigo", "--brand-violet"] as const;

  it("fails AA with a white label somewhere on the sweep", () => {
    const white = resolveToken(tokens, "--accent-foreground");
    const worst = Math.min(
      ...stops.map((stop) => contrastRatio(white, resolveToken(tokens, stop))),
    );
    expect(worst).toBeLessThan(WCAG_AA.normalText);
  });

  it("fails AA with a dark label somewhere on the sweep", () => {
    const dark = resolveToken(tokens, "--background");
    const worst = Math.min(
      ...stops.map((stop) => contrastRatio(dark, resolveToken(tokens, stop))),
    );
    expect(worst).toBeLessThan(WCAG_AA.normalText);
  });

  it("is why the primary action uses the flat accent, which passes", () => {
    /* The replacement, measured. Whatever is decided about the gradient, the
       control that ships has to clear AA. */
    expect(
      contrastRatio(
        resolveToken(tokens, "--accent-foreground"),
        resolveToken(tokens, "--accent"),
      ),
    ).toBeGreaterThanOrEqual(WCAG_AA.normalText);
  });
});
