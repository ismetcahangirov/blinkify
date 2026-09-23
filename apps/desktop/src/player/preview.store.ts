import type { DecodeStats, PreviewOpened } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/**
 * Renderer state for the preview player.
 *
 * `CLAUDE.md` section 2: the engine owns every media decision. This store
 * holds what the engine said — which session is open, how it is doing — and
 * nothing it could have computed itself: which frame is due, when to drop one,
 * where to seek, are all answered in Rust.
 */
export type PreviewStatus = "empty" | "opening" | "playing" | "failed";

interface PreviewState {
  status: PreviewStatus;
  preview: PreviewOpened | null;
  /** The file being previewed, as the user gave it. */
  path: string | null;
  error: string | null;
  stats: DecodeStats | null;
  /** Open `path` for preview, closing whatever was open. */
  open: (path: string, maxWidth: number, maxHeight: number) => Promise<void>;
  /** Close the open preview; resolves once its decoder has exited. */
  close: () => Promise<void>;
  refreshStats: () => Promise<void>;
  /** The frame stream stopped working. */
  fail: (message: string) => void;
}

/** Incremented by every open and close, so a slow open that loses a race
 * with a newer one does not install a stale session. */
let generation = 0;

export const usePreviewStore = create<PreviewState>((set, get) => ({
  status: "empty",
  preview: null,
  path: null,
  error: null,
  stats: null,

  open: async (path, maxWidth, maxHeight) => {
    const mine = ++generation;
    await get().close();
    generation = mine;
    set({ status: "opening", path, error: null, stats: null });
    try {
      const preview = await invoke<PreviewOpened>("open_preview", {
        path,
        maxWidth: Math.round(maxWidth),
        maxHeight: Math.round(maxHeight),
      });
      if (mine !== generation) {
        await invoke("close_preview", { session: preview.session });
        return;
      }
      set({ status: "playing", preview });
    } catch (error) {
      if (mine !== generation) return;
      set({ status: "failed", preview: null, error: String(error) });
    }
  },

  close: async () => {
    generation += 1;
    const { preview } = get();
    set({ status: "empty", preview: null, path: null, stats: null });
    if (preview) {
      await invoke("close_preview", { session: preview.session });
    }
  },

  refreshStats: async () => {
    const { preview } = get();
    if (!preview) return;
    try {
      const stats = await invoke<DecodeStats>("preview_stats", {
        session: preview.session,
      });
      if (get().preview?.session === preview.session) set({ stats });
    } catch {
      // The session closed between the check and the call; the next open
      // resets the display anyway.
    }
  },

  fail: (message) => {
    set({ status: "failed", error: message });
  },
}));
