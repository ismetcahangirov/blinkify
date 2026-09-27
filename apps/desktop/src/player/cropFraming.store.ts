import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { usePreviewStore } from "./preview.store.js";

/**
 * Whether a crop is being framed on the preview, and for which clip (#131).
 *
 * While it is, the engine plays that clip whole (`frame_crop`), so the
 * rectangle is drawn over the picture it cuts from; everything else plays as
 * it exports. Leaving shows the crop again. Only the preview changes: the
 * graph, the plan and the export never see this state.
 */
interface CropFramingState {
  /** The clip being framed, or `null`. */
  clip: number | null;
  /** Frame `placement`'s crop: show it whole, and bring the playhead onto
   * it if it is elsewhere, so there is a picture to draw on. */
  enter: (placement: Placement) => Promise<void>;
  /** Show every clip as it exports again. */
  leave: () => Promise<void>;
}

export const useCropFramingStore = create<CropFramingState>((set, get) => ({
  clip: null,
  enter: async (placement) => {
    set({ clip: placement.clip });
    await invoke("frame_crop", { clip: placement.clip });
    const preview = usePreviewStore.getState();
    const shown = preview.frameNumber;
    const inside =
      shown !== null &&
      shown >= placement.start &&
      shown < placement.start + placement.length;
    const rate =
      useProjectStore.getState().view?.project.sequence.settings.frameRate;
    if (!inside && rate && rate.num > 0) {
      await preview.transport({
        type: "seek",
        position: Math.round(
          (placement.start * 1_000_000 * rate.den) / rate.num,
        ),
      });
    }
  },
  leave: async () => {
    if (get().clip === null) return;
    set({ clip: null });
    await invoke("frame_crop", { clip: null });
  },
}));
