import { useEffect, useRef } from "react";

import { useFileDrop } from "../player/useFileDrop.js";
import { fileName, useProjectStore } from "./project.store.js";

/**
 * What opening the project found wrong with its sources, and the way out.
 *
 * `CLAUDE.md` section 17: the user never has to guess. A source that has
 * moved is named, with how many clips it takes offline, and the fix is on the
 * banner itself: drop the file there and the engine checks it is the same
 * content before pointing the project at it. A file that has changed in place
 * is said to have changed — relinking cannot make it the file the edit was
 * made on.
 *
 * Also where a launch project that could not be opened at all says why.
 */
export function SourcesBanner() {
  const view = useProjectStore((state) => state.view);
  const error = useProjectStore((state) => state.error);
  const relinkError = useProjectStore((state) => state.relinkError);
  const loadLaunch = useProjectStore((state) => state.loadLaunch);
  const relink = useProjectStore((state) => state.relink);
  const target = useRef<HTMLElement>(null);

  useEffect(() => {
    void loadLaunch();
  }, [loadLaunch]);

  const unavailable = view
    ? Object.entries(view.unavailable).flatMap(([id, status]) =>
        status ? [{ id: Number(id), status }] : [],
      )
    : [];
  const missing = unavailable.find(({ status }) => status.state === "missing");

  useFileDrop(target, (path) => {
    if (missing) void relink(missing.id, path);
  });

  if (error) {
    return (
      <aside
        className="sources-banner"
        role="alert"
        data-testid="project-error"
      >
        <p className="sources-banner__text">
          The project could not be opened: {error}.
        </p>
      </aside>
    );
  }
  if (!view || unavailable.length === 0) return null;

  const clips = view.affectedClips.length;
  return (
    <aside
      ref={target}
      className="sources-banner"
      role="status"
      data-testid="sources-banner"
    >
      <p className="sources-banner__text">
        {clips} {clips === 1 ? "clip is" : "clips are"} offline.
      </p>
      <ul className="sources-banner__list">
        {unavailable.map(({ id, status }) => {
          const path = view.project.sources[id]?.path ?? "";
          return (
            <li key={id} data-testid={`source-${id}`}>
              <strong>{fileName(path)}</strong>{" "}
              {status.state === "changed"
                ? "has changed since the project was saved."
                : `is not at ${path}.`}
            </li>
          );
        })}
      </ul>
      {missing && (
        <p className="sources-banner__hint">
          Drop the moved file of{" "}
          <strong>
            {fileName(view.project.sources[missing.id]?.path ?? "")}
          </strong>{" "}
          here to relink it.
        </p>
      )}
      {relinkError && (
        <p className="sources-banner__error" data-testid="relink-error">
          Not relinked: {relinkError}.
        </p>
      )}
    </aside>
  );
}
