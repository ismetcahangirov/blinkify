import type { Edit, StreamExtent } from "@blinkify/types";
import type { Placement, TrackRow } from "./draw.js";
import { xOf, type Viewport } from "./viewport.js";

/**
 * Clip interaction on the timeline (#34): what the pointer is over, where a
 * drag would land, and what it would commit. Pure, so every rule is tested
 * without a browser.
 *
 * A drag never touches the graph while it is under way. It is previewed on
 * the overlay, and on release exactly one edit is sent — so one drag is one
 * undo entry, however many pointer moves it took. All the preview arithmetic
 * is in sequence frames; the engine converts frames to source ticks, exactly,
 * and bounds the result itself. The preview's bounds are only there so the
 * user sees the limit while dragging, not after.
 */

/** How close, in screen pixels, an edge must come to a target to snap. A
 * distance on screen, not in time: the same feel at every zoom. */
export const SNAP_PIXELS = 8;

/** How wide the grab zone at each end of a clip is, in screen pixels. */
export const EDGE_PIXELS = 6;

/** How far the pointer must move before a press becomes a drag. */
export const DRAG_THRESHOLD = 3;

export type Zone = "body" | "start" | "end";

export interface Hit {
  readonly row: TrackRow;
  readonly placement: Placement;
  readonly zone: Zone;
}

/**
 * The clip under a point of the clip area, and which part of it. `y` is from
 * the top of the track stack (ruler excluded, scroll included).
 */
export function hitTest(
  rows: readonly TrackRow[],
  view: Viewport,
  x: number,
  y: number,
): Hit | null {
  const row = rows.find((r) => y >= r.top && y < r.top + r.height);
  if (!row) return null;
  for (const placement of row.placements) {
    const left = xOf(view, placement.start);
    const right = xOf(view, placement.start + placement.length);
    if (x < left || x >= right) continue;
    const grab = Math.min(EDGE_PIXELS, (right - left) / 3);
    const zone: Zone =
      x < left + grab ? "start" : x >= right - grab ? "end" : "body";
    return { row, placement, zone };
  }
  return null;
}

export interface Modifiers {
  readonly ctrl: boolean;
  readonly shift: boolean;
  readonly alt: boolean;
}

/**
 * The selection after clicking `hit` (or empty space, `null`): a plain click
 * selects one clip; Ctrl adds or removes it; Shift selects the run of clips
 * on its track from the last one selected.
 */
export function clickSelection(
  selection: readonly number[],
  hit: Hit | null,
  modifiers: Modifiers,
): number[] {
  if (!hit) return modifiers.ctrl || modifiers.shift ? [...selection] : [];
  const clip = hit.placement.clip;
  if (modifiers.ctrl)
    return selection.includes(clip)
      ? selection.filter((c) => c !== clip)
      : [...selection, clip];
  if (modifiers.shift) {
    const order = hit.row.placements.map((p) => p.clip);
    const anchor = [...selection].reverse().find((c) => order.includes(c));
    if (anchor === undefined) return [clip];
    const a = order.indexOf(anchor);
    const b = order.indexOf(clip);
    const run = order.slice(Math.min(a, b), Math.max(a, b) + 1);
    return [...new Set([...selection, ...run])];
  }
  return selection.includes(clip) ? [...selection] : [clip];
}

/**
 * `selection` with every clip linked to a selected one (#36): selecting a
 * video clip selects its detached sound, as CapCut does, until they are
 * unlinked.
 */
export function withLinked(
  selection: readonly number[],
  tracks: readonly { clips: readonly { id: number; link?: number }[] }[],
): number[] {
  const clips = tracks.flatMap((track) => track.clips);
  const links = new Set(
    clips
      .filter((clip) => selection.includes(clip.id) && clip.link !== undefined)
      .map((clip) => clip.link),
  );
  const linked = clips
    .filter((clip) => clip.link !== undefined && links.has(clip.link))
    .map((clip) => clip.id);
  return [...new Set([...selection, ...linked])];
}

/** Every clip, for select-all. */
export function allClips(rows: readonly TrackRow[]): number[] {
  return rows.flatMap((row) => row.placements.map((p) => p.clip));
}

/**
 * The frames an edge can snap to: every clip edge not being dragged, the
 * playhead, and the start of the sequence.
 */
export function snapTargets(
  rows: readonly TrackRow[],
  exclude: ReadonlySet<number>,
  playhead: number | null,
): number[] {
  const targets = new Set<number>([0]);
  if (playhead !== null) targets.add(playhead);
  for (const row of rows)
    for (const p of row.placements) {
      if (exclude.has(p.clip)) continue;
      targets.add(p.start);
      targets.add(p.start + p.length);
    }
  return [...targets].sort((a, b) => a - b);
}

/**
 * Snap any of `edges` (frames) to the nearest target within `pixels` on
 * screen. Returns the shift, in frames, that brings the closest edge onto
 * its target, and that target — or no shift.
 */
export function snap(
  edges: readonly number[],
  targets: readonly number[],
  scale: number,
  pixels = SNAP_PIXELS,
): { shift: number; target: number | null } {
  let best: { shift: number; target: number | null } = {
    shift: 0,
    target: null,
  };
  let distance = pixels / scale;
  for (const edge of edges)
    for (const target of targets) {
      const gap = Math.abs(target - edge);
      if (gap <= distance) {
        distance = gap;
        best = { shift: target - edge, target };
      }
    }
  return best;
}

/** Sequence frames a source tick range lasts in a placement. */
function playedFrames(placement: Placement, ticks: number): number {
  const tb = placement.timeBase;
  const seq = placement.sequenceTimeBase;
  const speed = placement.speed;
  return (
    (ticks * tb.num * seq.den * speed.den) / (tb.den * seq.num * speed.num)
  );
}

/**
 * How far an edge may move, in whole frames, before its source runs out —
 * the preview's bound. The engine applies the exact one.
 */
export function edgeReach(
  placement: Placement,
  edge: "start" | "end",
  extents: readonly StreamExtent[],
): [number, number] {
  const extent = extents.find(
    (e) =>
      e.source === placement.source &&
      e.stream === placement.stream &&
      e.timeBase.num === placement.timeBase.num &&
      e.timeBase.den === placement.timeBase.den,
  );
  if (edge === "start") {
    const before = extent
      ? Math.floor(playedFrames(placement, placement.sourceIn - extent.start))
      : Infinity;
    return [-before, placement.length - 1];
  }
  const after = extent
    ? Math.floor(
        playedFrames(placement, extent.end - placement.sourceIn) -
          placement.length,
      )
    : Infinity;
  return [1 - placement.length, Math.max(0, after)];
}

// ── drags ────────────────────────────────────────────────────────────────

export interface MoveDrag {
  readonly kind: "move";
  /** The clips moving, with their rows. */
  readonly clips: readonly { placement: Placement; row: TrackRow }[];
  /** The row the pointer grabbed. */
  readonly from: TrackRow;
}

export interface TrimDrag {
  readonly kind: "trim";
  readonly placement: Placement;
  readonly row: TrackRow;
  readonly edge: "start" | "end";
  /** The adjacent clip on the far side of the edge, for a roll. */
  readonly neighbour: Placement | null;
}

export type Drag = MoveDrag | TrimDrag;

/** Where a drag has got to: what it would commit, and how to show it. */
export interface DragResult {
  /** The clips as they would be: `[clip, row id, start, length]`. */
  readonly ghosts: readonly {
    clip: number;
    row: number;
    start: number;
    length: number;
  }[];
  /** The frame an edge has snapped to, for the snap line. */
  readonly snapped: number | null;
  /** A bound stopped the drag: the source ran out, a neighbour is in the way. */
  readonly clamped: boolean;
  /** The drop cannot happen here: another kind of track, or on a clip. */
  readonly invalid: string | null;
  /** What releasing would send, or `null` for nothing to do. */
  readonly edit: Edit | null;
}

/** Whether `start..start+length` on `row` hits a clip not in `moving`. */
function collides(
  row: TrackRow,
  start: number,
  length: number,
  moving: ReadonlySet<number>,
): boolean {
  return row.placements.some(
    (p) =>
      !moving.has(p.clip) &&
      p.start < start + length &&
      start < p.start + p.length,
  );
}

export interface DragInput {
  /** Frames the pointer has moved since the press, unrounded. */
  readonly frames: number;
  /** The row now under the pointer. */
  readonly row: TrackRow | null;
  readonly modifiers: Modifiers;
  readonly rows: readonly TrackRow[];
  readonly view: Viewport;
  readonly playhead: number | null;
  readonly extents: readonly StreamExtent[];
}

export function dragTo(drag: Drag, input: DragInput): DragResult {
  return drag.kind === "move" ? moveTo(drag, input) : trimTo(drag, input);
}

function moveTo(drag: MoveDrag, input: DragInput): DragResult {
  const moving = new Set(drag.clips.map((c) => c.placement.clip));
  let delta = Math.round(input.frames);
  let snapped: number | null = null;
  if (!input.modifiers.alt) {
    const edges = drag.clips.flatMap(({ placement }) => [
      placement.start + delta,
      placement.start + placement.length + delta,
    ]);
    const result = snap(
      edges,
      snapTargets(input.rows, moving, input.playhead),
      input.view.scale,
    );
    delta += result.shift;
    snapped = result.target;
  }
  const earliest = Math.min(...drag.clips.map((c) => c.placement.start));
  if (earliest + delta < 0) delta = -earliest;

  // Moving between tracks moves every clip by the same number of rows.
  const target = input.row ?? drag.from;
  const rowShift = input.rows.indexOf(target) - input.rows.indexOf(drag.from);
  let invalid: string | null = null;
  const ghosts = drag.clips.map(({ placement, row }) => {
    const to = input.rows[input.rows.indexOf(row) + rowShift] ?? row;
    if (to.locked) invalid = "That track is locked.";
    else if (to.kind !== row.kind)
      invalid = `A ${row.kind} clip cannot go on a ${to.kind} track.`;
    else if (collides(to, placement.start + delta, placement.length, moving))
      invalid = "Another clip is in the way.";
    return {
      clip: placement.clip,
      row: to.id,
      start: placement.start + delta,
      length: placement.length,
    };
  });
  const unchanged = delta === 0 && rowShift === 0;
  return {
    ghosts,
    snapped,
    clamped: false,
    invalid,
    edit:
      invalid || unchanged
        ? null
        : {
            edit: "move-clips",
            moves: ghosts.map((g) => ({
              clip: g.clip,
              track: g.row,
              start: g.start,
            })),
          },
  };
}

function trimTo(drag: TrimDrag, input: DragInput): DragResult {
  const { placement, row, edge } = drag;
  const ripple = input.modifiers.shift;
  const rolling = input.modifiers.ctrl && drag.neighbour !== null;
  const end = placement.start + placement.length;
  let delta = Math.round(input.frames);
  let snapped: number | null = null;
  if (!input.modifiers.alt) {
    const exclude = new Set([placement.clip]);
    if (rolling && drag.neighbour) exclude.add(drag.neighbour.clip);
    const at = edge === "start" ? placement.start : end;
    const result = snap(
      [at + delta],
      snapTargets(input.rows, exclude, input.playhead),
      input.view.scale,
    );
    delta += result.shift;
    snapped = result.target;
  }

  // The bounds: the source, one frame of length, and — unless it ripples or
  // rolls — the neighbours on the track.
  let [low, high] = edgeReach(placement, edge, input.extents);
  if (rolling && drag.neighbour) {
    const other = edgeReach(
      drag.neighbour,
      edge === "end" ? "start" : "end",
      input.extents,
    );
    low = Math.max(low, other[0]);
    high = Math.min(high, other[1]);
  } else if (!ripple) {
    const others = row.placements.filter((p) => p.clip !== placement.clip);
    if (edge === "start") {
      const before = Math.max(
        0,
        ...others
          .filter((p) => p.start + p.length <= placement.start)
          .map((p) => p.start + p.length),
      );
      low = Math.max(low, before - placement.start);
    } else {
      const after = others.filter((p) => p.start >= end).map((p) => p.start);
      if (after.length > 0) high = Math.min(high, Math.min(...after) - end);
    }
  } else if (edge === "start") {
    low = Math.max(low, -placement.start);
  }
  const clamped = delta < low || delta > high;
  delta = Math.min(high, Math.max(low, delta));
  if (clamped) snapped = null;

  const ghost =
    edge === "start"
      ? ripple
        ? {
            clip: placement.clip,
            row: row.id,
            start: placement.start,
            length: placement.length - delta,
          }
        : {
            clip: placement.clip,
            row: row.id,
            start: placement.start + delta,
            length: placement.length - delta,
          }
      : {
          clip: placement.clip,
          row: row.id,
          start: placement.start,
          length: placement.length + delta,
        };
  const ghosts = [ghost];
  if (rolling && drag.neighbour) {
    const n = drag.neighbour;
    ghosts.push(
      edge === "end"
        ? {
            clip: n.clip,
            row: row.id,
            start: n.start + delta,
            length: n.length - delta,
          }
        : {
            clip: n.clip,
            row: row.id,
            start: n.start,
            length: n.length + delta,
          },
    );
  }
  let edit: DragResult["edit"] = null;
  if (delta !== 0) {
    if (rolling && drag.neighbour)
      edit =
        edge === "end"
          ? {
              edit: "roll",
              left: placement.clip,
              right: drag.neighbour.clip,
              frames: delta,
            }
          : {
              edit: "roll",
              left: drag.neighbour.clip,
              right: placement.clip,
              frames: delta,
            };
    else
      edit = {
        edit: "trim-edge",
        clip: placement.clip,
        edge,
        frames: delta,
        ripple,
      };
  }
  return { ghosts, snapped, clamped, invalid: null, edit };
}

/** The clip that meets `placement`'s `edge` exactly, for a roll. */
export function neighbourAt(
  row: TrackRow,
  placement: Placement,
  edge: "start" | "end",
): Placement | null {
  const at =
    edge === "start" ? placement.start : placement.start + placement.length;
  return (
    row.placements.find((p) =>
      edge === "start" ? p.start + p.length === at : p.start === at,
    ) ?? null
  );
}
