import { Button } from "@blinkify/ui";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { ClipTimingFields } from "./ClipTimingFields.js";
import { SourceProperties } from "./SourceProperties.js";
import { SpeedControl } from "./SpeedControl.js";
import { shared } from "./mixedValue.js";
import { NORMAL_SPEED, sameRatio } from "./speed.js";

/**
 * The video section (#56): the selected video clips' timing, speed and
 * source, in the order the layout reference gives (clip, speed, then what
 * the source is).
 *
 * A view of the selection and nothing else. It reads the evaluated graph
 * and sends edits; it holds no copy of a value that could disagree with
 * the graph.
 */
export function VideoInspector({ clips }: { clips: readonly Placement[] }) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const [single] = clips;
  const source = shared(clips.map((clip) => clip.source));
  const atDefaults = clips.every((clip) => sameRatio(clip.speed, NORMAL_SPEED));

  return (
    <div className="video-inspector">
      <section className="inspector-section" aria-labelledby="inspector-clip">
        <header className="inspector-section__header">
          <h3 id="inspector-clip" className="inspector-section__title">
            {clips.length === 1 ? "Video clip" : `${clips.length} video clips`}
          </h3>
          {/* The section's reset: every setting here that has a default.
              Timing has none — a trim is where the user put it. */}
          <Button
            size="sm"
            variant="ghost"
            disabled={atDefaults}
            onClick={() =>
              void edit({
                edit: "set-speed",
                clips: clips.map((clip) => clip.clip),
                ratio: NORMAL_SPEED,
              })
            }
          >
            Reset all
          </Button>
        </header>
        {clips.length === 1 && single ? (
          <ClipTimingFields placement={single} />
        ) : (
          <p className="inspector-note">
            In and out points are set one clip at a time.
          </p>
        )}
      </section>

      <section className="inspector-section" aria-labelledby="inspector-speed">
        <h3 id="inspector-speed" className="inspector-section__title">
          Speed
        </h3>
        <SpeedControl clips={clips} />
      </section>

      <section className="inspector-section" aria-labelledby="inspector-source">
        <h3 id="inspector-source" className="inspector-section__title">
          Source
        </h3>
        {source.kind === "same" ? (
          <SourceProperties
            source={view?.project.sources[source.value]}
            asset={view?.assets[source.value]}
          />
        ) : (
          <p className="inspector-note">
            The clips come from different sources. Select clips of one source to
            see its properties.
          </p>
        )}
      </section>
    </div>
  );
}
