import { useEffect, useRef, useState } from "react";
import { codecBadge, duration, resolution, type Asset } from "./assets.js";
import { cardTile, onLibraryMedia, waveformReady } from "./libraryMedia.js";

/**
 * One asset in the library (#53): its picture, name, duration, resolution
 * and codec — the probe's values, as recorded at import — and its state.
 *
 * A source that has moved is shown as missing, with the relink from #32; a
 * source whose pictures cannot be copied into the sequence says so (#57),
 * because the drop would otherwise be the first the user hears of it.
 */
export interface AssetCardProps {
  readonly asset: Asset;
  readonly selected: boolean;
  readonly onPointerDown: (event: React.PointerEvent<HTMLElement>) => void;
  readonly onRelink: () => void;
  readonly onRemove: () => void;
  readonly onMatch: () => void;
}

/** The card's picture: the filmstrip's tile a second in, drawn to fit. */
function Thumbnail({ asset }: { asset: Asset }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const [ready, setReady] = useState(false);
  const offline = asset.status !== null;

  useEffect(() => {
    if (offline) return;
    const seconds = Math.min(1, (asset.info?.durationSeconds ?? 0) / 2);
    const draw = () => {
      const audio = asset.info?.audio?.stream;
      const sound = audio === undefined || waveformReady(asset.id, audio);
      if (!asset.info?.video) {
        setReady(sound);
        return;
      }
      const tile = cardTile(asset.id, seconds);
      const target = canvas.current;
      const context = target?.getContext("2d");
      if (!tile || !target || !context) return;
      target.width = target.clientWidth * (window.devicePixelRatio || 1);
      target.height = target.clientHeight * (window.devicePixelRatio || 1);
      const scale = Math.min(target.width / tile.sw, target.height / tile.sh);
      const w = tile.sw * scale;
      const h = tile.sh * scale;
      context.clearRect(0, 0, target.width, target.height);
      context.drawImage(
        tile.image,
        tile.sx,
        tile.sy,
        tile.sw,
        tile.sh,
        (target.width - w) / 2,
        (target.height - h) / 2,
        w,
        h,
      );
      setReady(sound);
    };
    draw();
    return onLibraryMedia(draw);
  }, [asset.id, asset.info, offline]);

  return (
    <div className="asset__picture">
      {asset.info?.video ? (
        <canvas ref={canvas} className="asset__canvas" aria-hidden="true" />
      ) : (
        <span className="asset__glyph" aria-hidden="true">
          ♪
        </span>
      )}
      {!offline && !ready && (
        <span className="asset__preparing" data-testid="asset-preparing">
          Preparing…
        </span>
      )}
      <span className="asset__duration">
        {duration(asset.info?.durationSeconds)}
      </span>
    </div>
  );
}

export function AssetCard({
  asset,
  selected,
  onPointerDown,
  onRelink,
  onRemove,
  onMatch,
}: AssetCardProps) {
  const size = resolution(asset.info);
  const missing = asset.status?.state === "missing";
  const changed = asset.status?.state === "changed";
  return (
    <li
      className="asset"
      aria-selected={selected}
      data-testid={`asset-${asset.id}`}
      data-state={asset.status?.state ?? "online"}
      title={asset.path}
      onPointerDown={onPointerDown}
    >
      <Thumbnail asset={asset} />
      <div className="asset__facts">
        <span className="asset__name">{asset.name}</span>
        <span className="asset__meta">
          <span className="asset__badge">{codecBadge(asset.info)}</span>
          {size && <span>{size}</span>}
          {asset.info?.video?.hdr && <span className="asset__badge">HDR</span>}
          {asset.info?.video?.variableFrameRate && (
            <span className="asset__badge">VFR</span>
          )}
        </span>
        {missing && (
          <span className="asset__problem" role="status">
            Missing: not at {asset.path}.{" "}
            <button type="button" className="asset__action" onClick={onRelink}>
              Relink…
            </button>
          </span>
        )}
        {changed && (
          <span className="asset__problem" role="status">
            Changed since it was imported.
          </span>
        )}
        {!asset.status && asset.eligible === false && (
          <span className="asset__notice" data-testid="asset-reencodes">
            Doesn’t match the sequence: its clips will be re-encoded.{" "}
            {asset.info?.matching && (
              <button type="button" className="asset__action" onClick={onMatch}>
                Match sequence…
              </button>
            )}
          </span>
        )}
      </div>
      <button
        type="button"
        className="asset__remove"
        aria-label={`Remove ${asset.name} from the project`}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={onRemove}
      >
        ×
      </button>
    </li>
  );
}
