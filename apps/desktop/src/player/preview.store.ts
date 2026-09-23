import type {
  DecodeStats,
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
  /** The file being previewed, as the user gave it. */
  path: string | null;
  error: string | null;
  stats: DecodeStats | null;
  /** Open `path` for preview, closing whatever was open. */
  open: (path: string, maxWidth: number, maxHeight: number) => Promise<void>;
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

export const usePreviewStore = create<PreviewState>((set, get) => ({
  status: "empty",
  session: null,
  playback: null,
  frameNumber: null,
  framePosition: null,
  path: null,
  error: null,
  stats: null,

  open: async (path, maxWidth, maxHeight) => {
    const mine = ++generation;
    await get().close();
    generation = mine;
    set({ status: "opening", path, error: null, stats: null });
    try {
      const opened = await invoke<PreviewOpened>("open_preview", {
        path,
        maxWidth: Math.round(maxWidth),
        maxHeight: Math.round(maxHeight),
      });
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
      });
    } catch (error) {
      if (mine !== generation) return;
      set({ status: "failed", session: null, error: String(error) });
    }
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
}));
