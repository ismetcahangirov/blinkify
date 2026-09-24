import { memo } from "react";
import { MediaLibrary } from "../../library/MediaLibrary.js";

/**
 * The media library (#53): import, the asset grid, search, and dragging
 * media onto the timeline.
 *
 * `memo` is not an optimisation guess here — it is the boundary the issue asks
 * for: "each zone is an independent React subtree, so a re-render in one does
 * not re-render the others". A drag from here to the timeline crosses that
 * boundary as data (`library/assetDrag.ts`), never as a component.
 *
 * The heading is real rather than decorative. The shell is the only thing in
 * the product that builds a whole document, so it is where the landmark and
 * heading structure has to be correct.
 */
export const LibraryZone = memo(function LibraryZone() {
  return (
    <div className="zone">
      <h2 className="zone__title">Library</h2>
      <MediaLibrary />
    </div>
  );
});
