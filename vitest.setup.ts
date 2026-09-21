import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// Testing Library's automatic cleanup hooks into a *global* afterEach, which
// only exists when Vitest runs with `globals: true`. We import explicitly, so
// the cleanup has to be explicit too — without it the second test in a file
// queries a DOM that still holds the first test's render, and fails with
// "found multiple elements" rather than anything that points at the cause.
afterEach(() => {
  cleanup();
});
