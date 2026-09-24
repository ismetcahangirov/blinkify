import type { DiagnosticOperation, Diagnostics } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

/** How often the panel asks what applies under the playhead. */
const REFRESH_MS = 250;

/**
 * What the evaluator applies under the playhead (#30).
 *
 * The same list the export will apply there: the engine evaluates the graph
 * once for both, and this panel only shows the answer — it never reads the
 * graph itself (`pnpm evaluator:check`). It is how someone finds out why a
 * clip will be re-encoded, so it names each operation plainly, and says when
 * the preview does not render one yet rather than letting the sound imply
 * the export will not apply it either.
 */
export function OperationsPanel({ session }: { session: number }) {
  const [diagnostics, setDiagnostics] = useState<Diagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const refresh = async () => {
      try {
        const next = await invoke<Diagnostics>("operations_at", { session });
        if (live) {
          setDiagnostics(next);
          setError(null);
        }
      } catch (cause) {
        if (live) setError(String(cause));
      }
    };
    void refresh();
    const timer = setInterval(() => {
      void refresh();
    }, REFRESH_MS);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [session]);

  return (
    <section
      className="player__operations"
      aria-label="Operations at the playhead"
      data-testid="operations-panel"
    >
      {error !== null && <p className="player__error">{error}</p>}
      {diagnostics !== null && diagnostics.clips.length === 0 && (
        <p className="zone__placeholder">
          Nothing on the timeline at frame {diagnostics.position}.
        </p>
      )}
      {diagnostics?.clips.map((clip) => (
        <div key={`${clip.track}-${clip.clip}`} data-testid="operations-clip">
          <p className="player__operations-clip">
            Track {clip.track} · {clip.sourceName}
          </p>
          <ol className="player__operations-list">
            {clip.applied.map((step, index) => (
              <li key={index}>
                {describe(step)}
                {!step.previewed && (
                  <span className="player__operations-note">
                    {" "}
                    — applied on export, not heard in preview yet
                  </span>
                )}
              </li>
            ))}
          </ol>
        </div>
      ))}
    </section>
  );
}

/** One operation, in words. */
export function describe({ operation }: DiagnosticOperation): string {
  switch (operation.op) {
    case "trim":
      return `Trim: source ${String(operation.from)} to ${String(operation.to)}`;
    case "speed":
      return `Speed ×${formatRatio(operation.ratio.num, operation.ratio.den)}`;
    case "gain":
      return `Gain ${signed(operation.db)} dB`;
    case "denoise":
      return `Denoise ${String(Math.round(operation.strength * 100))} %`;
    case "normalise":
      return `Normalise to ${minus(operation.targetLufs)} LUFS`;
    case "freeze":
      return `Freeze frame: ${String(operation.frames)} frames (re-encoded)`;
    case "reverse":
      return "Reverse (re-encoded)";
  }
}

function formatRatio(num: number, den: number): string {
  const value = num / den;
  return Number.isInteger(value) ? String(value) : value.toFixed(3);
}

/** One decimal, with a typographic minus. */
function minus(value: number): string {
  const text = Math.abs(value).toFixed(1);
  return value < 0 ? `−${text}` : text;
}

function signed(value: number): string {
  return value < 0 ? minus(value) : `+${minus(value)}`;
}
