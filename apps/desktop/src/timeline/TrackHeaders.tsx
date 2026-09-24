import type { Edit, Track } from "@blinkify/types";
import { Button, DropdownMenu, IconButton } from "@blinkify/ui";
import { useState } from "react";
import {
  useProjectStore,
  type DeepReadonly,
} from "../project/project.store.js";
import { layoutRows, RULER_HEIGHT, trackNames } from "./rows.js";
import { useTimelineStore } from "./timeline.store.js";

/**
 * The track-header column (#33, #36): fixed while the clips scroll sideways,
 * following them up and down, one header per row.
 *
 * Each header names its track and holds its switches — mute, solo, lock,
 * collapse — and a menu to rename, move or remove it. Every one is an edit
 * on the graph (#37), undoable, and a lock is enforced by the engine, not by
 * disabling buttons here. Tracks are listed top to bottom, which is the
 * compositing order: the top video track is in front.
 */

function Glyph({ d }: { d: string }) {
  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d={d}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

const GLYPHS = {
  eye: "M1.5 8s2.5-4.5 6.5-4.5S14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8zM8 6.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3z",
  speaker: "M2.5 6h2.5l3-2.5v9L5 10H2.5zM11 5.5a3.5 3.5 0 0 1 0 5",
  lock: "M4.5 7.5h7v6h-7zM6 7.5V5.5a2 2 0 0 1 4 0v2",
  collapse: "M4 6l4 4 4-4",
  more: "M4 8h.01M8 8h.01M12 8h.01",
};

function send(edit: Edit): void {
  void useProjectStore
    .getState()
    .edit(edit)
    .then((refusal) => {
      if (refusal) useTimelineStore.getState().setNotice(refusal);
    });
}

const setTrack = (
  track: number,
  flags: Partial<Record<"muted" | "solo" | "locked" | "collapsed", boolean>>,
): Edit => ({
  edit: "set-track",
  track,
  muted: flags.muted ?? null,
  solo: flags.solo ?? null,
  locked: flags.locked ?? null,
  collapsed: flags.collapsed ?? null,
});

function TrackHeader({
  track,
  name,
  height,
  index,
  count,
}: {
  track: DeepReadonly<Track>;
  name: string;
  height: number;
  index: number;
  count: number;
}) {
  const [renaming, setRenaming] = useState(false);
  const clips = track.clips.length;
  return (
    <div
      role="listitem"
      className="timeline__header"
      data-kind={track.kind}
      data-muted={track.muted}
      data-locked={track.locked}
      style={{ height }}
    >
      {renaming ? (
        <input
          className="timeline__rename"
          aria-label={`Rename ${name}`}
          defaultValue={track.name}
          placeholder={name}
          autoFocus
          onBlur={(event) => {
            setRenaming(false);
            send({
              edit: "rename-track",
              track: track.id,
              name: event.currentTarget.value,
            });
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") event.currentTarget.blur();
            if (event.key === "Escape") setRenaming(false);
          }}
        />
      ) : (
        <span
          className="timeline__track-name"
          onDoubleClick={() => setRenaming(true)}
        >
          {name}
        </span>
      )}
      {track.collapsed ? null : (
        <span className="timeline__switches">
          <IconButton
            label={
              track.kind === "video"
                ? track.muted
                  ? `Show ${name}`
                  : `Hide ${name}`
                : track.muted
                  ? `Unmute ${name}`
                  : `Mute ${name}`
            }
            size="sm"
            aria-pressed={track.muted}
            icon={
              <Glyph d={track.kind === "video" ? GLYPHS.eye : GLYPHS.speaker} />
            }
            onClick={() => send(setTrack(track.id, { muted: !track.muted }))}
          />
          <Button
            size="sm"
            variant="ghost"
            aria-label={`Solo ${name}`}
            aria-pressed={track.solo}
            onClick={() => send(setTrack(track.id, { solo: !track.solo }))}
          >
            S
          </Button>
          <IconButton
            label={track.locked ? `Unlock ${name}` : `Lock ${name}`}
            size="sm"
            aria-pressed={track.locked}
            icon={<Glyph d={GLYPHS.lock} />}
            onClick={() => send(setTrack(track.id, { locked: !track.locked }))}
          />
        </span>
      )}
      <IconButton
        label={track.collapsed ? `Expand ${name}` : `Collapse ${name}`}
        size="sm"
        aria-pressed={track.collapsed}
        icon={<Glyph d={GLYPHS.collapse} />}
        onClick={() =>
          send(setTrack(track.id, { collapsed: !track.collapsed }))
        }
      />
      <DropdownMenu
        trigger={
          <IconButton
            label={`${name} options`}
            size="sm"
            icon={<Glyph d={GLYPHS.more} />}
          />
        }
        groups={[
          {
            items: [
              {
                id: "rename",
                label: "Rename",
                onSelect: () => setRenaming(true),
              },
              {
                id: "up",
                label: "Move up",
                disabled: index === 0,
                onSelect: () =>
                  send({
                    edit: "move-track",
                    track: track.id,
                    index: index - 1,
                  }),
              },
              {
                id: "down",
                label: "Move down",
                disabled: index === count - 1,
                onSelect: () =>
                  send({
                    edit: "move-track",
                    track: track.id,
                    index: index + 1,
                  }),
              },
            ],
          },
          {
            items: [
              {
                id: "remove",
                // Removing a track removes its clips: said, not implied.
                label:
                  clips === 0
                    ? "Remove track"
                    : `Remove track and its ${clips === 1 ? "clip" : `${clips} clips`}`,
                onSelect: () => send({ edit: "remove-track", track: track.id }),
              },
            ],
          },
        ]}
      />
    </div>
  );
}

export function TrackHeaders() {
  const view = useProjectStore((state) => state.view);
  const scrollTop = useTimelineStore((state) => state.view.scrollTop);
  const tracks = view?.project.sequence.tracks ?? [];
  const rows = layoutRows(view?.timeline, tracks);
  const names = trackNames(tracks);

  return (
    <div className="timeline__headers">
      <div className="timeline__corner" style={{ height: RULER_HEIGHT }}>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => send({ edit: "add-track", kind: "video" })}
        >
          + Video
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => send({ edit: "add-track", kind: "audio" })}
        >
          + Audio
        </Button>
      </div>
      <div className="timeline__header-clip">
        <div
          className="timeline__header-stack"
          role="list"
          aria-label="Tracks"
          style={{ transform: `translateY(${-scrollTop}px)` }}
        >
          {rows.map((row, index) => {
            const track = tracks.find((t) => t.id === row.id);
            if (!track) return null;
            return (
              <TrackHeader
                key={row.id}
                track={track}
                name={names.get(row.id) ?? ""}
                height={row.height}
                index={index}
                count={rows.length}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}
