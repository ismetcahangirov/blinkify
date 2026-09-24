import type { ImportProgress, Refusal } from "@blinkify/types";
import { create } from "zustand";
import { useProjectStore } from "../project/project.store.js";
import type { AssetKind } from "./assets.js";

/**
 * The library panel's own state (#53): how it is browsed, and the import in
 * progress. Renderer state only — what is in the library is the project's
 * sources, in the project store.
 */

export type Density = "grid" | "list";

export interface Importing {
  readonly done: number;
  readonly total: number;
  /** The file most recently finished. */
  readonly current: string | null;
}

interface LibraryState {
  query: string;
  kind: AssetKind | "all";
  density: Density;
  setQuery: (query: string) => void;
  setKind: (kind: AssetKind | "all") => void;
  setDensity: (density: Density) => void;
  /** An import in progress, file by file, from the engine's events. */
  importing: Importing | null;
  /** What the last import refused, and why, until dismissed. */
  refused: readonly Refusal[];
  progress: (event: ImportProgress) => void;
  dismissRefused: () => void;
  /** Import `paths`: the grid fills as the engine reports each file. */
  importPaths: (paths: readonly string[]) => Promise<void>;
}

export const useLibraryStore = create<LibraryState>((set) => ({
  query: "",
  kind: "all",
  density: "grid",
  importing: null,
  refused: [],
  setQuery: (query) => set({ query }),
  setKind: (kind) => set({ kind }),
  setDensity: (density) => set({ density }),
  progress: (event) =>
    set({
      importing:
        event.done >= event.total
          ? null
          : { done: event.done, total: event.total, current: event.path },
    }),
  dismissRefused: () => set({ refused: [] }),
  importPaths: async (paths) => {
    if (paths.length === 0) return;
    set({ importing: { done: 0, total: paths.length, current: null } });
    const outcome = await useProjectStore.getState().importMedia(paths);
    set({ importing: null, refused: outcome?.refused ?? [] });
  },
}));
