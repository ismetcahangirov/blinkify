import type { PlaybackUpdate } from "@blinkify/types";
import { Button } from "@blinkify/ui";
import { listen } from "@tauri-apps/api/event";
import { memo, useCallback, useEffect, useRef, useState } from "react";

import { DecodeStatsOverlay } from "../../player/DecodeStatsOverlay.js";
import { MonitorControls } from "../../player/MonitorControls.js";
import { OperationsPanel } from "../../player/OperationsPanel.js";
import { PreviewCanvas } from "../../player/PreviewCanvas.js";
import { usePreviewStore } from "../../player/preview.store.js";
import { ScrubBar } from "../../player/ScrubBar.js";
import { TransportBar } from "../../player/TransportBar.js";
import { useFileDrop } from "../../player/useFileDrop.js";
import {
  framesPerSecond,
  useTransportShortcuts,
} from "../../player/useTransportShortcuts.js";
import { projectName, useProjectStore } from "../../project/project.store.js";
import { useShellStore } from "../../shell.store.js";

/** The engine's transport event: see `media::PLAYBACK_EVENT` in the shell. */
const PLAYBACK_EVENT = "media://playback";

/**
 * The preview player.
 *
 * #27 filled it with the video surface: drop a file on it and the engine
 * decodes it into raw frames this zone draws. #28 added the transport — play,
 * pause, stop, frame steps, jumps, speed, loop, and the keys that drive them.
 * #29 added the playhead to drag, the indication that a seek is still on its
 * way, and the badge that says the picture comes from a proxy. #31 added
 * monitoring: mute, the monitor volume and the level meter. #30 plays the
 * open project through the shared evaluator — the preview of exactly what
 * the export will make — with the operations under the playhead on demand. A
 * dropped file still previews on its own.
 *
 * `memo` is not an optimisation guess here — it is the boundary the shell asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". The preview's own state lives in its own store,
 * and a re-render caused by the inspector must not reach the video surface.
 *
 * It is also the zone that reports the engine's status, because the engine is
 * what the player needs before it can show anything, and a preview that is
 * blank because nothing is loaded should not look like a preview that is blank
 * because the engine never started.
 */
export const PlayerZone = memo(function PlayerZone() {
  const engineStatus = useShellStore((state) => state.engineStatus);
  const status = usePreviewStore((state) => state.status);
  const session = usePreviewStore((state) => state.session);
  const path = usePreviewStore((state) => state.path);
  const error = usePreviewStore((state) => state.error);
  const open = usePreviewStore((state) => state.open);
  const transport = usePreviewStore((state) => state.transport);
  const resolving = usePreviewStore(
    (state) => state.playback?.resolving ?? false,
  );
  const proxy = usePreviewStore((state) => state.playback?.proxy ?? false);
  const kind = usePreviewStore((state) => state.kind);
  const openProject = usePreviewStore((state) => state.openProject);
  const project = useProjectStore((state) => state.view);
  const [showStats, setShowStats] = useState(false);
  const [showOperations, setShowOperations] = useState(false);
  const surface = useRef<HTMLDivElement>(null);

  const openDropped = useCallback(
    (dropped: string) => {
      const element = surface.current;
      const ratio = window.devicePixelRatio || 1;
      // Decode at the size the surface can show, not the source's: preview
      // quality is deliberately decoupled from export quality.
      void open(
        dropped,
        (element?.clientWidth ?? 0) * ratio,
        (element?.clientHeight ?? 0) * ratio,
      );
    },
    [open],
  );
  useFileDrop(surface, openDropped);

  // A project Blinkify was opened with plays as soon as it is known.
  const projectPath = project?.path ?? null;
  useEffect(() => {
    if (projectPath === null) return;
    const element = surface.current;
    const ratio = window.devicePixelRatio || 1;
    void openProject(
      projectName(useProjectStore.getState().view),
      (element?.clientWidth ?? 0) * ratio,
      (element?.clientHeight ?? 0) * ratio,
    );
  }, [projectPath, openProject]);

  const sendTransport = useCallback(
    (command: Parameters<typeof transport>[0]) => {
      void transport(command);
    },
    [transport],
  );
  const keyContext = useCallback(() => {
    const playback = usePreviewStore.getState().playback;
    return {
      playing: playback?.state === "playing",
      secondInFrames: playback ? framesPerSecond(playback.frameRate) : 30,
    };
  }, []);
  useTransportShortcuts(session !== null, sendTransport, keyContext);

  // What the engine changes on its own — reaching the end, a new audio
  // device — arrives as an event rather than as an answer.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    let listening: Promise<() => void>;
    try {
      listening = listen<PlaybackUpdate>(PLAYBACK_EVENT, (event) => {
        usePreviewStore.getState().applyUpdate(event.payload);
      });
    } catch {
      // Outside the application window there is no engine to hear from.
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
  }, []);

  return (
    <div className="zone player">
      <div className="player__header">
        <h2 className="zone__title">Player</h2>
        {/* #26: whenever the picture comes from a proxy, say so — a proxy
            is a 540-line copy, and nobody should judge detail on it
            believing it is the file. */}
        {proxy && (
          <span className="player__badge" data-testid="proxy-badge">
            Proxy
          </span>
        )}
        {session !== null && (
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={showStats}
            onClick={() => {
              setShowStats((shown) => !shown);
            }}
          >
            Stats
          </Button>
        )}
        {session !== null && kind === "project" && (
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={showOperations}
            onClick={() => {
              setShowOperations((shown) => !shown);
            }}
          >
            Operations
          </Button>
        )}
      </div>
      <div
        ref={surface}
        className="player__surface"
        data-testid="player-surface"
      >
        {session !== null ? (
          <PreviewCanvas key={session} session={session} />
        ) : (
          <p className="zone__placeholder">
            {status === "opening"
              ? `Opening ${path ?? ""}…`
              : "Drop a video file here to preview it."}
          </p>
        )}
        {session !== null && showStats && <DecodeStatsOverlay />}
        {session !== null && kind === "project" && showOperations && (
          <OperationsPanel session={session} />
        )}
        {resolving && (
          <p
            className="player__resolving"
            role="status"
            data-testid="resolving"
          >
            Finding the frame…
          </p>
        )}
      </div>
      {session !== null && <ScrubBar />}
      {session !== null && <TransportBar />}
      {session !== null && <MonitorControls />}
      {status === "failed" && error !== null && (
        <p className="player__error" role="alert">
          {error}
        </p>
      )}
      {/* The shell must never imply a capability it does not have. An engine
          status that defaulted to "ready" would be the same class of claim as
          an export reporting lossless without having checked. */}
      <p className="zone__placeholder" data-testid="engine-status">
        Engine: {engineStatus}
      </p>
    </div>
  );
});
