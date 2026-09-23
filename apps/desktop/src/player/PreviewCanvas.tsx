import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";

import { frameUrl, startFramePump } from "./framePump.js";
import { drawFrame, placeFrame } from "./orientation.js";
import { usePreviewStore } from "./preview.store.js";

interface PreviewCanvasProps {
  session: number;
}

/**
 * The video surface.
 *
 * Frames arrive as raw RGBA from the engine's `frame` scheme (#27), are put
 * into an offscreen canvas at their own size with `putImageData` — no decode,
 * no conversion, no copy — and drawn onto the visible canvas rotated and
 * fitted. Each frame carries its own rotation, because a timeline can cut from
 * a landscape clip to a portrait one. A frame in a gap of the timeline is
 * black: the canvas is cleared to the surface behind it.
 *
 * Every frame drawn reports its timeline frame number, so the timecode shown
 * is the timecode of the picture shown (#28).
 */
export function PreviewCanvas({ session }: PreviewCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    const staging = document.createElement("canvas");
    const stagingContext = staging.getContext("2d");
    if (!canvas || !context || !stagingContext) return undefined;

    const base = convertFileSrc("", "frame");
    return startFramePump({
      url: (after) => frameUrl(base, session, after),
      fetch: (url, init) => fetch(url, init),
      onFrame: (frame) => {
        const ratio = window.devicePixelRatio || 1;
        const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
        const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
        if (canvas.width !== width || canvas.height !== height) {
          canvas.width = width;
          canvas.height = height;
        }
        usePreviewStore.getState().showFrame(frame.frameNumber);
        if (frame.black) {
          context.clearRect(0, 0, width, height);
          return;
        }
        if (staging.width !== frame.width || staging.height !== frame.height) {
          staging.width = frame.width;
          staging.height = frame.height;
        }
        stagingContext.putImageData(
          new ImageData(frame.pixels, frame.width, frame.height),
          0,
          0,
        );
        drawFrame(
          context,
          staging,
          placeFrame(frame.width, frame.height, frame.rotation, width, height),
        );
      },
      onError: (error) => {
        usePreviewStore.getState().fail(error.message);
      },
    });
  }, [session]);

  return (
    <canvas
      ref={canvasRef}
      className="player__canvas"
      data-testid="preview-canvas"
      aria-label="Video preview"
    />
  );
}
