import type { Timeline, Track } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import type { TrackRow } from "./draw.js";

/**
 * The track stack's vertical layout (#33, #36): one row per evaluated track,
 * top to bottom in the sequence's order — which is also the compositing
 * order, the top video track in front. Video rows are taller than audio so
 * thumbnails read; a collapsed track is a thin strip.
 */

/** The ruler along the top, in CSS pixels. */
export const RULER_HEIGHT = 24;

export const ROW_HEIGHT = { video: 56, audio: 40 } as const;

/** A collapsed track's height. */
export const COLLAPSED_HEIGHT = 20;

/** Each track's name as shown: the user's, or "V1", "A2" counted from the top. */
export function trackNames(
  tracks: readonly DeepReadonly<Track>[],
): Map<number, string> {
  let video = 0;
  let audio = 0;
  return new Map(
    tracks.map((track) => {
      const counted = track.kind === "video" ? `V${++video}` : `A${++audio}`;
      return [track.id, track.name.trim() || counted];
    }),
  );
}

export function layoutRows(
  timeline: DeepReadonly<Timeline> | null | undefined,
  tracks: readonly DeepReadonly<Track>[] = [],
): TrackRow[] {
  const headers = new Map(tracks.map((track) => [track.id, track]));
  let top = 0;
  return (timeline?.tracks ?? []).map((track) => {
    const header = headers.get(track.id);
    const height = header?.collapsed
      ? COLLAPSED_HEIGHT
      : ROW_HEIGHT[track.kind];
    const row: TrackRow = {
      id: track.id,
      kind: track.kind,
      top,
      height,
      placements: track.placements,
      muted: header?.muted ?? false,
      locked: header?.locked ?? false,
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
