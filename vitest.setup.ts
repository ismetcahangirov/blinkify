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

/*
 * Browser APIs jsdom does not implement, which Radix uses.
 *
 * These are stubs for a test environment, not shims for the product. WebView2
 * is evergreen Chromium and implements all three; jsdom implements none, and a
 * component that measures itself throws on import rather than failing an
 * assertion — which reads as the component being broken rather than as the
 * environment being thin.
 *
 * Each one is the smallest thing that lets the component mount. A
 * `ResizeObserver` that never fires is correct here: there is no layout in
 * jsdom, so there is nothing for it to observe, and a test that depended on it
 * firing would be a test asserting something jsdom cannot tell it.
 */
if (!("ResizeObserver" in globalThis)) {
  class ResizeObserverStub {
    /* Arrow properties rather than methods: these are handed around detached
       from the instance, and a method would carry an implicit `this` that
       lint is right to object to. */
    readonly observe = (): void => undefined;
    readonly unobserve = (): void => undefined;
    readonly disconnect = (): void => undefined;
  }
  globalThis.ResizeObserver = ResizeObserverStub;
}

if (!("DOMRect" in globalThis)) {
  // Radix's popper measures its trigger before positioning.
  class DOMRectStub {
    constructor(
      readonly x = 0,
      readonly y = 0,
      readonly width = 0,
      readonly height = 0,
    ) {}
    readonly top = 0;
    readonly left = 0;
    readonly right = 0;
    readonly bottom = 0;
    readonly toJSON = (): object => ({});
  }
  globalThis.DOMRect = DOMRectStub as unknown as typeof DOMRect;
}

/*
 * Pointer capture and `scrollIntoView`, called by Select and Slider on every
 * open and every drag. jsdom defines neither.
 *
 * Assigned through an index rather than by name so that lint does not read a
 * typed method off the prototype — `unbound-method` is right to object to that
 * everywhere except here, where the point is to replace the property itself.
 */
if (typeof Element !== "undefined") {
  const prototype = Element.prototype as unknown as Record<string, unknown>;
  prototype["hasPointerCapture"] ??= () => false;
  prototype["setPointerCapture"] ??= () => undefined;
  prototype["releasePointerCapture"] ??= () => undefined;
  prototype["scrollIntoView"] ??= () => undefined;
}
