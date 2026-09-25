import { displayAspect } from "../project/SequenceSettingsDialog.js";
import { useProjectStore } from "../project/project.store.js";
import { formatRate } from "./speed.js";

/**
 * The inspector with nothing selected (#56, per the layout reference): the
 * sequence, and the rule binding it to copy eligibility (#57). A sequence
 * that disagrees with its sources is the commonest reason a clip cannot be
 * copied, so this is where it is said — from the engine's eligibility, not
 * worked out here.
 */
export function SequenceSummary() {
  const view = useProjectStore((state) => state.view);
  if (!view)
    return (
      <p className="zone__placeholder">
        Contents follow the timeline selection. No project is open.
      </p>
    );
  const settings = view.project.sequence.settings;
  const eligibility = Object.values(view.eligibility);
  const copied = eligibility.filter((e) => e.eligible).length;
  return (
    <section className="inspector-section" aria-labelledby="inspector-sequence">
      <h3 id="inspector-sequence" className="inspector-section__title">
        Sequence
      </h3>
      <p className="zone__placeholder">
        Nothing is selected. Select a clip on the timeline to inspect it.
      </p>
      <dl className="inspector-facts">
        <dt>Resolution</dt>
        <dd>
          {settings.width}×{settings.height} ({displayAspect(settings)})
        </dd>
        <dt>Frame rate</dt>
        <dd>{formatRate(settings.frameRate)}</dd>
        <dt>Pixel aspect</dt>
        <dd>
          {settings.pixelAspect.num}:{settings.pixelAspect.den}
        </dd>
        <dt>Colour</dt>
        <dd>SDR (Rec. 709)</dd>
      </dl>
      <p className="inspector-note">
        {eligibility.length === 0
          ? "A source is copied only where the sequence has its resolution, frame rate and pixel aspect."
          : `${copied} of ${eligibility.length} sources match the sequence and can be copied. A source is copied only where the sequence has its resolution, frame rate and pixel aspect.`}{" "}
        Change them in File ▸ Sequence settings….
      </p>
    </section>
  );
}
