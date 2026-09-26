import { useEffect } from "react";
import { usePreviewStore } from "./preview.store.js";

/** How often the meter is read: often enough to look alive, cheap to ask. */
export const METER_MS = 66;

let users = 0;
let timer: ReturnType<typeof setInterval> | null = null;

/**
 * Keep the preview's levels fresh while any meter is on screen (#31, #49).
 *
 * The player's meter and the audio inspector's show the same levels, so
 * they share one timer rather than each asking the engine fifteen times a
 * second.
 */
export function useMetering(): void {
  useEffect(() => {
    users += 1;
    timer ??= setInterval(() => {
      void usePreviewStore.getState().refreshLevels();
    }, METER_MS);
    return () => {
      users -= 1;
      if (users === 0 && timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    };
  }, []);
}
