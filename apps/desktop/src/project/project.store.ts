import type { Project, ProjectView } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/**
 * The open project, as the shell reports it (#32).
 *
 * The renderer shows the project and never reads the file itself: opening,
 * checking sources and relinking are the engine's, behind two commands. A
 * source that is missing or changed does not stop the project opening — it
 * is listed in `view.unavailable`, its clips in `view.affectedClips`, and the
 * user is offered a relink.
 */
interface ProjectState {
  view: ProjectView | null;
  /** Why the launch project could not be opened, in the shell's words. */
  error: string | null;
  /** Why the last relink was refused. */
  relinkError: string | null;
  /** Open the project Blinkify was started with, if any. */
  loadLaunch: () => Promise<void>;
  relink: (source: number, path: string) => Promise<void>;
  /**
   * Replace the graph — how the timeline's edits (#33 onward) reach the
   * engine. The preview follows at once; nothing is saved (#54). Resolves
   * with the engine's refusal, if it refused.
   */
  update: (project: Project) => Promise<string | null>;
}

const message = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause);

export const useProjectStore = create<ProjectState>((set) => ({
  view: null,
  error: null,
  relinkError: null,

  loadLaunch: async () => {
    try {
      const view = await invoke<ProjectView | null>("launch_project");
      set({ view, error: null });
    } catch (cause) {
      set({ view: null, error: message(cause) });
    }
  },

  update: async (project) => {
    try {
      const view = await invoke<ProjectView>("update_project", { project });
      set({ view });
      return null;
    } catch (cause) {
      return message(cause);
    }
  },

  relink: async (source, path) => {
    try {
      const view = await invoke<ProjectView>("relink_source", { source, path });
      set({ view, relinkError: null });
    } catch (cause) {
      set({ relinkError: message(cause) });
    }
  },
}));

/** The name to show for the open project. */
export function projectName(view: ProjectView | null): string {
  if (!view) return "Untitled project";
  return view.project.name.trim() || "Untitled project";
}

/** The file name at the end of a Windows or POSIX path. */
export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
