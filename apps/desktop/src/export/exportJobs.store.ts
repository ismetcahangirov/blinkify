import type { ExportJob } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

/**
 * The export queue as the renderer follows it (#51).
 *
 * The engine owns the queue: its order, its progress and what a crash left
 * behind. This store holds the last word the engine sent about each job —
 * the list once when it starts, then every change as an event — and asks the
 * engine to cancel, restart or discard. It never moves a job on by itself.
 */

/** Every change of every export job; `export::JOB_EVENT` in the shell. */
export const JOB_EVENT = "export://job";

interface ExportJobsState {
  /** The queue in order, then the history. */
  jobs: readonly ExportJob[];
  /** Why the last request was refused. */
  error: string | null;
  load: () => Promise<void>;
  /** A job as the engine last described it. */
  receive: (job: ExportJob) => void;
  cancel: (id: number) => Promise<void>;
  resume: (id: number) => Promise<void>;
  discard: (id: number) => Promise<void>;
  clearHistory: () => Promise<void>;
}

/** `jobs` with `job` in its place: replaced where it is, else added last. */
export function upsert(
  jobs: readonly ExportJob[],
  job: ExportJob,
): readonly ExportJob[] {
  const at = jobs.findIndex((existing) => existing.id === job.id);
  if (at === -1) return [...jobs, job];
  return jobs.map((existing, index) => (index === at ? job : existing));
}

export const useExportJobs = create<ExportJobsState>((set, get) => {
  const ask = async (request: () => Promise<unknown>) => {
    try {
      await request();
      set({ error: null });
    } catch (cause) {
      set({ error: String(cause) });
    }
  };
  return {
    jobs: [],
    error: null,
    load: async () => {
      try {
        set({ jobs: await invoke<ExportJob[]>("export_jobs"), error: null });
      } catch (cause) {
        set({ error: String(cause) });
      }
    },
    receive: (job) => set({ jobs: upsert(get().jobs, job) }),
    cancel: (id) => ask(() => invoke("cancel_export", { id })),
    // A restarted job moves behind the ones waiting, so the engine's list
    // is read again rather than guessed.
    resume: (id) =>
      ask(async () => {
        await invoke("resume_export", { id });
        await get().load();
      }),
    discard: (id) => ask(() => invoke("discard_export", { id })),
    clearHistory: () =>
      ask(async () => {
        await invoke("clear_export_history");
        await get().load();
      }),
  };
});

/**
 * Follow the engine's queue: read it, then take every change. Returns the
 * way to stop.
 */
export function followExportJobs(): () => void {
  let stop: (() => void) | null = null;
  let stopped = false;
  void listen<ExportJob>(JOB_EVENT, (event) =>
    useExportJobs.getState().receive(event.payload),
  )
    .then((unlisten) => {
      if (stopped) unlisten();
      else stop = unlisten;
    })
    .catch(() => undefined);
  void useExportJobs.getState().load();
  return () => {
    stopped = true;
    stop?.();
  };
}
