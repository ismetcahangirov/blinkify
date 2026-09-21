import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { App } from "./App.js";

describe("App", () => {
  it("mounts and renders the wordmark", () => {
    render(<App />);
    expect(screen.getByRole("heading", { name: "Blinkify" })).toBeVisible();
  });

  it("reports the engine as not connected until something connects it", () => {
    // The shell must never imply a capability it does not have. An engine
    // status that defaults to "ready" is the same class of lie as an export
    // that reports lossless without checking.
    render(<App />);
    expect(screen.getByTestId("engine-status")).toHaveTextContent(
      "Engine: not-connected",
    );
  });
});
