/**
 * A drag of library assets onto another zone (#53).
 *
 * The drag crosses a React subtree boundary — the library and the timeline
 * are separate zones (#19) — so what travels is data: the source ids being
 * dragged and the pointer's position. A zone that accepts a drop registers a
 * target here and is told where the pointer is; it never learns anything of
 * the library's components, and the library never learns what a target does
 * with a drop.
 *
 * Pointer events, not HTML5 drag-and-drop: the application window takes the
 * native drag-and-drop for files dropped from the desktop, which on Windows
 * leaves no HTML5 drag inside the page.
 */

export interface AssetPayload {
  readonly sources: readonly number[];
}

/** A point in the window, in CSS pixels. */
export interface Point {
  readonly x: number;
  readonly y: number;
}

export interface DropTarget {
  /** Whether `point` is over this target. */
  contains: (point: Point) => boolean;
  /** The drag is over this target, at `point`: show where it would land. */
  hover: (
    payload: AssetPayload,
    point: Point,
    modifiers: DragModifiers,
  ) => void;
  /** The drag has left, or was abandoned: show nothing. */
  leave: () => void;
  /** Released over this target. */
  drop: (payload: AssetPayload, point: Point, modifiers: DragModifiers) => void;
}

export interface DragModifiers {
  /** Alt suspends snapping, as it does for a clip drag (#34). */
  readonly alt: boolean;
}

const targets = new Set<DropTarget>();

/** Accept drops on `target` until the returned function is called. */
export function registerDropTarget(target: DropTarget): () => void {
  targets.add(target);
  return () => {
    target.leave();
    targets.delete(target);
  };
}

/** A drag in progress. */
export interface AssetDrag {
  move: (point: Point, modifiers: DragModifiers) => void;
  /** Released at `point`: whether a target took the drop. */
  release: (point: Point, modifiers: DragModifiers) => boolean;
  /** Abandoned: nothing is dropped anywhere. */
  cancel: () => void;
}

export function startAssetDrag(payload: AssetPayload): AssetDrag {
  let over: DropTarget | null = null;
  const at = (point: Point): DropTarget | null => {
    for (const target of targets) if (target.contains(point)) return target;
    return null;
  };
  const enter = (target: DropTarget | null) => {
    if (target === over) return;
    over?.leave();
    over = target;
  };
  return {
    move: (point, modifiers) => {
      enter(at(point));
      over?.hover(payload, point, modifiers);
    },
    release: (point, modifiers) => {
      const target = at(point);
      enter(null);
      if (!target) return false;
      target.drop(payload, point, modifiers);
      return true;
    },
    cancel: () => enter(null),
  };
}
