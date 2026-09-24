import type {
  Edit,
  EditContext,
  EditOutcome,
  ProjectView,
  RecentProject,
  RecoveryOffer,
  SequenceSettings,
} from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { usePreviewStore } from "../player/preview.store.js";

/**
 * The open project, as the shell reports it (#32), and the one way the
 * renderer changes it: an edit (#37).
 *
 * The renderer shows the project and never reads the file itself: opening,
 * checking sources and relinking are the engine's. It never changes its copy
 * of the graph either. Every change is an `Edit` sent to the engine, which
 * applies it through the document's history and answers with the new view —
 * so every change is undoable, and no second copy of the graph can drift from
 * the one the preview and the export read. `view` is typed read-only to make
 * the point, and `pnpm edits:check` keeps every other renderer file from
 * sending an edit around this store.
 *
 * A source that is missing or changed does not stop the project opening — it
 * is listed in `view.unavailable`, its clips in `view.affectedClips`, and the
 * user is offered a relink.
 */
interface ProjectState {
  view: DeepReadonly<ProjectView> | null;
  /** Why the launch project could not be opened, in the shell's words. */
  error: string | null;
  /** Why the last relink was refused. */
  relinkError: string | null;
  /** Why the last edit was refused. */
  editError: string | null;
  /** Whether the last edit met a bound — the end of a source, the next
   * clip — and did less than was asked. */
  lastClamped: boolean;
  /** The selected clips. Travels with every edit, and comes back on undo. */
  selection: readonly number[];
  select: (clips: readonly number[]) => void;
  /** Open the project Blinkify was started with, if any. */
  loadLaunch: () => Promise<void>;
  /**
   * The launch (#54): the project Blinkify was started with; failing that,
   * unsaved work to offer back; failing that, a new untitled project, so
   * there is always somewhere to import into.
   */
  start: () => Promise<void>;
  /** Recently opened projects, most recent first. */
  recent: readonly RecentProject[];
  loadRecent: () => Promise<void>;
  /** Unsaved work from a session that ended uncleanly, offered back. */
  recovery: readonly RecoveryOffer[];
  restore: (offer: RecoveryOffer) => Promise<void>;
  discardRecovery: (offer: RecoveryOffer) => Promise<void>;
  /** A new project; `settings` null matches the first clip (#57). */
  newProject: (
    name: string,
    settings: SequenceSettings | null,
  ) => Promise<void>;
  openProject: (path: string) => Promise<void>;
  /** Save to the project file. Resolves with the engine's refusal, or
   * "untitled" when it has none yet — the caller then asks where. */
  save: () => Promise<string | null>;
  saveAs: (path: string) => Promise<string | null>;
  closeProject: (discard: boolean) => Promise<void>;
  autosave: () => Promise<void>;
  /** Why the last lifecycle command failed, in the engine's words. */
  lifecycleError: string | null;
  relink: (source: number, path: string) => Promise<void>;
  /**
   * Apply one edit. Resolves with the engine's refusal, if it refused; the
   * graph is then unchanged.
   */
  edit: (edit: Edit) => Promise<string | null>;
  undo: () => Promise<void>;
  redo: () => Promise<void>;
  /**
   * Bracket a gesture — a slider being dragged — so its edits are one
   * history entry. Explicit, never a timer.
   */
  beginGesture: (label: string) => Promise<void>;
  endGesture: () => Promise<void>;
}

/** A value no renderer code can assign into, however deep. */
export type DeepReadonly<T> = T extends (infer E)[]
  ? readonly DeepReadonly<E>[]
  : T extends object
    ? { readonly [K in keyof T]: DeepReadonly<T[K]> }
    : T;

const message = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause);

/** Microseconds per sequence frame at `frameRate`. */
function frameMicroseconds(frameRate: { num: number; den: number }): number {
  return (frameRate.den * 1_000_000) / frameRate.num;
}

export const useProjectStore = create<ProjectState>((set, get) => {
  /** Where the playhead is, in sequence frames: the project preview's. */
  const playhead = (): number => {
    const preview = usePreviewStore.getState();
    return preview.kind === "project" ? (preview.frameNumber ?? 0) : 0;
  };

  /** Run a lifecycle command; a refusal is kept to show, not thrown. */
  const command = async <T>(
    name: string,
    args?: Record<string, unknown>,
  ): Promise<T | null> => {
    try {
      const result = await invoke<T>(name, args);
      set({ lifecycleError: null });
      return result;
    } catch (cause) {
      set({ lifecycleError: message(cause) });
      return null;
    }
  };

  const context = (): EditContext => ({
    selection: [...get().selection],
    playhead: playhead(),
  });

  /** Show what an edit, undo or redo returned: the graph, the selection,
   * and — if it moved — the playhead. */
  const settle = (outcome: EditOutcome): void => {
    set({
      view: outcome.view,
      selection: outcome.context.selection,
      editError: null,
      lastClamped: outcome.clamped,
    });
    const rate = outcome.view.project.sequence.settings.frameRate;
    const preview = usePreviewStore.getState();
    if (
      preview.kind === "project" &&
      outcome.context.playhead !== playhead() &&
      rate.num > 0
    ) {
      void preview.transport({
        type: "seek",
        position: Math.round(
          outcome.context.playhead * frameMicroseconds(rate),
        ),
      });
    }
  };

  return {
    view: null,
    error: null,
    relinkError: null,
    editError: null,
    lastClamped: false,
    selection: [],
    recent: [],
    recovery: [],
    lifecycleError: null,

    select: (clips) => set({ selection: [...clips] }),

    loadLaunch: async () => {
      try {
        const view = await invoke<ProjectView | null>("launch_project");
        set({ view, error: null, selection: [] });
      } catch (cause) {
        set({ view: null, error: message(cause) });
      }
    },

    start: async () => {
      await get().loadLaunch();
      if (get().view || get().error) return;
      try {
        const offers = (await invoke<RecoveryOffer[]>("recovery_offers")) ?? [];
        if (offers.length > 0) {
          set({ recovery: offers });
          return;
        }
      } catch {
        // No recovery folder yet: nothing to offer.
      }
      await get().newProject("Untitled project", null);
    },

    loadRecent: async () => {
      try {
        set({
          recent: (await invoke<RecentProject[]>("recent_projects")) ?? [],
        });
      } catch {
        set({ recent: [] });
      }
    },

    restore: async (offer) => {
      const view = await command<ProjectView>("restore_recovery", { offer });
      if (view) {
        set({ view, selection: [] });
        set({ recovery: get().recovery.filter((o) => o !== offer) });
      }
    },

    discardRecovery: async (offer) => {
      await command("discard_recovery", { offer });
      const recovery = get().recovery.filter((o) => o !== offer);
      set({ recovery });
      if (recovery.length === 0 && !get().view)
        await get().newProject("Untitled project", null);
    },

    newProject: async (name, settings) => {
      const view = await command<ProjectView>("new_project", {
        name,
        settings,
      });
      if (view) set({ view, selection: [] });
    },

    openProject: async (path) => {
      const view = await command<ProjectView>("open_project", { path });
      if (view) {
        set({ view, selection: [] });
        await get().loadRecent();
      }
    },

    save: async () => {
      try {
        const view = await invoke<ProjectView>("save_project");
        set({ view, lifecycleError: null });
        await get().loadRecent();
        return null;
      } catch (cause) {
        const refusal = message(cause);
        if (refusal !== "untitled") set({ lifecycleError: refusal });
        return refusal;
      }
    },

    saveAs: async (path) => {
      try {
        const view = await invoke<ProjectView>("save_project_as", { path });
        set({ view, lifecycleError: null });
        await get().loadRecent();
        return null;
      } catch (cause) {
        const refusal = message(cause);
        set({ lifecycleError: refusal });
        return refusal;
      }
    },

    closeProject: async (discard) => {
      await command("close_project", { discard });
      await usePreviewStore.getState().close();
      set({ view: null, selection: [] });
    },

    autosave: async () => {
      if (!get().view) return;
      await command<boolean>("autosave_project");
    },

    edit: async (edit) => {
      try {
        const outcome = await invoke<EditOutcome>("edit_project", {
          edit,
          context: context(),
        });
        settle(outcome);
        return null;
      } catch (cause) {
        const refusal = message(cause);
        set({ editError: refusal });
        return refusal;
      }
    },

    undo: async () => {
      if (!get().view) return;
      try {
        const outcome = await invoke<EditOutcome | null>("undo_edit");
        if (outcome) settle(outcome);
      } catch (cause) {
        set({ editError: message(cause) });
      }
    },

    redo: async () => {
      if (!get().view) return;
      try {
        const outcome = await invoke<EditOutcome | null>("redo_edit");
        if (outcome) settle(outcome);
      } catch (cause) {
        set({ editError: message(cause) });
      }
    },

    beginGesture: async (label) => {
      if (!get().view) return;
      await invoke("begin_gesture", { label, context: context() });
    },

    endGesture: async () => {
      if (!get().view) return;
      const view = await invoke<ProjectView>("end_gesture");
      set({ view });
    },

    relink: async (source, path) => {
      try {
        const view = await invoke<ProjectView>("relink_source", {
          source,
          path,
          context: context(),
        });
        set({ view, relinkError: null });
      } catch (cause) {
        set({ relinkError: message(cause) });
      }
    },
  };
});

/** The name to show for the open project, marked when it has unsaved work. */
export function projectTitle(
  view: DeepReadonly<ProjectView> | null | undefined,
): string {
  const name = projectName(view);
  return view?.dirty ? `${name} •` : name;
}

/** The name to show for the open project. */
export function projectName(
  view: DeepReadonly<ProjectView> | null | undefined,
): string {
  if (!view) return "Untitled project";
  return view.project.name.trim() || "Untitled project";
}

/** The file name at the end of a Windows or POSIX path. */
export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
