import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/**
 * Renderer state for the application shell.
 *
 * `CLAUDE.md` section 3: Zustand in the renderer, no state library in the
 * engine. And section 2: the renderer may display what the engine decided — it
 * must never compute a tier itself.
 */
export type EngineStatus = "not-connected" | "connecting" | "ready" | "failed";

interface ShellState {
  engineStatus: EngineStatus;
  /** Why the engine is unavailable, in the engine's words. */
  engineError: string | null;
  setEngineStatus: (status: EngineStatus) => void;
}

export const useShellStore = create<ShellState>((set) => ({
  engineStatus: "not-connected",
  engineError: null,
  setEngineStatus: (engineStatus) => {
    set({ engineStatus });
  },
}));

/** How often the encoder probe is asked about while it is still running. */
export const ENGINE_POLL_MS = 500;

/**
 * Follow the engine from launch until it is ready or cannot be (#151).
 *
 * `encoder_capabilities` is the engine's own answer: an error when the FFmpeg
 * sidecar is missing, `null` while the encoder probe still runs, the profile
 * once it is done. Nothing here guesses; a status is only ever what the engine
 * said. Returns what stops asking.
 */
export function followEngine(): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const { setState } = useShellStore;
  setState({ engineStatus: "connecting", engineError: null });
  const ask = async (): Promise<void> => {
    try {
      const profile = await invoke<unknown>("encoder_capabilities");
      if (stopped) return;
      if (profile === null || profile === undefined) {
        timer = setTimeout(() => void ask(), ENGINE_POLL_MS);
        return;
      }
      setState({ engineStatus: "ready", engineError: null });
    } catch (cause) {
      if (stopped) return;
      setState({
        engineStatus: "failed",
        engineError: cause instanceof Error ? cause.message : String(cause),
      });
    }
  };
  void ask();
  return () => {
    stopped = true;
    clearTimeout(timer);
  };
}

/** What the player says about the engine: words, never the raw state. */
export function engineLabel(
  status: EngineStatus,
  error: string | null,
): string {
  switch (status) {
    case "not-connected":
      return "Engine: not started";
    case "connecting":
      return "Engine: starting…";
    case "ready":
      return "Engine: ready";
    case "failed":
      return error ? `Engine: unavailable (${error})` : "Engine: unavailable";
  }
}
