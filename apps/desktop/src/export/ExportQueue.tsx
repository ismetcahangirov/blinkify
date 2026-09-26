import type { ExportJob } from "@blinkify/types";
import { Button, Popover } from "@blinkify/ui";
import {
  baseName,
  isActive,
  isFinished,
  isInterrupted,
  statusLine,
} from "./exportJobs.js";
import { useExportJobs } from "./exportJobs.store.js";

/**
 * The export queue in the application bar (#51): what is exporting, what is
 * waiting, and what has been exported — each with what happened to it.
 *
 * The button says how many exports are under way, so a background export is
 * never invisible. Progress is the engine's, from the muxer; the bar only
 * draws it.
 */
export function ExportQueueButton() {
  const jobs = useExportJobs((state) => state.jobs);
  const active = jobs.filter(isActive).length;

  return (
    <Popover
      align="end"
      className="export-queue"
      trigger={
        <Button variant="ghost" size="sm" data-testid="export-queue-button">
          {active > 0 ? `Exports (${active})` : "Exports"}
        </Button>
      }
    >
      <ExportQueuePanel />
    </Popover>
  );
}

export function ExportQueuePanel() {
  const jobs = useExportJobs((state) => state.jobs);
  const error = useExportJobs((state) => state.error);
  const clearHistory = useExportJobs((state) => state.clearHistory);
  const current = jobs.filter((job) => !isFinished(job));
  // Newest first: the one just exported is the one looked for.
  const history = jobs.filter(isFinished).reverse();

  return (
    <div className="export-queue__panel" data-testid="export-queue">
      <h2 className="export-queue__heading">Exports</h2>
      {jobs.length === 0 ? (
        <p className="export-queue__empty">Nothing has been exported yet.</p>
      ) : null}
      {current.length > 0 ? (
        <ol className="export-queue__list">
          {current.map((job) => (
            <JobRow key={job.id} job={job} />
          ))}
        </ol>
      ) : null}
      {history.length > 0 ? (
        <>
          <div className="export-queue__history-head">
            <h3 className="export-queue__subheading">History</h3>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void clearHistory()}
            >
              Clear
            </Button>
          </div>
          <ol className="export-queue__list">
            {history.map((job) => (
              <JobRow key={job.id} job={job} />
            ))}
          </ol>
        </>
      ) : null}
      {error ? (
        <p className="export-queue__error" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function JobRow({ job }: { readonly job: ExportJob }) {
  const cancel = useExportJobs((state) => state.cancel);
  const resume = useExportJobs((state) => state.resume);
  const discard = useExportJobs((state) => state.discard);
  const state = job.state;

  return (
    <li
      className="export-queue__job"
      data-state={state.state}
      data-testid={`export-job-${job.id}`}
    >
      <div className="export-queue__job-head">
        <span className="export-queue__name" title={job.target}>
          {baseName(job.target)}
        </span>
        {isActive(job) ? (
          <Button variant="ghost" size="sm" onClick={() => void cancel(job.id)}>
            Cancel
          </Button>
        ) : null}
        {isInterrupted(job) ? (
          <>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void resume(job.id)}
            >
              Export again
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void discard(job.id)}
            >
              Discard
            </Button>
          </>
        ) : null}
      </div>
      {state.state === "running" ? (
        <progress
          className="export-queue__progress"
          // Indeterminate while preparing: how long a loudness measurement
          // takes is not known, and a bar that guesses is a bar that lies.
          {...(state.stage === "exporting"
            ? { value: state.fraction, max: 1 }
            : {})}
          aria-label={`Progress of ${baseName(job.target)}`}
        />
      ) : null}
      <p className="export-queue__status">{statusLine(job)}</p>
    </li>
  );
}

/**
 * Exports a crash interrupted, offered at launch: export again from the
 * start, or discard. Never "continue" — an export has no point it can
 * honestly resume from.
 */
export function InterruptedExportsBanner() {
  const jobs = useExportJobs((state) => state.jobs);
  const resume = useExportJobs((state) => state.resume);
  const discard = useExportJobs((state) => state.discard);
  const interrupted = jobs.filter(isInterrupted);
  if (interrupted.length === 0) return null;

  return (
    <aside
      className="export-banner"
      role="status"
      data-testid="interrupted-exports"
    >
      <p className="export-banner__text">
        {interrupted.length === 1
          ? "An export was interrupted when Blinkify closed."
          : `${interrupted.length} exports were interrupted when Blinkify closed.`}{" "}
        An export cannot continue from where it stopped; it can be exported
        again from the start.
      </p>
      <ul className="export-banner__list">
        {interrupted.map((job) => (
          <li key={job.id} className="export-banner__job">
            <strong title={job.target}>{baseName(job.target)}</strong>
            <Button
              variant="primary"
              size="sm"
              onClick={() => void resume(job.id)}
            >
              Export again
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void discard(job.id)}
            >
              Discard
            </Button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
