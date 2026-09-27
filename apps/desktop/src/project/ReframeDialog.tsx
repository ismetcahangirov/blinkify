import type { Aspect, ReframeImpact } from "@blinkify/types";
import { Button, Dialog, DialogClose, Select } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { fileName, useProjectStore } from "./project.store.js";
import { REFRAME_ASPECTS, reframeStatement } from "./reframe.js";
import { displayAspect } from "./SequenceSettingsDialog.js";

/**
 * Reframe (#132): turn the sequence to another aspect ratio in one step,
 * with each clip's crop placed — and, before anything is applied, what it
 * costs.
 *
 * The engine decides everything: the size, each crop, which clips are left
 * (`preview_reframe`, the plans before and after). The dialog states it and
 * applies it as one edit, so one undo takes it all back (ADR-0022). Each
 * crop can then be moved with the ordinary crop controls.
 */
export function ReframeDialog({
  open,
  onOpenChange,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) {
  const hasProject = useProjectStore((state) => state.view !== null);
  // Mounted only while open, so each opening asks afresh.
  if (!open || !hasProject) return null;
  return <ReframeBody onOpenChange={onOpenChange} />;
}

function ReframeBody({
  onOpenChange,
}: {
  readonly onOpenChange: (open: boolean) => void;
}) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const current = view ? displayAspect(view.project.sequence.settings) : null;
  // The first shape the sequence is not already.
  const [aspect, setAspect] = useState<Aspect>(
    () =>
      REFRAME_ASPECTS.find((option) => option.value !== current)?.value ??
      "9:16",
  );
  // The engine's answer, for the shape it was asked about: a statement from
  // one shape is never shown under another.
  const [answer, setAnswer] = useState<{
    readonly aspect: Aspect;
    readonly impact: ReframeImpact | null;
    readonly problem: string | null;
  } | null>(null);
  const asked = answer?.aspect === aspect ? answer : null;
  const impact = asked?.impact ?? null;
  const problem = asked?.problem ?? null;

  // Ask the engine what the reframe would do and cost, before it is applied.
  useEffect(() => {
    let stale = false;
    invoke<ReframeImpact>("preview_reframe", { aspect })
      .then((reply) => {
        if (!stale) setAnswer({ aspect, impact: reply, problem: null });
      })
      .catch((cause: unknown) => {
        if (!stale) setAnswer({ aspect, impact: null, problem: String(cause) });
      });
    return () => {
      stale = true;
    };
  }, [aspect]);

  if (!view) return null;

  const sourceName = (source: number) => {
    const path = view.project.sources[source]?.path;
    return path ? fileName(path) : `source ${String(source)}`;
  };

  const apply = async () => {
    const refusal = await edit({ edit: "reframe", aspect });
    if (refusal) setAnswer({ aspect, impact, problem: refusal });
    else onOpenChange(false);
  };

  return (
    <Dialog
      open
      onOpenChange={onOpenChange}
      title="Reframe"
      description="Turn the sequence to another shape, with each clip's crop centred. Cropping re-encodes a clip's pictures; a clip already of the shape stays a copy."
      actions={
        <>
          <DialogClose>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <Button
            variant="primary"
            disabled={impact === null || problem !== null}
            onClick={() => void apply()}
          >
            Reframe
          </Button>
        </>
      }
    >
      <div className="sequence-settings">
        <Select
          label="Shape"
          value={aspect}
          onValueChange={(value) => {
            const option = REFRAME_ASPECTS.find((o) => o.value === value);
            if (option) setAspect(option.value);
          }}
          options={REFRAME_ASPECTS.map((option) => ({
            value: option.value,
            label: option.label,
          }))}
        />
        <p className="sequence-settings__note">
          The sequence is {current ?? "unknown"} now. Undo takes the whole
          reframe back in one step.
        </p>
        <div
          className="sequence-settings__impact"
          role="status"
          data-testid="reframe-impact"
          data-state={
            problem
              ? "refused"
              : impact && impact.reEncodedClips.length > 0
                ? "costly"
                : "ok"
          }
        >
          {problem ? (
            <p className="sequence-settings__line">Cannot reframe: {problem}</p>
          ) : impact ? (
            reframeStatement(impact, sourceName).map((line) => (
              <p key={line} className="sequence-settings__line">
                {line}
              </p>
            ))
          ) : (
            <p className="sequence-settings__line">Asking the export plan…</p>
          )}
        </div>
      </div>
    </Dialog>
  );
}
