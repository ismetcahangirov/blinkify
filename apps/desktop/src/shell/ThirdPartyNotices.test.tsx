import { TooltipProvider } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { AppBar } from "./AppBar.js";
import { ThirdPartyNotices } from "./ThirdPartyNotices.js";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(() => Promise.resolve(() => undefined)),
}));

const invoked = vi.mocked(invoke);

const NOTICES =
  "Blinkify: third-party notices\n\nserde 1.0.229 (Rust crate)\n  Taken under: MIT\n";

function show(open = true) {
  return render(
    <TooltipProvider>
      <ThirdPartyNotices open={open} onOpenChange={() => undefined} />
    </TooltipProvider>,
  );
}

describe("the third-party notices", () => {
  beforeEach(() => {
    invoked.mockReset();
  });

  it("shows the installed document, read by the shell, exactly as it is", async () => {
    invoked.mockResolvedValue(NOTICES);
    show();
    expect(
      screen.getByRole("dialog", { name: "Third-party notices" }),
    ).toBeVisible();
    const text = await screen.findByTestId("third-party-notices");
    expect(text.textContent).toBe(NOTICES);
    expect(invoked).toHaveBeenCalledWith("third_party_notices");
  });

  it("says why when the document cannot be read, rather than showing nothing", async () => {
    invoked.mockRejectedValue("THIRD-PARTY-NOTICES.txt was not found");
    show();
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The notices could not be read: THIRD-PARTY-NOTICES.txt was not found",
    );
  });

  it("reads nothing until it is opened", () => {
    show(false);
    expect(invoked).not.toHaveBeenCalled();
  });

  it("is reachable from Help in the application bar", async () => {
    invoked.mockImplementation((command) =>
      Promise.resolve(command === "third_party_notices" ? NOTICES : undefined),
    );
    const user = userEvent.setup();
    render(
      <TooltipProvider>
        <AppBar />
      </TooltipProvider>,
    );
    // From the keyboard: jsdom has no pointer geometry for Radix to open on.
    screen.getByRole("button", { name: "Help" }).focus();
    await user.keyboard("{Enter}");
    await user.click(
      await screen.findByRole("menuitem", { name: "Third-party notices…" }),
    );
    expect(
      await screen.findByRole("dialog", { name: "Third-party notices" }),
    ).toBeVisible();
    expect((await screen.findByTestId("third-party-notices")).textContent).toBe(
      NOTICES,
    );
  });
});
