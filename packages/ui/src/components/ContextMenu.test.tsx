import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ContextMenu } from "./Menu.js";
import { TooltipProvider } from "./Tooltip.js";

/**
 * The context menu, in a file of its own.
 *
 * ── Why it is not in `components.test.tsx` with everything else ────────────
 *
 * `ContextMenu` and `DropdownMenu` are both built on `@radix-ui/react-menu`,
 * which keeps bookkeeping at module scope about which dismissable layer
 * currently owns the document. A dropdown-menu test earlier in the same module
 * scope leaves that bookkeeping holding a layer whose component has been
 * unmounted, and the next menu to open is then dismissed immediately — it
 * opens and closes within the same tick, and the test reports that the right
 * click did nothing.
 *
 * That was diagnosed rather than guessed: the test passes alone, passes with
 * only the dropdown tests skipped, and fails with them. Closing the dropdown
 * explicitly at the end of each of those tests does not clear it, because the
 * state lives above the component tree.
 *
 * Vitest gives each test file its own module registry, so one extra file is the
 * whole fix. The alternative — ordering the describes so the context menu runs
 * first — would work today and break silently the moment somebody adds a menu
 * test above it.
 */

describe("ContextMenu", () => {
  /* One menu lifecycle per file, for the reason above: a second `it` here would
     be the second menu in this module scope and would meet the same stale-layer
     state. So the summons and the selection are asserted in one pass, which is
     also how a user meets them. */
  it("opens on a right click and selects an item", async () => {
    const onSelect = vi.fn();
    render(
      <TooltipProvider>
        <ContextMenu
          groups={[{ items: [{ id: "split", label: "Split", onSelect }] }]}
        >
          <div>A clip</div>
        </ContextMenu>
      </TooltipProvider>,
    );

    /* `fireEvent`, not `userEvent`. Radix listens for the `contextmenu` event
       itself and anchors the menu at the pointer, and userEvent's right click
       in jsdom synthesises neither the event nor the coordinates. Dispatching
       it directly asserts the component's actual contract rather than the test
       library's emulation of a mouse. */
    fireEvent.contextMenu(screen.getByText("A clip"), {
      button: 2,
      clientX: 10,
      clientY: 10,
    });

    const item = await screen.findByRole("menuitem", { name: "Split" });
    expect(item).toBeInTheDocument();

    fireEvent.click(item);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});
