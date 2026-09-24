import type {
  Edit,
  ProjectView,
  Rational,
  StreamExtent,
  TrackKind,
} from "@blinkify/types";
import { fileName, type DeepReadonly } from "../project/project.store.js";
import type { TrackRow } from "./draw.js";
import type { DragResult } from "./interaction.js";
import { snap, snapTargets } from "./interaction.js";
import type { Viewport } from "./viewport.js";
import { frameAt } from "./viewport.js";

/**
 * Where library assets dropped on the timeline land (#53): a clip per asset,
 * one after another from the frame under the pointer, on the track under it
 * — with the same ghost, snapping and refusals as a clip drag (#34), and the
 * whole of each stream.
 *
 * Pure: the canvas shows the result while the pointer moves and sends its
 * edits on release.
 */

/** What the timeline needs to know about a dragged asset. */
export interface DroppedAsset {
  readonly source: number;
  readonly name: string;
  /** The stream of each kind, where the file has one. */
  readonly video: number | null;
  readonly audio: number | null;
}

/** The dragged sources as the timeline needs them, in drag order. */
export function droppedAssets(
  view: DeepReadonly<ProjectView> | null,
  sources: readonly number[],
): DroppedAsset[] {
  if (!view) return [];
  return sources.flatMap((source) => {
    const reference = view.project.sources[source];
    if (!reference) return [];
    const info = view.assets[source];
    return [
      {
        source,
        name: fileName(reference.path),
        video: info?.video?.stream ?? null,
        audio: info?.audio?.stream ?? null,
      },
    ];
  });
}

export interface DropInput {
  readonly assets: readonly DroppedAsset[];
  readonly rows: readonly TrackRow[];
  /** The row under the pointer, if any. */
  readonly row: TrackRow | null;
  /** The pointer, in the clip area's CSS pixels. */
  readonly x: number;
  readonly view: Viewport;
  readonly playhead: number | null;
  readonly extents: readonly DeepReadonly<StreamExtent>[];
  readonly frameRate: DeepReadonly<Rational>;
  /** Alt: no snapping. */
  readonly noSnap: boolean;
}

/** The result of a drop: the preview, and the edits to send in order. */
export interface DropResult extends DragResult {
  readonly edits: readonly Edit[];
}

/** Sequence frames `extent` lasts at `frameRate`, at least one. */
export function extentFrames(
  extent: DeepReadonly<StreamExtent>,
  frameRate: DeepReadonly<Rational>,
): number {
  const ticks = extent.end - extent.start;
  const frames =
    (ticks * extent.timeBase.num * frameRate.num) /
    (extent.timeBase.den * frameRate.den);
  return Math.max(1, Math.round(frames));
}

const refused = (invalid: string): DropResult => ({
  ghosts: [],
  snapped: null,
  clamped: false,
  invalid,
  edit: null,
  edits: [],
});

function streamOf(asset: DroppedAsset, kind: TrackKind): number | null {
  return kind === "video" ? asset.video : asset.audio;
}

export function dropAt(input: DropInput): DropResult {
  const { row, assets } = input;
  if (!row) return refused("Drop onto a track.");
  if (row.locked)
    return refused("This track is locked: unlock it to add clips to it.");
  const pieces: { asset: DroppedAsset; extent: DeepReadonly<StreamExtent> }[] =
    [];
  for (const asset of assets) {
    const stream = streamOf(asset, row.kind);
    if (stream === null)
      return refused(
        row.kind === "video"
          ? `${asset.name} has no pictures to put on a video track.`
          : `${asset.name} has no sound to put on an audio track.`,
      );
    const extent = input.extents.find(
      (e) => e.source === asset.source && e.stream === stream,
    );
    if (!extent) return refused(`${asset.name}'s length is not known.`);
    pieces.push({ asset, extent });
  }
  const lengths = pieces.map(({ extent }) =>
    extentFrames(extent, input.frameRate),
  );
  const total = lengths.reduce((sum, n) => sum + n, 0);
  const pointer = Math.max(0, Math.round(frameAt(input.view, input.x)));
  const snapped = input.noSnap
    ? { shift: 0, target: null }
    : snap(
        [pointer, pointer + total],
        snapTargets(input.rows, new Set(), input.playhead),
        input.view.scale,
      );
  const start = Math.max(0, pointer + snapped.shift);
  if (
    row.placements.some(
      (p) => p.start < start + total && start < p.start + p.length,
    )
  )
    return {
      ...refused("There is a clip in the way: drop into a gap."),
      ghosts: [{ clip: -1, row: row.id, start, length: total }],
    };

  let at = start;
  const ghosts: DropResult["ghosts"][number][] = [];
  const edits: Edit[] = [];
  pieces.forEach(({ asset, extent }, index) => {
    const length = lengths[index] ?? 1;
    ghosts.push({ clip: -1 - index, row: row.id, start: at, length });
    edits.push({
      edit: "add-clip",
      track: row.id,
      source: asset.source,
      stream: extent.stream,
      timeBase: { num: extent.timeBase.num, den: extent.timeBase.den },
      start: at,
      from: extent.start,
      to: extent.end,
    });
    at += length;
  });
  return {
    ghosts,
    snapped: snapped.target,
    clamped: false,
    invalid: null,
    edit: edits[0] ?? null,
    edits,
  };
}
