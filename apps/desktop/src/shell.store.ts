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
  setEngineStatus: (status: EngineStatus) => void;
}

export const useShellStore = create<ShellState>((set) => ({
  engineStatus: "not-connected",
  setEngineStatus: (engineStatus) => {
    set({ engineStatus });
  },
}));
