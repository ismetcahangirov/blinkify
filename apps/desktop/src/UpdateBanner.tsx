import { useEffect } from "react";

import { useUpdateStore } from "./update.store.js";

/**
 * The update offer, stated rather than acted on.
 *
 * `CLAUDE.md` section 17: a user must never have to guess what the application
 * did. So this names both versions — the one running and the one on offer —
 * rather than saying "an update is available" and leaving the person to work
 * out whether it matters.
 *
 * Deliberately plain. The design system is Epic #2 (#15 – #17), and styling
 * this against tokens that do not exist yet would mean styling it twice.
 */
export function UpdateBanner() {
  const status = useUpdateStore((state) => state.status);
  const offer = useUpdateStore((state) => state.offer);
  const error = useUpdateStore((state) => state.error);
  const refresh = useUpdateStore((state) => state.refresh);
  const install = useUpdateStore((state) => state.install);
  const dismiss = useUpdateStore((state) => state.dismiss);

  useEffect(() => {
    // Reads state the shell already holds. The one network call this
    // application is allowed happened at launch, in the engine, and is over by
    // now — see forbidden behaviour 8.
    void refresh();
  }, [refresh]);

  if (status === "idle" || status === "dismissed" || !offer) return null;

  return (
    <aside className="update-banner" role="status" data-testid="update-banner">
      <p className="update-banner__text">
        Blinkify <strong>{offer.version}</strong> is available. You are running{" "}
        <strong>{offer.currentVersion}</strong>.
      </p>

      {status === "failed" && (
        <p className="update-banner__error" data-testid="update-error">
          The update was not installed: {error ?? "reason unknown"}. Blinkify is
          unchanged.
        </p>
      )}

      <div className="update-banner__actions">
        <button
          type="button"
          onClick={() => {
            void install();
          }}
          disabled={status === "installing"}
        >
          {status === "installing" ? "Installing…" : "Install and restart"}
        </button>
        <button
          type="button"
          onClick={dismiss}
          disabled={status === "installing"}
        >
          Not now
        </button>
      </div>
    </aside>
  );
}
