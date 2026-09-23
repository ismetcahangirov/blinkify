import { type DragDropEvent, getCurrentWebview } from "@tauri-apps/api/webview";
import { type RefObject, useEffect, useRef } from "react";

/**
 * Whether a drop at `position` — physical pixels, as the webview reports it —
 * landed on `rect`, which is in CSS pixels.
 */
export function droppedOn(
  position: { x: number; y: number },
  rect: Pick<DOMRect, "left" | "top" | "right" | "bottom">,
  devicePixelRatio: number,
): boolean {
  const x = position.x / devicePixelRatio;
  const y = position.y / devicePixelRatio;
  return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
}

/**
 * Call `onDrop` with the first path of any files dropped onto `target`.
 *
 * The webview's native drag-and-drop gives real file-system paths, which the
 * HTML5 drop event does not — and the engine needs a path, because it opens
 * the file itself, read-only (`CLAUDE.md` section 19).
 */
export function useFileDrop(
  target: RefObject<HTMLElement | null>,
  onDrop: (path: string) => void,
): void {
  const onDropRef = useRef(onDrop);
  useEffect(() => {
    onDropRef.current = onDrop;
  }, [onDrop]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const handle = (event: { payload: DragDropEvent }) => {
      if (event.payload.type !== "drop") return;
      const element = target.current;
      const [path] = event.payload.paths;
      if (!element || path === undefined) return;
      if (
        droppedOn(
          event.payload.position,
          element.getBoundingClientRect(),
          window.devicePixelRatio || 1,
        )
      ) {
        onDropRef.current(path);
      }
    };
    let listening: Promise<() => void>;
    try {
      listening = getCurrentWebview().onDragDropEvent(handle);
    } catch {
      // Outside the application window (a test, or `pnpm dev` in a browser)
      // there is no webview to listen to, and nothing can be dropped.
      return undefined;
    }
    listening
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [target]);
}
