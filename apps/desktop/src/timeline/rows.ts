import type { Timeline } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import type { TrackRow } from "./draw.js";

/**
 * The track stack's vertical layout (#33): one row per evaluated track, top
 * to bottom in the sequence's order, video taller than audio so thumbnails
 * read. #36 makes heights per track and adjustable.
 */

/** The ruler along the top, in CSS pixels. */
export const RULER_HEIGHT = 24;

export const ROW_HEIGHT = { video: 56, audio: 40 } as const;

export function layoutRows(
  timeline: DeepReadonly<Timeline> | null | undefined,
): TrackRow[] {
  let top = 0;
  return (timeline?.tracks ?? []).map((track) => {
    const height = ROW_HEIGHT[track.kind];
    const row: TrackRow = {
      id: track.id,
      kind: track.kind,
      top,
      height,
      placements: track.placements,
    };
    top += height;
    return row;
  });
}

/** The height of every row together. */
export function stackHeight(rows: readonly TrackRow[]): number {
  const last = rows[rows.length - 1];
  return last ? last.top + last.height : 0;
}

/** The row under `y`, measured from the top of the track stack. */
export function rowAt(
  rows: readonly TrackRow[],
  y: number,
): TrackRow | undefined {
  return rows.find((row) => y >= row.top && y < row.top + row.height);
}
