import type { AssetInfo, SourceRef } from "@blinkify/types";
import type { DeepReadonly } from "../project/project.store.js";
import { fileName } from "../project/project.store.js";
import { formatRate } from "./speed.js";

/**
 * A source's pictures, read-only (#56), exactly as the probe (#23) reported
 * them. This is what a user quotes in a bug report, so nothing is rounded or
 * prettified: a frame rate is `30000/1001`, a colour transfer is
 * `arib-std-b67`, and a value the file does not state says so rather than
 * showing a guess.
 */
export function SourceProperties({
  source,
  asset,
}: {
  source: DeepReadonly<SourceRef> | undefined;
  asset: DeepReadonly<AssetInfo> | undefined;
}) {
  const video = asset?.video;
  const stated = (value: string | number | null | undefined) =>
    value === null || value === undefined ? "Not stated" : String(value);
  return (
    <dl className="inspector-facts" aria-label="Source properties">
      <dt>File</dt>
      <dd title={source?.path}>{source ? fileName(source.path) : "Unknown"}</dd>
      {video ? (
        <>
          <dt>Codec</dt>
          <dd>{stated(video.codec)}</dd>
          <dt>Profile</dt>
          <dd>{stated(video.profile)}</dd>
          <dt>Resolution</dt>
          <dd>
            {video.width}×{video.height}
            {video.rotation === 0
              ? ""
              : `, shown ${video.displayWidth}×${video.displayHeight} (rotated ${video.rotation}°)`}
          </dd>
          <dt>Bit depth</dt>
          <dd>
            {video.bitDepth === null ? "Not stated" : `${video.bitDepth}-bit`}
          </dd>
          <dt>Pixel format</dt>
          <dd>{stated(video.pixelFormat)}</dd>
          <dt>Frame rate</dt>
          <dd>
            {video.frameRate ? formatRate(video.frameRate) : "Not stated"}
          </dd>
          <dt>Variable frame rate</dt>
          <dd>{video.variableFrameRate ? "Yes" : "No"}</dd>
          <dt>Colour primaries</dt>
          <dd>{stated(video.colourPrimaries)}</dd>
          <dt>Transfer</dt>
          <dd>{stated(video.colourTransfer)}</dd>
          <dt>Matrix</dt>
          <dd>{stated(video.colourMatrix)}</dd>
          <dt>Range</dt>
          <dd>{stated(video.colourRange)}</dd>
          <dt>HDR</dt>
          <dd>{video.hdr ? "Yes" : "No"}</dd>
        </>
      ) : (
        <>
          <dt>Pictures</dt>
          <dd>Unknown: the source is offline or could not be read</dd>
        </>
      )}
    </dl>
  );
}
