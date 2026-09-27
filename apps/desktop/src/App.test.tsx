import { TooltipProvider } from "@blinkify/ui";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App.js";

/**
 * The application, composed.
 *
 * `App` is almost nothing since #19 — it hands four zones to the shell — so
 * what is worth asserting here is the composition rather than any behaviour.
 * The shell's own behaviour is `shell/AppShell.test.tsx`, and the layout
 * contract is `shell/layout.test.ts`.
 */

const mount = () =>
  render(
    <TooltipProvider>
      <App />
    </TooltipProvider>,
  );

describe("App", () => {
  it("mounts the four zones", () => {
    mount();

    for (const zone of ["Library", "Player", "Inspector", "Timeline"]) {
      expect(screen.getByRole("region", { name: zone })).toBeInTheDocument();
    }
  });

  it("reports the engine as ready only when the engine says so", async () => {
    // The shell must never imply a capability it does not have. An engine
    // status that defaults to "ready" is the same class of lie as an export
    // that reports lossless without checking. Here there is no engine at all
    // (no Tauri), so the answer is that it is unavailable (#151).
    mount();
    expect(screen.getByTestId("engine-status")).not.toHaveTextContent(
      "Engine: ready",
    );
    expect(await screen.findByText(/^Engine: unavailable/)).toBeInTheDocument();
  });

  it("does not claim an export tier before anything has computed one", () => {
    /* With no project there is no plan (#39), and the indicator says so —
       the one claim this product is built on is not made optimistically. */
    mount();
    expect(screen.getByTestId("lossless-indicator")).toHaveTextContent(
      "not computed",
    );
  });
});
