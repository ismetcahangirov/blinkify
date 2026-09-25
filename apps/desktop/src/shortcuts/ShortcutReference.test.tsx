import { TooltipProvider } from "@blinkify/ui";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ShortcutReference } from "./ShortcutReference.js";
import { ACTIONS, BINDINGS, displayChord, type Chord } from "./shortcuts.js";
import { useShortcutsUi } from "./shortcuts.store.js";

describe("the shortcut reference", () => {
  it("shows every binding in the registry, and nothing else", () => {
    useShortcutsUi.setState({ referenceOpen: true });
    render(
      <TooltipProvider>
        <ShortcutReference />
      </TooltipProvider>,
    );
    const dialog = screen.getByRole("dialog", { name: "Keyboard shortcuts" });
    const shown = within(dialog)
      .getAllByText((_, element) => element?.tagName === "KBD")
      .map((kbd) => kbd.textContent)
      .sort();
    const registered = (Object.keys(BINDINGS) as Chord[])
      .map(displayChord)
      .sort();
    expect(shown).toEqual(registered);
    for (const info of Object.values(ACTIONS))
      expect(
        within(dialog).getByRole("rowheader", { name: info.label }),
      ).toBeVisible();
  });
});
