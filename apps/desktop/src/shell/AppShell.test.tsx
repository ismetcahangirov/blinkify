import { TooltipProvider } from "@blinkify/ui";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { memo, useRef, type ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AppShell } from "./AppShell.js";
import {
  fractionVariable,
  SPLITTER_KEYBOARD_STEP_PX,
  ZONE_BOUNDS,
} from "./layout.js";
import {
  LAYOUT_STORAGE_KEY,
  readStoredLayout,
  useLayoutStore,
} from "./layout.store.js";

/**
 * The shell's two load-bearing behaviours (#19): a drag that re-renders
 * nothing, and a layout that survives a restart.
 *
 * ── Why the sizes have to be faked ──────────────────────────────────────────
 *
 * jsdom performs no layout. Every element is zero by zero, `clientWidth` is 0
 * and `getBoundingClientRect()` returns zeros — so a splitter asked to convert
 * a pointer position into a fraction of its container would be dividing by
 * nothing, and the handlers correctly refuse to.
 *
 * The container's measurements are therefore stubbed. That is honest about what
 * is being tested: the arithmetic and the render behaviour, not the browser's
 * grid implementation. What the grid does with a fraction is the browser's job
 * and is not something a test of ours can add confidence to.
 */

/**
 * The shell under the provider the application mounts it under.
 *
 * `main.tsx` wraps the whole application in one `TooltipProvider` because that
 * provider is the tooltip group's shared delay clock. Rendering the shell
 * without it throws, which is Radix being right: a tooltip with its own private
 * timer is the flickering toolbar the delay exists to prevent, so there is no
 * sensible fallback for it to quietly use.
 */
function mount(ui: ReactElement) {
  return render(<TooltipProvider>{ui}</TooltipProvider>);
}

/*
 * The default window from `tauri.conf.json`, not the minimum one.
 *
 * At the 1280px minimum the inspector's documented 20% default is 256px, which
 * is under its documented 260px minimum, so it opens clamped to its floor
 * rather than at its default. That is the layout behaving correctly at the
 * edge of its range, and it has a test of its own below — but it makes a poor
 * baseline for everything else, because every expectation would be a clamp
 * rather than the thing being measured.
 */
const CONTAINER_WIDTH = 1440;
const CONTAINER_HEIGHT = 800;

/** Give the shell body the dimensions jsdom will not. */
function measureBody(): HTMLElement {
  const body = screen.getByTestId("shell-body");

  Object.defineProperty(body, "clientWidth", {
    value: CONTAINER_WIDTH,
    configurable: true,
  });
  Object.defineProperty(body, "clientHeight", {
    value: CONTAINER_HEIGHT,
    configurable: true,
  });
  body.getBoundingClientRect = () => ({
    x: 0,
    y: 0,
    left: 0,
    top: 0,
    right: CONTAINER_WIDTH,
    bottom: CONTAINER_HEIGHT,
    width: CONTAINER_WIDTH,
    height: CONTAINER_HEIGHT,
    toJSON: () => ({}),
  });

  return body;
}

/** What a zone's fraction currently is, straight off the element. */
function fractionOf(
  body: HTMLElement,
  zone: "library" | "inspector" | "timeline",
): number {
  return Number.parseFloat(body.style.getPropertyValue(fractionVariable(zone)));
}

/**
 * A zone that counts how many times React rendered it.
 *
 * `memo` matches the real zones, because the question is whether the shell
 * causes a render at all — not whether a zone would have been cheap to render.
 */
function countingZone(counts: { value: number }) {
  return memo(function CountingZone() {
    counts.value += 1;
    return <div data-testid="counting-zone">zone</div>;
  });
}

function renderShell() {
  const counts = { value: 0 };
  const CountingZone = countingZone(counts);

  const view = mount(
    <AppShell
      library={<CountingZone />}
      player={<div>player</div>}
      inspector={<div>inspector</div>}
      timeline={<div>timeline</div>}
    />,
  );

  return { ...view, counts, body: measureBody() };
}

beforeEach(() => {
  globalThis.localStorage.clear();
  useLayoutStore.setState({
    fractions: { library: 0.22, inspector: 0.2, timeline: 0.38 },
  });
});

afterEach(() => {
  globalThis.localStorage.clear();
});

describe("the four zones", () => {
  it("renders all four, each as its own labelled region", () => {
    renderShell();

    for (const zone of ["Library", "Player", "Inspector", "Timeline"]) {
      expect(screen.getByRole("region", { name: zone })).toBeInTheDocument();
    }
  });

  it("starts at the documented default proportions", () => {
    /* The acceptance criterion for a first launch. The values are asserted
       against the contract rather than against literals, so this test cannot
       disagree with `layout.test.ts` about what the defaults are. */
    const { body } = renderShell();

    for (const zone of ["library", "inspector", "timeline"] as const) {
      expect(fractionOf(body, zone)).toBeCloseTo(
        ZONE_BOUNDS[zone].defaultFraction,
        5,
      );
    }
  });

  it("gives every splitter an accessible name and a keyboard", () => {
    renderShell();

    for (const name of [
      "Library width",
      "Inspector width",
      "Timeline height",
    ]) {
      const splitter = screen.getByRole("separator", { name });
      expect(splitter).toHaveAttribute("tabindex", "0");
    }
  });
});

describe("dragging a splitter", () => {
  /**
   * The acceptance criterion this whole design exists for: "dragging a splitter
   * does not cause a re-render in the zones being resized".
   *
   * The drag writes a CSS custom property straight onto the DOM. If anybody
   * ever routes it through React state, the count below goes up and this fails
   * — which is the only way that regression is visible, because the interface
   * would look and behave identically until the timeline has a canvas in it.
   */
  it("moves the zone without rendering it again", () => {
    const { body, counts } = renderShell();

    const before = counts.value;
    expect(before).toBeGreaterThan(0);

    const splitter = screen.getByRole("separator", { name: "Library width" });

    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: 220,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointermove", {
        bubbles: true,
        clientX: CONTAINER_WIDTH * 0.35,
      }),
    );

    // The layout moved…
    expect(fractionOf(body, "library")).toBeCloseTo(0.35, 5);
    // …and React was not involved.
    expect(counts.value).toBe(before);

    splitter.dispatchEvent(
      new PointerEvent("pointerup", {
        bubbles: true,
        clientX: CONTAINER_WIDTH * 0.35,
      }),
    );

    /* Committing on release is allowed to notify the store, and still must not
       render a zone: nothing in the tree subscribes to the value. */
    expect(counts.value).toBe(before);
    expect(useLayoutStore.getState().fractions.library).toBeCloseTo(0.35, 5);
  });

  it("stops at the minimum rather than collapsing the zone", () => {
    const { body } = renderShell();
    const splitter = screen.getByRole("separator", { name: "Library width" });

    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: 220,
      }),
    );
    // Dragged hard against the left edge, and past it.
    splitter.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, clientX: -400 }),
    );

    expect(fractionOf(body, "library") * CONTAINER_WIDTH).toBeCloseTo(
      ZONE_BOUNDS.library.minimumPx,
      5,
    );
  });

  it("stops at the maximum rather than eating the player", () => {
    const { body } = renderShell();
    const splitter = screen.getByRole("separator", { name: "Library width" });

    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: 220,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointermove", { bubbles: true, clientX: 5000 }),
    );

    expect(fractionOf(body, "library")).toBeCloseTo(
      ZONE_BOUNDS.library.maximumFraction,
      5,
    );
  });

  it("measures the inspector from the other edge", () => {
    /* The inspector grows as the pointer moves *left*. Getting this backwards
       is the kind of bug that looks like the splitter being inverted and is
       trivially missed when only one splitter is tested. */
    const { body } = renderShell();
    const splitter = screen.getByRole("separator", { name: "Inspector width" });

    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: CONTAINER_WIDTH * 0.8,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointermove", {
        bubbles: true,
        clientX: CONTAINER_WIDTH * 0.7,
      }),
    );

    expect(fractionOf(body, "inspector")).toBeCloseTo(0.3, 5);
  });

  it("measures the timeline from the bottom", () => {
    const { body } = renderShell();
    const splitter = screen.getByRole("separator", { name: "Timeline height" });

    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientY: CONTAINER_HEIGHT * 0.6,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointermove", {
        bubbles: true,
        clientY: CONTAINER_HEIGHT * 0.5,
      }),
    );

    expect(fractionOf(body, "timeline")).toBeCloseTo(0.5, 5);
  });
});

describe("the splitter keyboard", () => {
  it("moves with the arrow keys", async () => {
    const user = userEvent.setup();
    const { body } = renderShell();

    const splitter = screen.getByRole("separator", { name: "Library width" });
    splitter.focus();

    const before = fractionOf(body, "library");
    await user.keyboard("{ArrowRight}");

    // One --space-4 step, expressed as a fraction of the container.
    expect(fractionOf(body, "library")).toBeCloseTo(
      before + SPLITTER_KEYBOARD_STEP_PX / CONTAINER_WIDTH,
      5,
    );
  });

  it("moves the right way round for a zone on the other side", async () => {
    const user = userEvent.setup();
    const { body } = renderShell();

    const splitter = screen.getByRole("separator", { name: "Inspector width" });
    splitter.focus();

    const before = fractionOf(body, "inspector");
    /* Left arrow moves the splitter left, which makes the inspector *bigger*.
       The alternative — arrow direction meaning "grow" regardless of side —
       means the two vertical splitters respond to the same key in opposite
       directions on screen. */
    await user.keyboard("{ArrowLeft}");

    expect(fractionOf(body, "inspector")).toBeCloseTo(
      before + SPLITTER_KEYBOARD_STEP_PX / CONTAINER_WIDTH,
      5,
    );
  });

  it("jumps to the bounds with Home and End", async () => {
    const user = userEvent.setup();
    const { body } = renderShell();

    const splitter = screen.getByRole("separator", { name: "Library width" });
    splitter.focus();

    await user.keyboard("{End}");
    expect(fractionOf(body, "library")).toBeCloseTo(
      ZONE_BOUNDS.library.maximumFraction,
      5,
    );

    await user.keyboard("{Home}");
    expect(fractionOf(body, "library") * CONTAINER_WIDTH).toBeCloseTo(
      ZONE_BOUNDS.library.minimumPx,
      5,
    );
  });

  it("returns a zone to its default on a double click", async () => {
    const user = userEvent.setup();
    const { body } = renderShell();

    const splitter = screen.getByRole("separator", { name: "Library width" });
    splitter.focus();
    await user.keyboard("{End}");
    expect(fractionOf(body, "library")).not.toBeCloseTo(
      ZONE_BOUNDS.library.defaultFraction,
      5,
    );

    await user.dblClick(splitter);

    expect(fractionOf(body, "library")).toBeCloseTo(
      ZONE_BOUNDS.library.defaultFraction,
      5,
    );
  });
});

describe("persistence across a restart", () => {
  it("comes back where it was left", () => {
    /* A restart is a fresh module state reading the same storage. The store is
       re-seeded from `localStorage` exactly as it is on a cold launch, and the
       shell is mounted again from nothing. */
    const { unmount, body } = renderShell();

    const splitter = screen.getByRole("separator", { name: "Library width" });
    splitter.dispatchEvent(
      new PointerEvent("pointerdown", {
        bubbles: true,
        button: 0,
        clientX: 220,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointermove", {
        bubbles: true,
        clientX: CONTAINER_WIDTH * 0.3,
      }),
    );
    splitter.dispatchEvent(
      new PointerEvent("pointerup", {
        bubbles: true,
        clientX: CONTAINER_WIDTH * 0.3,
      }),
    );
    expect(fractionOf(body, "library")).toBeCloseTo(0.3, 5);

    unmount();

    /* What a relaunch does, through the same function a cold launch calls.
       Parsing the storage by hand here would test the test's idea of the format
       rather than the reader that actually runs on startup. */
    expect(globalThis.localStorage.getItem(LAYOUT_STORAGE_KEY)).not.toBeNull();
    useLayoutStore.setState({ fractions: readStoredLayout() });

    const second = renderShell();
    expect(fractionOf(second.body, "library")).toBeCloseTo(0.3, 5);
  });

  it("opens at the defaults when the stored preference is corrupt", () => {
    /* A preference file is on the user's disk and can be anything. Failing to
       open because a panel width did not parse would be the worst possible
       trade. */
    globalThis.localStorage.setItem(LAYOUT_STORAGE_KEY, "{not json at all");

    useLayoutStore.setState({
      fractions: {
        library: ZONE_BOUNDS.library.defaultFraction,
        inspector: ZONE_BOUNDS.inspector.defaultFraction,
        timeline: ZONE_BOUNDS.timeline.defaultFraction,
      },
    });

    const { body } = renderShell();
    expect(fractionOf(body, "library")).toBeCloseTo(
      ZONE_BOUNDS.library.defaultFraction,
      5,
    );
  });
});

describe("a zone re-rendering", () => {
  it("does not re-render its neighbours", () => {
    /* "Each zone is an independent React subtree, so a re-render in one does
       not re-render the others." The library zone below renders on every tick
       of its own state; the timeline's count must not move. */
    const libraryCounts = { value: 0 };
    const timelineCounts = { value: 0 };

    const Library = memo(function Library() {
      const renders = useRef(0);
      renders.current += 1;
      libraryCounts.value = renders.current;
      return <button type="button">library</button>;
    });
    const Timeline = memo(function Timeline() {
      timelineCounts.value += 1;
      return <div>timeline</div>;
    });

    const { rerender } = mount(
      <AppShell
        library={<Library />}
        player={<div>player</div>}
        inspector={<div>inspector</div>}
        timeline={<Timeline />}
      />,
    );

    const timelineBefore = timelineCounts.value;

    // A new element for the library only. The timeline's element is unchanged,
    // so `memo` stops the render at its boundary.
    rerender(
      <TooltipProvider>
        <AppShell
          library={<Library />}
          player={<div>player</div>}
          inspector={<div>inspector</div>}
          timeline={<Timeline />}
        />
      </TooltipProvider>,
    );

    expect(timelineCounts.value).toBe(timelineBefore);
  });
});
