import type { ImportProgress, SettingsImpact } from "@blinkify/types";
import { Button, Dialog, DialogClose, Select, Tabs } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useFilesDrop } from "../player/useFileDrop.js";
import { useProjectStore } from "../project/project.store.js";
import { impactStatement } from "../project/SequenceSettingsDialog.js";
import { startAssetDrag, type AssetDrag } from "./assetDrag.js";
import { AssetCard } from "./AssetCard.js";
import { assetsOf, filterAssets, type Asset } from "./assets.js";
import { setLibrarySources } from "./libraryMedia.js";
import { useLibraryStore, type Density } from "./library.store.js";

/** The engine's import event: see `library::IMPORT_EVENT` in the shell. */
export const IMPORT_EVENT = "library://import";

/**
 * What the import dialog offers. A filter, not a gate: the engine probes
 * whatever arrives and refuses what it cannot use, with the reason.
 */
export const MEDIA_EXTENSIONS = [
  "mp4",
  "mov",
  "m4v",
  "mkv",
  "webm",
  "avi",
  "mts",
  "m2ts",
  "mxf",
  "mp3",
  "m4a",
  "aac",
  "wav",
  "flac",
  "ogg",
  "opus",
];

/** The tab row from the layout reference: only Media in v1 (#53). */
const LATER_TABS = [
  "Audio",
  "Text",
  "Stickers",
  "Effects",
  "Transitions",
  "Filters",
] as const;

/** Pixels a press must travel before it is a drag, as on the timeline. */
const DRAG_THRESHOLD = 3;

async function chooseMedia(multiple: boolean): Promise<string[]> {
  const chosen = await openDialog({
    multiple,
    directory: false,
    filters: [{ name: "Video and audio", extensions: MEDIA_EXTENSIONS }],
  });
  if (chosen === null) return [];
  return Array.isArray(chosen) ? chosen : [chosen];
}

/**
 * The media library (#53): import, browse, search, and drag onto the
 * timeline. What it lists is the project's sources — references to files
 * where they are, never copies.
 */
export function MediaLibrary() {
  const [tab, setTab] = useState("media");
  const tabs = [
    { value: "media", label: "Media", content: <MediaTab /> },
    ...LATER_TABS.map((label) => ({
      value: label.toLowerCase(),
      label,
      content: null,
      disabled: true,
    })),
  ];
  return (
    <Tabs
      label="Library"
      className="library__tabs"
      tabs={tabs}
      value={tab}
      onValueChange={setTab}
    />
  );
}

function MediaTab() {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const relink = useProjectStore((state) => state.relink);
  const {
    query,
    kind,
    density,
    importing,
    refused,
    setQuery,
    setKind,
    setDensity,
    progress,
    dismissRefused,
    importPaths,
  } = useLibraryStore();
  const [selected, setSelected] = useState<readonly number[]>([]);
  const [removing, setRemoving] = useState<Asset | null>(null);
  const [matching, setMatching] = useState<{
    asset: Asset;
    impact: SettingsImpact;
  } | null>(null);
  const [ghost, setGhost] = useState<{
    x: number;
    y: number;
    label: string;
  } | null>(null);
  const panel = useRef<HTMLDivElement>(null);

  const assets = useMemo(() => assetsOf(view), [view]);
  const shown = filterAssets(assets, query, kind);

  useEffect(() => {
    setLibrarySources(new Map(assets.map((a) => [a.id, a.path])));
  }, [assets]);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let closed = false;
    void listen<ImportProgress>(IMPORT_EVENT, (event) =>
      progress(event.payload),
    )
      .then((unlisten) => {
        if (closed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      closed = true;
      stop?.();
    };
  }, [progress]);

  useFilesDrop(panel, (paths) => void importPaths(paths));

  const importChosen = async () => importPaths(await chooseMedia(true));

  const relinkChosen = async (asset: Asset) => {
    const [path] = await chooseMedia(false);
    if (path !== undefined) await relink(asset.id, path);
  };

  const remove = (asset: Asset) => {
    if (asset.uses > 0) setRemoving(asset);
    else void edit({ edit: "remove-source", source: asset.id });
  };

  const offerMatch = async (asset: Asset) => {
    const settings = asset.info?.matching;
    if (!settings) return;
    const impact = await invoke<SettingsImpact>("preview_settings", {
      settings,
    }).catch(() => null);
    if (impact) setMatching({ asset, impact });
  };

  /** A press on a card: a click selects it, a drag carries the selection. */
  const press = (asset: Asset, event: React.PointerEvent<HTMLElement>) => {
    if (event.button !== 0 || (event.target as Element).closest("button"))
      return;
    const additive = event.ctrlKey || event.metaKey;
    const dragged = selected.includes(asset.id) ? selected : [asset.id];
    const origin = { x: event.clientX, y: event.clientY };
    let drag: AssetDrag | null = null;
    const move = (e: PointerEvent) => {
      const point = { x: e.clientX, y: e.clientY };
      if (!drag) {
        if (Math.hypot(point.x - origin.x, point.y - origin.y) < DRAG_THRESHOLD)
          return;
        drag = startAssetDrag({ sources: dragged });
      }
      drag.move(point, { alt: e.altKey });
      setGhost({
        ...point,
        label: dragged.length === 1 ? asset.name : `${dragged.length} files`,
      });
    };
    const finish = (e: PointerEvent, cancelled: boolean) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", escape);
      setGhost(null);
      if (drag) {
        if (cancelled) drag.cancel();
        else drag.release({ x: e.clientX, y: e.clientY }, { alt: e.altKey });
        return;
      }
      if (cancelled) return;
      setSelected((current) =>
        additive
          ? current.includes(asset.id)
            ? current.filter((id) => id !== asset.id)
            : [...current, asset.id]
          : [asset.id],
      );
    };
    const up = (e: PointerEvent) => finish(e, false);
    const cancel = (e: PointerEvent) => finish(e, true);
    const escape = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      drag?.cancel();
      drag = null;
      setGhost(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", escape);
  };

  return (
    <div ref={panel} className="library" data-testid="library">
      <div className="library__toolbar">
        <Button
          variant="primary"
          disabled={!view}
          onClick={() => void importChosen()}
        >
          Import
        </Button>
        <input
          className="library__search"
          type="search"
          placeholder="Search"
          aria-label="Search media"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <Select
          label="Media type"
          value={kind}
          onValueChange={(value) =>
            setKind(value === "video" || value === "audio" ? value : "all")
          }
          options={[
            { value: "all", label: "All" },
            { value: "video", label: "Video" },
            { value: "audio", label: "Audio" },
          ]}
        />
        <Select
          label="Density"
          value={density}
          onValueChange={(value) => setDensity(value as Density)}
          options={[
            { value: "grid", label: "Grid" },
            { value: "list", label: "List" },
          ]}
        />
      </div>

      {importing && (
        <div className="library__progress" role="status">
          <progress max={importing.total} value={importing.done} />
          <span>
            Importing {importing.done} of {importing.total}…
          </span>
        </div>
      )}

      {refused.length > 0 && (
        <div className="library__refused" role="alert" data-testid="refused">
          <p>
            {refused.length === 1
              ? "One file was not imported:"
              : `${refused.length} files were not imported:`}
          </p>
          <ul>
            {refused.map((refusal) => (
              <li key={refusal.path}>
                <strong>{refusal.path.split(/[\\/]/).pop()}</strong>:{" "}
                {refusal.reason}.
              </li>
            ))}
          </ul>
          <Button variant="secondary" onClick={dismissRefused}>
            Dismiss
          </Button>
        </div>
      )}

      {!view ? (
        <p className="zone__placeholder">Open or create a project to import.</p>
      ) : assets.length === 0 ? (
        <p className="zone__placeholder">
          Import files, or drop them here. They stay where they are — the
          project refers to them, it never copies them.
        </p>
      ) : shown.length === 0 ? (
        <p className="zone__placeholder">Nothing matches.</p>
      ) : (
        <ul
          className={`library__assets library__assets--${density}`}
          aria-label="Media"
        >
          {shown.map((asset) => (
            <AssetCard
              key={asset.id}
              asset={asset}
              selected={selected.includes(asset.id)}
              onPointerDown={(event) => press(asset, event)}
              onRelink={() => void relinkChosen(asset)}
              onRemove={() => remove(asset)}
              onMatch={() => void offerMatch(asset)}
            />
          ))}
        </ul>
      )}

      {ghost && (
        <div
          className="library__ghost"
          style={{ left: ghost.x + 12, top: ghost.y + 12 }}
          aria-hidden="true"
        >
          {ghost.label}
        </div>
      )}

      <Dialog
        open={removing !== null}
        onOpenChange={(open) => !open && setRemoving(null)}
        title={`Remove ${removing?.name ?? ""}?`}
        description={`It is used by ${removing?.uses ?? 0} ${
          removing?.uses === 1 ? "clip" : "clips"
        } on the timeline. Removing it removes ${
          removing?.uses === 1 ? "that clip" : "those clips"
        } too. The file itself is not touched, and Undo brings it all back.`}
        actions={
          <>
            <DialogClose>
              <Button variant="secondary">Cancel</Button>
            </DialogClose>
            <Button
              variant="primary"
              onClick={() => {
                if (removing)
                  void edit({ edit: "remove-source", source: removing.id });
                setRemoving(null);
              }}
            >
              Remove
            </Button>
          </>
        }
      />

      <Dialog
        open={matching !== null}
        onOpenChange={(open) => !open && setMatching(null)}
        title="Match the sequence to this file?"
        description={matching ? impactStatement(matching.impact) : ""}
        actions={
          <>
            <DialogClose>
              <Button variant="secondary">Cancel</Button>
            </DialogClose>
            <Button
              variant="primary"
              onClick={() => {
                const settings = matching?.asset.info?.matching;
                if (settings)
                  void edit({
                    edit: "set-settings",
                    settings: {
                      ...settings,
                      frameRate: { ...settings.frameRate },
                      pixelAspect: { ...settings.pixelAspect },
                    },
                  });
                setMatching(null);
              }}
            >
              Match
            </Button>
          </>
        }
      />
    </div>
  );
}
