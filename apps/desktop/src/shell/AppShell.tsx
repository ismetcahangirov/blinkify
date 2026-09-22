import { useLayoutEffect, useRef, type ReactNode } from "react";
import { AppBar } from "./AppBar.js";
import { fractionVariable, RESIZABLE_ZONES } from "./layout.js";
import { useLayoutStore } from "./layout.store.js";
import { Splitter } from "./Splitter.js";

/**
 * The four-zone shell (#19), built to `docs/design/capcut-layout-reference.md`.
 *
 * ── The zones arrive as props ───────────────────────────────────────────────
 *
 * Not because the shell is reusable — there is one window — but because it
 * makes the shell a layout and nothing else. It cannot reach into a panel's
 * state, it cannot grow a special case for one zone, and the render-count test
 * can hand it four instrumented subtrees and watch what happens to them during
 * a drag. A shell that imported its own panels could not be asked that
 * question.
 *
 * ── Why this component does not subscribe to the layout store ──────────────
 *
 * It reads the store once, imperatively, to seed the CSS custom properties, and
 * never again. Nothing here re-renders when a splitter commits, because nothing
 * here is subscribed to the value that changed.
 *
 * That is the mechanism behind "dragging a splitter does not cause a re-render
 * in the zones being resized". It is not `memo` — `memo` would mean the shell
 * re-renders and React then compares props to decide the zones need not. This
 * way the render never starts. The difference matters at 120 pointer events a
 * second with a canvas timeline in one of the zones.
 *
 * ── Where the sizes actually come from ─────────────────────────────────────
 *
 * Two custom properties per zone, both on `.shell__body`:
 *
 *   --layout-<zone>-fraction   written by the splitter, read by the grid
 *   --layout-<zone>-min        a pixel floor, applied by `clamp()` in the CSS
 *
 * The CSS `clamp()` is what keeps a zone above its minimum when the *window*
 * shrinks, which no amount of clamping in the drag handler would catch.
 */

export interface AppShellProps {
  readonly library: ReactNode;
  readonly player: ReactNode;
  readonly inspector: ReactNode;
  readonly timeline: ReactNode;
}

export function AppShell({
  library,
  player,
  inspector,
  timeline,
}: AppShellProps) {
  const bodyRef = useRef<HTMLDivElement>(null);

  /*
   * Seed the layout from the stored preference.
   *
   * `useLayoutEffect`, not `useEffect`: the properties have to be on the
   * element before the browser paints, or the first frame shows the defaults
   * and the panels jump a moment later. That jump is the one visual defect a
   * user notices on every single launch.
   *
   * Read through `getState()` rather than a selector, so this component never
   * subscribes — see the note above.
   */
  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (body === null) return;

    const { fractions } = useLayoutStore.getState();
    for (const zone of RESIZABLE_ZONES) {
      body.style.setProperty(fractionVariable(zone), String(fractions[zone]));
    }
  }, []);

  return (
    <div className="shell">
      <AppBar />

      <div className="shell__body" ref={bodyRef} data-testid="shell-body">
        <div className="shell__upper">
          <section
            className="shell__zone shell__zone--library"
            aria-label="Library"
          >
            {library}
          </section>

          <Splitter
            zone="library"
            orientation="vertical"
            rootRef={bodyRef}
            label="Library width"
            grows="before"
          />

          <section
            className="shell__zone shell__zone--player"
            aria-label="Player"
          >
            {player}
          </section>

          <Splitter
            zone="inspector"
            orientation="vertical"
            rootRef={bodyRef}
            label="Inspector width"
            grows="after"
          />

          <section
            className="shell__zone shell__zone--inspector"
            aria-label="Inspector"
          >
            {inspector}
          </section>
        </div>

        <Splitter
          zone="timeline"
          orientation="horizontal"
          rootRef={bodyRef}
          label="Timeline height"
          grows="after"
        />

        <section
          className="shell__zone shell__zone--timeline"
          aria-label="Timeline"
        >
          {timeline}
        </section>
      </div>
    </div>
  );
}
