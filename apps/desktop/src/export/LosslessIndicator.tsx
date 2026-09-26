import type { ExportPlan } from "@blinkify/types";
import { Tooltip } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

import { useProjectStore } from "../project/project.store.js";
import { planSummary } from "./exportChoices.js";

/**
 * The lossless indicator in the application bar: the export plan in one
 * state and a count — `Lossless`, `2 seams`, `3 segments re-encoded`.
 *
 * The planner (#39) decides; this displays. It asks again after every
 * change to the graph, and while it has no plan — nothing on the timeline,
 * a source offline — it says the tier is not computed, never `Lossless`.
 * The export dialog (#50) says which parts and why.
 */
export function LosslessIndicator() {
  const view = useProjectStore((state) => state.view);
  const [plan, setPlan] = useState<ExportPlan | null>(null);
  const [reason, setReason] = useState<string | null>(null);

  useEffect(() => {
    if (!view) return;
    let stale = false;
    invoke<ExportPlan>("plan_export")
      .then((answer) => {
        if (stale) return;
        setPlan(answer);
        setReason(null);
      })
      .catch((cause: unknown) => {
        if (stale) return;
        setPlan(null);
        setReason(String(cause));
      });
    return () => {
      stale = true;
    };
  }, [view]);

  // A plan for a project that has since closed is no plan.
  const shown = view ? plan : null;
  const why = view ? reason : "no project is open";

  if (shown === null)
    return (
      <Tooltip
        label={`The export planner has no plan: ${why ?? "planning…"}. Blinkify will not claim a tier it has not computed.`}
        side="bottom"
      >
        <span
          className="shell__lossless"
          data-state="unknown"
          data-testid="lossless-indicator"
        >
          Export tier: not computed
        </span>
      </Tooltip>
    );

  const summary = planSummary(shown);
  return (
    <Tooltip
      label={
        summary.state === "lossless"
          ? "Every segment of the export is a stream copy: the output's packets are the source's."
          : "Open Export to see which parts are re-encoded, and why."
      }
      side="bottom"
    >
      <span
        className="shell__lossless"
        data-state={summary.state}
        data-testid="lossless-indicator"
      >
        {summary.text}
      </span>
    </Tooltip>
  );
}
