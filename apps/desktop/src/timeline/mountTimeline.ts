import type { ProjectView, WaveformUpdate } from "@blinkify/types";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { usePreviewStore } from "../player/preview.store.js";
import {
  fileName,
  useProjectStore,
  type DeepReadonly,
} from "../project/project.store.js";
import {
  drawContent,
  drawOverlay,
  TextCache,
  type Scene,
  type TrackRow,
} from "./draw.js";
import { layoutRows, RULER_HEIGHT } from "./rows.js";
import { RedrawScheduler, type FrameSource } from "./scheduler.js";
import { readTheme } from "./theme.js";
import { TimelineMedia } from "./timelineMedia.js";
import { useTimelineStore } from "./timeline.store.js";

/** The engine's waveform event: see `media::WAVEFORM_EVENT` in the shell. */
const WAVEFORM_EVENT = "media://waveform";

function loadImage(url: string): Promise<CanvasImageSource> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`could not load ${url}`));
    image.src = url;
  });
}

/**
 * The video clips whose source the model says cannot be stream-copied into
 * the sequence (#57) — the ones the timeline marks. The model's answer per
 * source, applied to its clips; nothing here decides eligibility.
 */
export function ineligibleClips(
  view: DeepReadonly<ProjectView> | null,
  rows: readonly TrackRow[],
): Set<number> {
  const eligibility = view?.eligibility ?? {};
  return new Set(
    rows
      .filter((row) => row.kind === "video")
      .flatMap((row) => row.placements)
      .filter((p) => eligibility[p.source]?.eligible === false)
      .map((p) => p.clip),
  );
}

export interface MountedTimeline {
  readonly scheduler: RedrawScheduler;
  dispose: () => void;
}

/**
 * Connect the timeline's two canvases to the stores (#33).
 *
 * Nothing here draws in response to a React render. The stores are
 * subscribed to directly, a change marks a layer dirty, and the scheduler
 * draws the dirty layers on the next animation frame. The project's graph,
 * the selection, the zoom and the scroll dirty the content layer; the
 * playhead dirties only the overlay, so playback never repaints a clip.
 */
export function mountTimeline(
  element: HTMLElement,
  contentElement: HTMLCanvasElement,
  overlayElement: HTMLCanvasElement,
  frames?: FrameSource,
): MountedTimeline {
  const content = contentElement.getContext("2d");
  const overlay = overlayElement.getContext("2d");

  let dpr = window.devicePixelRatio || 1;
  let theme = readTheme();
  const text = new TextCache();
  let rows: TrackRow[] = [];
  let labels = new Map<number, string>();
  let ineligible = new Set<number>();

  const scene = (): Scene => {
    const { view, selection } = {
      view: useTimelineStore.getState().view,
      selection: useProjectStore.getState().selection,
    };
    const project = useProjectStore.getState().view;
    return {
      view,
      dpr,
      rulerHeight: RULER_HEIGHT,
      rows,
      frameRate: project?.project.sequence.settings.frameRate ?? {
        num: 30,
        den: 1,
      },
      selection: new Set(selection),
      ineligible,
      labels,
      theme,
      media,
      text,
    };
  };

  const scheduler = new RedrawScheduler((layers) => {
    if (layers.has("content") && content) drawContent(content, scene());
    if (layers.has("overlay") && overlay)
      drawOverlay(
        overlay,
        scene(),
        useTimelineStore.getState().playhead,
        useTimelineStore.getState().drag,
      );
  }, frames);

  const media = new TimelineMedia(
    {
      invoke: (command, args) => invoke(command, args),
      loadImage,
      sheetUrl: (path) =>
        `${convertFileSrc("", "frame")}sheet/${encodeURIComponent(path)}`,
    },
    () => scheduler.invalidate("content"),
  );

  const fromProject = (): void => {
    const view = useProjectStore.getState().view;
    rows = layoutRows(view?.timeline);
    ineligible = ineligibleClips(view, rows);
    const sources = view?.project.sources ?? {};
    labels = new Map(
      Object.entries(sources).map(([id, source]) => [
        Number(id),
        fileName(source?.path ?? ""),
      ]),
    );
    media.setSources(
      new Map(
        Object.entries(sources).map(([id, source]) => [
          Number(id),
          source?.path ?? "",
        ]),
      ),
    );
    const timeline = useTimelineStore.getState();
    if (!timeline.fitted && view?.timeline) timeline.fitAll();
    scheduler.invalidate("content");
  };

  const resize = (): void => {
    dpr = window.devicePixelRatio || 1;
    const width = element.clientWidth;
    const height = element.clientHeight;
    for (const canvas of [contentElement, overlayElement]) {
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    useTimelineStore.getState().setSize(width, height);
    scheduler.invalidate("content");
    scheduler.invalidate("overlay");
  };

  const unsubscribers = [
    useProjectStore.subscribe((state, previous) => {
      if (state.view !== previous.view) fromProject();
      else if (state.selection !== previous.selection)
        scheduler.invalidate("content");
    }),
    useTimelineStore.subscribe((state, previous) => {
      if (state.view !== previous.view) {
        scheduler.invalidate("content");
        scheduler.invalidate("overlay");
      } else if (
        state.playhead !== previous.playhead ||
        state.drag !== previous.drag
      ) {
        // A drag is drawn on the overlay: the clips under it are not
        // repainted while it moves.
        scheduler.invalidate("overlay");
      }
    }),
    usePreviewStore.subscribe((state) => {
      useTimelineStore
        .getState()
        .setPlayhead(state.kind === "project" ? state.frameNumber : null);
    }),
  ];

  const observer = new ResizeObserver(resize);
  observer.observe(element);
  // The display scaling changed — the window moved to another monitor.
  const scaling = window.matchMedia?.(`(resolution: ${dpr}dppx)`);
  const rescale = (): void => {
    theme = readTheme();
    resize();
  };
  scaling?.addEventListener?.("change", rescale);

  let stopWaveforms: (() => void) | null = null;
  let closed = false;
  void listen<WaveformUpdate>(WAVEFORM_EVENT, (event) =>
    media.waveformUpdate(event.payload),
  )
    .then((stop) => {
      if (closed) stop();
      else stopWaveforms = stop;
    })
    .catch(() => undefined);

  resize();
  fromProject();

  const dispose = (): void => {
    closed = true;
    stopWaveforms?.();
    for (const stop of unsubscribers) stop();
    observer.disconnect();
    scaling?.removeEventListener?.("change", rescale);
    scheduler.dispose();
    media.dispose();
  };
  return { scheduler, dispose };
}
