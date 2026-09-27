import type { CropFrame, CropRect } from "@blinkify/types";
import { Button, Select } from "@blinkify/ui";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
} from "react";
import { ASPECTS, presetOf } from "../inspector/crop.js";
import { editGesture, type EditGesture } from "../inspector/editGesture.js";
import {
  useProjectStore,
  type DeepReadonly,
} from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import {
  HANDLES,
  dragRect,
  handleAt,
  handlePoint,
  keptAspect,
  nudge,
  pictureBox,
  toCanvas,
  toSource,
  type Box,
  type Handle,
} from "./cropFraming.js";
import { useCropFramingStore } from "./cropFraming.store.js";
import { usePreviewStore } from "./preview.store.js";

type Frame = DeepReadonly<CropFrame>;
type Rect = DeepReadonly<CropRect>;

/** Half the side of a drawn handle, in CSS pixels. */
const HANDLE_SIZE = 4;

const CURSORS: Record<Handle, string> = {
  nw: "nwse-resize",
  se: "nwse-resize",
  ne: "nesw-resize",
  sw: "nesw-resize",
  n: "ns-resize",
  s: "ns-resize",
  e: "ew-resize",
  w: "ew-resize",
  move: "move",
};

/** A drag under way: where it started, in the source's display pixels. */
interface Drag {
  readonly handle: Handle;
  readonly pointer: number;
  readonly start: Rect;
  readonly from: { readonly x: number; readonly y: number };
  readonly gesture: EditGesture;
  /** Whether an edit has been sent, so a cancel knows to take it back. */
  sent: boolean;
}

/**
 * Framing a crop on the preview (#131).
 *
 * The whole picture is shown — the engine plays the framed clip uncropped —
 * with the area outside the rectangle dimmed, eight handles and a draggable
 * inside. Everything is drawn on one canvas over the preview, never as a DOM
 * node per handle, so a drag stays at the frame rate (`CLAUDE.md` §12).
 *
 * - **A drag is one gesture** (#37): the rectangle follows the pointer and is
 *   sent live, and the whole drag is one undo entry on release.
 * - **On the grid, always.** Every rectangle is snapped to the source's
 *   chroma grid, kept inside the picture and at least the minimum size, in
 *   display orientation, whatever the display scale (`cropFraming.ts`).
 * - **The shape** of a preset is kept while resizing; Shift keeps whatever
 *   shape the rectangle has. The inspector's presets are here too.
 * - **Keys**: the arrows move the rectangle one grid step, ten with Shift.
 *   Escape during a drag puts the crop back as it was before the drag;
 *   otherwise Escape, Enter or Done leave, keeping the crop as it stands.
 *   Selecting another clip leaves too. The rule is `docs/design/crop-framing.md`.
 */
export function CropOverlay() {
  const clip = useCropFramingStore((state) => state.clip);
  // One framing, one component: nothing of a drag outlives it.
  return clip === null ? null : <Framing key={clip} clip={clip} />;
}

function Framing({ clip }: { clip: number }) {
  const leave = useCropFramingStore((state) => state.leave);
  const view = useProjectStore((state) => state.view);
  const selection = useProjectStore((state) => state.selection);
  const edit = useProjectStore((state) => state.edit);
  const undo = useProjectStore((state) => state.undo);
  const picture = usePreviewStore((state) => state.picture);
  const previewing = usePreviewStore((state) => state.kind === "project");
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const drag = useRef<Drag | null>(null);
  const keys = useRef<EditGesture | null>(null);
  const [draft, setDraft] = useState<Rect | null>(null);
  const [hover, setHover] = useState<Handle | null>(null);
  const [grabbed, setGrabbed] = useState<Handle | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0, ratio: 1 });

  const placement: DeepReadonly<Placement> | undefined = view?.timeline?.tracks
    .flatMap((track) => track.placements)
    .find((candidate) => candidate.clip === clip);
  const frame: Frame | undefined =
    placement === undefined ? undefined : view?.frames[placement.source];
  const rect: Rect | null = frame
    ? (placement?.crop ?? {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
      })
    : null;
  const shown = draft ?? rect;

  const box: Box | null =
    picture && size.width > 0
      ? pictureBox(picture, size.width, size.height, size.ratio)
      : null;

  const cancelDrag = useCallback(async () => {
    const open = drag.current;
    drag.current = null;
    setDraft(null);
    setGrabbed(null);
    if (!open) return;
    await open.gesture.end();
    // The drag was one entry: taking it back leaves the crop as it was.
    if (open.sent) await undo();
  }, [undo]);

  // Leaving the clip — another selected, or it is gone — keeps the crop.
  useEffect(() => {
    const alone = selection.length === 1 && selection[0] === clip;
    if (!alone || !previewing || (view !== null && placement === undefined)) {
      const open = drag.current;
      drag.current = null;
      if (open) void open.gesture.end();
      void leave();
    }
  }, [clip, selection, placement, view, previewing, leave]);

  // Focus comes here so the keys work at once.
  useEffect(() => {
    canvasRef.current?.focus();
  }, []);

  // The canvas is the size of the preview surface, at device pixels.
  useLayoutEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return undefined;
    const measure = () => {
      setSize({
        width: canvas.clientWidth,
        height: canvas.clientHeight,
        ratio: window.devicePixelRatio || 1,
      });
    };
    measure();
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(measure);
    observer?.observe(canvas);
    return () => observer?.disconnect();
  }, []);

  useEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    const width = Math.max(1, Math.round(size.width * size.ratio));
    const height = Math.max(1, Math.round(size.height * size.ratio));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.clearRect(0, 0, width, height);
    if (!box || !frame || !shown) return;
    context.scale(size.ratio, size.ratio);
    draw(context, box, toCanvas(box, frame, shown), readColours(canvas));
  }, [box, frame, shown, size]);

  const point = (event: PointerEvent<HTMLCanvasElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  };

  const onPointerDown = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!box || !frame || !rect || drag.current) return;
    const at = point(event);
    const handle = handleAt(toCanvas(box, frame, rect), at.x, at.y);
    if (!handle) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setGrabbed(handle);
    drag.current = {
      handle,
      pointer: event.pointerId,
      start: rect,
      from: toSource(box, frame, at.x, at.y),
      gesture: editGesture("Crop clip"),
      sent: false,
    };
  };

  const onPointerMove = (event: PointerEvent<HTMLCanvasElement>) => {
    if (!box || !frame || !rect) return;
    const at = point(event);
    const open = drag.current;
    if (!open || open.pointer !== event.pointerId) {
      setHover(handleAt(toCanvas(box, frame, rect), at.x, at.y));
      return;
    }
    const to = toSource(box, frame, at.x, at.y);
    const next = dragRect(
      open.start,
      open.handle,
      to.x - open.from.x,
      to.y - open.from.y,
      frame,
      keptAspect(open.start, frame, event.shiftKey),
    );
    setDraft(next);
    if (!same(next, draft ?? open.start)) {
      open.sent = true;
      open.gesture.change({ edit: "set-crop", clips: [clip], rect: next });
    }
  };

  const onPointerUp = (event: PointerEvent<HTMLCanvasElement>) => {
    const open = drag.current;
    if (!open || open.pointer !== event.pointerId) return;
    drag.current = null;
    setGrabbed(null);
    void open.gesture.end().then(() => {
      setDraft(null);
    });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLCanvasElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      if (drag.current) void cancelDrag();
      else void leave();
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      void leave();
      return;
    }
    const steps = ARROWS[event.key];
    if (!steps || !frame || !rect || drag.current) return;
    event.preventDefault();
    const scale = event.shiftKey ? 10 : 1;
    const next = nudge(rect, frame, steps[0] * scale, steps[1] * scale);
    if (same(next, rect)) return;
    // A key held down is one entry, like a drag.
    keys.current ??= editGesture("Move crop");
    keys.current.change({ edit: "set-crop", clips: [clip], rect: next });
  };

  const onKeyUp = () => {
    const open = keys.current;
    keys.current = null;
    if (open) void open.end();
  };

  const aspect = frame && shown ? presetOf(shown, frame) : "free";

  return (
    <>
      <canvas
        ref={canvasRef}
        className="player__crop"
        data-testid="crop-overlay"
        tabIndex={0}
        role="application"
        aria-label={
          shown
            ? `Crop ${String(shown.width)} × ${String(shown.height)} from ${String(shown.x)}, ${String(shown.y)}. Drag the handles, or use the arrow keys to move it; Enter or Escape to finish.`
            : "Crop"
        }
        style={{ cursor: CURSORS[grabbed ?? hover ?? "move"] }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={() => void cancelDrag()}
        onKeyDown={onKeyDown}
        onKeyUp={onKeyUp}
      />
      <div className="player__crop-bar" data-testid="crop-bar">
        {frame ? (
          <>
            <span className="player__crop-hint">
              Shift keeps the shape · arrows move it
            </span>
            <Select
              label="Aspect"
              value={aspect}
              onValueChange={(value) => {
                const chosen = ASPECTS.find((option) => option.value === value);
                if (chosen && chosen.value !== "free")
                  void edit({
                    edit: "crop-to-aspect",
                    clips: [clip],
                    aspect: chosen.value,
                  });
              }}
              options={ASPECTS.map((option) => ({
                value: option.value,
                label: option.label,
                disabled: option.value === "free",
              }))}
            />
          </>
        ) : (
          <span className="player__crop-hint" role="status">
            The clip&apos;s source is offline, so its picture cannot be framed.
          </span>
        )}
        <Button size="sm" onClick={() => void leave()}>
          Done
        </Button>
      </div>
    </>
  );
}

const ARROWS: Record<string, readonly [number, number] | undefined> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, -1],
  ArrowDown: [0, 1],
};

function same(a: Rect, b: Rect): boolean {
  return (
    a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
  );
}

interface Colours {
  readonly dim: string;
  readonly line: string;
  readonly handle: string;
}

/** The overlay's colours, from the design tokens: a canvas cannot use
 * `var()`, and no colour is spelled here (the colour gate). */
function readColours(element: Element): Colours {
  const style = getComputedStyle(element);
  const read = (name: string) => style.getPropertyValue(name).trim();
  return {
    dim: read("--scrim"),
    line: read("--focus-ring"),
    handle: read("--text-primary"),
  };
}

/** The picture dimmed outside `shown`, its border, and its handles. */
function draw(
  context: CanvasRenderingContext2D,
  box: Box,
  shown: Box,
  colours: Colours,
): void {
  context.save();
  context.globalAlpha = 0.6;
  context.fillStyle = colours.dim;
  const right = shown.left + shown.width;
  const bottom = shown.top + shown.height;
  context.fillRect(box.left, box.top, box.width, shown.top - box.top);
  context.fillRect(box.left, bottom, box.width, box.top + box.height - bottom);
  context.fillRect(box.left, shown.top, shown.left - box.left, shown.height);
  context.fillRect(
    right,
    shown.top,
    box.left + box.width - right,
    shown.height,
  );
  context.restore();
  context.strokeStyle = colours.line;
  context.lineWidth = 1.5;
  context.strokeRect(shown.left, shown.top, shown.width, shown.height);
  context.fillStyle = colours.handle;
  context.strokeStyle = colours.line;
  for (const handle of HANDLES) {
    const { x, y } = handlePoint(shown, handle);
    context.fillRect(
      x - HANDLE_SIZE,
      y - HANDLE_SIZE,
      HANDLE_SIZE * 2,
      HANDLE_SIZE * 2,
    );
    context.strokeRect(
      x - HANDLE_SIZE,
      y - HANDLE_SIZE,
      HANDLE_SIZE * 2,
      HANDLE_SIZE * 2,
    );
  }
}
