import {
  useRef,
  type PointerEvent,
  type KeyboardEvent,
  type RefObject,
} from "react";
import {
  clampFraction,
  fractionVariable,
  SPLITTER_KEYBOARD_STEP_PX,
  ZONE_BOUNDS,
  type ResizableZone,
} from "./layout.js";
import { useLayoutStore } from "./layout.store.js";

/**
 * One resizable divider.
 *
 * ── The thing this component exists to get right ───────────────────────────
 *
 * #19: "splitter dragging must not re-render zone contents … a layout that
 * re-renders a canvas timeline on every mouse move will feel broken regardless
 * of how fast the timeline is".
 *
 * So a drag never touches React state. `pointermove` writes a CSS custom
 * property straight onto the shell's root element, the browser recalculates the
 * grid, and nothing re-renders — not the zones, not the shell, not this
 * component. React is told once, on `pointerup`, and only so that the position
 * survives a restart.
 *
 * That is why this takes a ref to the root rather than a callback: a callback
 * that set state would be the bug.
 *
 * ── Why it is a real control and not a styled div ──────────────────────────
 *
 * The visual line is 1px, which is right for chrome and wrong for a pointer
 * target, so the hit area is widened to 8px by the stylesheet. That still
 * leaves a keyboard user with nothing, so the separator is focusable and
 * answers the arrow keys, Home and End — which is also what makes it
 * announceable, with a position a screen reader can read out.
 */

export interface SplitterProps {
  readonly zone: ResizableZone;
  /** `vertical` divides columns and is dragged sideways. */
  readonly orientation: "vertical" | "horizontal";
  /** The element carrying the layout custom properties. */
  readonly rootRef: RefObject<HTMLElement | null>;
  /** What the splitter resizes, for the accessible name: "Library width". */
  readonly label: string;
  /**
   * Which side of the divider the zone is on. The library grows as the pointer
   * moves right; the inspector and the timeline grow as it moves left or up.
   */
  readonly grows: "before" | "after";
}

export function Splitter({
  zone,
  orientation,
  rootRef,
  label,
  grows,
}: SplitterProps) {
  const setFraction = useLayoutStore((state) => state.setFraction);
  /* The live value during a drag. A ref rather than state for the same reason
     the whole component exists: state here would re-render on every frame. */
  const draggingFraction = useRef<number | null>(null);

  const bounds = ZONE_BOUNDS[zone];
  const isVertical = orientation === "vertical";

  /** The container the fraction is a fraction of. */
  const containerSize = (): number => {
    const root = rootRef.current;
    if (root === null) return 0;
    return isVertical ? root.clientWidth : root.clientHeight;
  };

  const currentFraction = (): number => {
    const root = rootRef.current;
    if (root === null) return bounds.defaultFraction;
    const raw = getComputedStyle(root).getPropertyValue(fractionVariable(zone));
    const parsed = Number.parseFloat(raw);
    return Number.isFinite(parsed) ? parsed : bounds.defaultFraction;
  };

  /** Write straight to the DOM. No state, no render. */
  const apply = (fraction: number): void => {
    rootRef.current?.style.setProperty(
      fractionVariable(zone),
      String(fraction),
    );
  };

  const handlePointerDown = (event: PointerEvent<HTMLDivElement>): void => {
    if (event.button !== 0) return;
    /* Pointer capture, so the drag survives the pointer leaving an 8px target —
       which it does immediately, because the gesture is hundreds of pixels long
       and the target is eight. */
    event.currentTarget.setPointerCapture(event.pointerId);
    draggingFraction.current = currentFraction();
    event.preventDefault();
  };

  const handlePointerMove = (event: PointerEvent<HTMLDivElement>): void => {
    if (draggingFraction.current === null) return;

    const root = rootRef.current;
    const size = containerSize();
    if (root === null || size <= 0) return;

    const box = root.getBoundingClientRect();
    /* Measured from the container edge rather than accumulated from the drag's
       origin. Accumulating drifts: every clamp along the way is lost, and the
       zone jumps when the pointer comes back from beyond the minimum. */
    const fromStart = isVertical
      ? event.clientX - box.left
      : event.clientY - box.top;
    const extent = grows === "before" ? fromStart : size - fromStart;

    const next = clampFraction(zone, extent / size, size);
    draggingFraction.current = next;
    apply(next);
  };

  const endDrag = (event: PointerEvent<HTMLDivElement>): void => {
    const fraction = draggingFraction.current;
    if (fraction === null) return;
    draggingFraction.current = null;

    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }

    // The one moment React hears about any of this.
    setFraction(zone, fraction);
  };

  const step = (direction: -1 | 1): void => {
    const size = containerSize();
    if (size <= 0) return;
    const delta = (SPLITTER_KEYBOARD_STEP_PX / size) * direction;
    const next = clampFraction(zone, currentFraction() + delta, size);
    apply(next);
    setFraction(zone, next);
  };

  const jump = (to: "minimum" | "maximum"): void => {
    const size = containerSize();
    if (size <= 0) return;
    const target =
      to === "minimum" ? bounds.minimumPx / size : bounds.maximumFraction;
    const next = clampFraction(zone, target, size);
    apply(next);
    setFraction(zone, next);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>): void => {
    const decrease = isVertical ? "ArrowLeft" : "ArrowUp";
    const increase = isVertical ? "ArrowRight" : "ArrowDown";

    /* The keys move the *splitter*, and the zone grows or shrinks depending on
       which side of it the zone is on. Anything else means the inspector's
       right-arrow makes it smaller, which nobody expects twice. */
    const towardsLarger = grows === "before" ? increase : decrease;
    const towardsSmaller = grows === "before" ? decrease : increase;

    if (event.key === towardsLarger) {
      event.preventDefault();
      step(1);
    } else if (event.key === towardsSmaller) {
      event.preventDefault();
      step(-1);
    } else if (event.key === "Home") {
      event.preventDefault();
      jump("minimum");
    } else if (event.key === "End") {
      event.preventDefault();
      jump("maximum");
    }
  };

  return (
    <div
      className="shell__splitter"
      data-orientation={orientation}
      data-zone={zone}
      /* `separator` with a tabindex is the ARIA pattern for a resizable
         splitter: a plain separator is decorative and is not focusable, and the
         difference is exactly whether a keyboard user can move it. */
      role="separator"
      tabIndex={0}
      aria-label={label}
      aria-orientation={orientation}
      aria-valuemin={0}
      aria-valuemax={100}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      onKeyDown={handleKeyDown}
      /* A double-click resets the zone to its documented default. Cheap to
         implement, and the only way back for someone who has dragged a panel
         somewhere they did not mean to. */
      onDoubleClick={() => {
        apply(bounds.defaultFraction);
        setFraction(zone, bounds.defaultFraction);
      }}
    />
  );
}
