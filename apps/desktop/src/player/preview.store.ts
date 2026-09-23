import type {
  DecodeStats,
  MonitorCommand,
  MonitorLevels,
  MonitorStatus,
  PlaybackStatus,
  PlaybackUpdate,
  PreviewOpened,
  TransportCommand,
} from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/**
 * Renderer state for the preview player.
 *
 * `CLAUDE.md` section 2: the engine owns every media decision. This store
 * holds what the engine said — which session is open, where it is, how it is
 * doing — and sends it what the user asked for. Which frame is due, when to
 * drop one, where a step lands, are all answered in Rust.
 */
export type PreviewStatus = "empty" | "opening" | "open" | "failed";

interface PreviewState {
  status: PreviewStatus;
  session: number | null;
  /** Where the engine last said the player was. */
  playback: PlaybackStatus | null;
  /** The timeline frame number of the frame on screen. */
  frameNumber: number | null;
  /** Where the frame on screen starts on the timeline, in microseconds. */
  framePosition: number | null;
  /** The file being previewed as the user gave it, or the project's name. */
  path: string | null;
  /** A dropped file, or the open project through the evaluator (#30). */
  kind: "file" | "project" | null;
  error: string | null;
  stats: DecodeStats | null;
  /** What the editor hears: volume, mute, solo. Never part of an export. */
  monitoring: MonitorStatus;
  /** The meter, as last read. */
  levels: MonitorLevels | null;
  /** Change monitoring, or put out the clip indication. */
  monitor: (command: MonitorCommand) => Promise<void>;
  refreshLevels: () => Promise<void>;
  /** Open `path` for preview, closing whatever was open. */
  open: (path: string, maxWidth: number, maxHeight: number) => Promise<void>;
  /**
   * Preview the open project through the shared evaluator (#30), closing
   * whatever was open. `name` is what the placeholder says while it opens.
   */
  openProject: (
    name: string,
    maxWidth: number,
    maxHeight: number,
  ) => Promise<void>;
  /** Close the open preview; resolves once its decoders have exited. */
  close: () => Promise<void>;
  /** Send a transport command to the open preview. */
  transport: (command: TransportCommand) => Promise<void>;
  /** A status the engine pushed. */
  applyUpdate: (update: PlaybackUpdate) => void;
  /** The canvas drew a frame. */
  showFrame: (frameNumber: number, position: number) => void;
  refreshStats: () => Promise<void>;
  /** The frame stream stopped working. */
  fail: (message: string) => void;
}

/** Incremented by every open and close, so a slow open that loses a race
 * with a newer one does not install a stale session. */
let generation = 0;

export const usePreviewStore = create<PreviewState>((set, get) => {
  /** Close what is open, then open what `request` opens, under `label`. */
  const start = async (
    label: string,
    kind: "file" | "project",
    request: () => Promise<PreviewOpened>,
  ): Promise<void> => {
    const mine = ++generation;
    await get().close();
    generation = mine;
    set({ status: "opening", path: label, kind, error: null, stats: null });
    try {
      const opened = await request();
      if (mine !== generation) {
        await invoke("close_preview", { session: opened.session });
        return;
      }
      set({
        status: "open",
        session: opened.session,
        playback: opened.status,
        frameNumber: null,
        framePosition: null,
        monitoring: { volume: 1, muted: false, soloed: [], mutedTracks: [] },
        levels: null,
      });
    } catch (error) {
      if (mine !== generation) return;
      set({ status: "failed", session: null, error: String(error) });
    }
  };

  return {
    status: "empty",
    session: null,
    playback: null,
    frameNumber: null,
    framePosition: null,
    path: null,
    kind: null,
    error: null,
    stats: null,
    monitoring: { volume: 1, muted: false, soloed: [], mutedTracks: [] },
    levels: null,

    monitor: async (command) => {
      const { session } = get();
      if (session === null) return;
      try {
        const monitoring = await invoke<MonitorStatus>("monitor", {
          session,
          command,
        });
        if (get().session === session) set({ monitoring });
        if (command.type === "reset-clip") await get().refreshLevels();
      } catch (error) {
        set({ error: String(error) });
      }
    },

    refreshLevels: async () => {
      const { session } = get();
      if (session === null) return;
      try {
        const levels = await invoke<MonitorLevels>("monitor_levels", {
          session,
        });
        if (get().session === session) set({ levels });
      } catch {
        // Closed between the check and the call.
      }
    },

    open: async (path, maxWidth, maxHeight) => {
      await start(path, "file", () =>
        invoke<PreviewOpened>("open_preview", {
          path,
          maxWidth: Math.round(maxWidth),
          maxHeight: Math.round(maxHeight),
        }),
      );
    },

    openProject: async (name, maxWidth, maxHeight) => {
      await start(name, "project", () =>
        invoke<PreviewOpened>("open_project_preview", {
          maxWidth: Math.round(maxWidth),
          maxHeight: Math.round(maxHeight),
        }),
      );
    },
    close: async () => {
      generation += 1;
      const { session } = get();
      set({
        status: "empty",
        session: null,
        playback: null,
        frameNumber: null,
        framePosition: null,
        path: null,
        kind: null,
        stats: null,
      });
      if (session !== null) {
        await invoke("close_preview", { session });
      }
    },

    transport: async (command) => {
      const { session } = get();
      if (session === null) return;
      try {
        const playback = await invoke<PlaybackStatus>("transport", {
          session,
          command,
        });
        if (get().session === session) set({ playback });
      } catch (error) {
        set({ error: String(error) });
      }
    },

    applyUpdate: (update) => {
      if (update.session === get().session) set({ playback: update.status });
    },

    showFrame: (frameNumber, framePosition) => {
      if (get().framePosition !== framePosition) {
        set({ frameNumber, framePosition });
      }
    },

    refreshStats: async () => {
      const { session } = get();
      if (session === null) return;
      try {
        const stats = await invoke<DecodeStats>("preview_stats", { session });
        if (get().session === session) set({ stats });
      } catch {
        // The session closed between the check and the call; the next open
        // resets the display anyway.
      }
    },

    fail: (message) => {
      set({ status: "failed", error: message });
    },
  };
});
