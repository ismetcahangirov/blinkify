import type { ExportOverview } from "@blinkify/types";
import { Button, Dialog, DialogClose, Select } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import { projectName, useProjectStore } from "../project/project.store.js";
import {
  CONTAINERS,
  PRESERVE,
  PRESETS,
  type Container,
  audioChoicesFor,
  containerOf,
  cutLine,
  duration,
  picturesLine,
  preservingContainer,
  sizeLine,
  soundLine,
  timecodeAt,
  withContainer,
} from "./exportChoices.js";
import { size } from "./exportJobs.js";
import { useExportJobs } from "./exportJobs.store.js";

/**
 * The export dialog (#50): where the product's value becomes visible.
 *
 * Every claim in it is the engine's (`export_overview`, from the plan of
 * #39): whether the pictures and the sound are copied, each reason one is
 * not with its timecode, the snap to keyframes and how far it moves each
 * cut, the size, and whether the drive has room. It is asked again whenever
 * the graph, the container or the sound target changes, so the answer is
 * always for what would be exported now.
 *
 * Resolution and frame rate are the sequence's (#57). Changing them there
 * shows what the change costs; the export has no second, silent way to
 * scale.
 */
export function ExportDialog({
  open,
  onOpenChange,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) {
  const hasProject = useProjectStore((state) => state.view !== null);
  // Mounted only while open: each opening asks the engine afresh.
  if (!open || !hasProject) return null;
  return <ExportDialogBody onOpenChange={onOpenChange} />;
}

function ExportDialogBody({
  onOpenChange,
}: {
  readonly onOpenChange: (open: boolean) => void;
}) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const firstSource = view
    ? Object.values(view.project.sources)[0]?.path
    : undefined;
  const [preset, setPreset] = useState(PRESERVE);
  const [container, setContainer] = useState<Container>(() =>
    preservingContainer(firstSource),
  );
  const [audio, setAudio] = useState("flac");
  // Chosen in the save dialog, which itself asks before replacing a file:
  // only that exact path may be replaced.
  const [target, setTarget] = useState<string | null>(null);
  const [confirmed, setConfirmed] = useState<string | null>(null);
  const [overview, setOverview] = useState<ExportOverview | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const choices = audioChoicesFor(container);
  const choice = choices.find((c) => c.id === audio) ?? choices[0];
  // Before a destination is chosen, the overview is asked for a name with
  // the container's extension, which is all it needs of it.
  const destination =
    target ?? withContainer(`${projectName(view)}.export`, container);

  // Ask the engine what this export would do, whenever anything it depends
  // on changes — the graph included, so a snap is reflected at once.
  useEffect(() => {
    if (!choice) return;
    let stale = false;
    invoke<ExportOverview>("export_overview", {
      target: destination,
      audio: choice.target,
    })
      .then((answer) => {
        if (stale) return;
        setOverview(answer);
        setProblem(null);
      })
      .catch((cause: unknown) => {
        if (stale) return;
        setOverview(null);
        setProblem(String(cause));
      });
    return () => {
      stale = true;
    };
  }, [destination, choice, view]);

  if (!view) return null;
  const frameRate = view.project.sequence.settings.frameRate;
  const settings = view.project.sequence.settings;

  const applyPreset = (id: string) => {
    const chosen = PRESETS.find((p) => p.id === id);
    if (!chosen) return;
    const next =
      chosen.container === "source"
        ? preservingContainer(firstSource)
        : chosen.container;
    setPreset(id);
    setContainer(next);
    setAudio(chosen.audio);
    if (target) setTarget(withContainer(target, next));
  };

  const chooseContainer = (value: string) => {
    const next = CONTAINERS.find((c) => c.value === value)?.value;
    if (!next) return;
    setPreset("custom");
    setContainer(next);
    if (!audioChoicesFor(next).some((c) => c.id === audio))
      setAudio(audioChoicesFor(next)[0]?.id ?? audio);
    if (target) setTarget(withContainer(target, next));
  };

  const chooseTarget = async () => {
    const path = await saveDialog({
      defaultPath: destination,
      filters: [{ name: container.toUpperCase(), extensions: [container] }],
    });
    if (!path) return;
    const chosen = containerOf(path) ? path : withContainer(path, container);
    const own = containerOf(chosen);
    if (own && own !== container) {
      setContainer(own);
      setPreset("custom");
    }
    setTarget(chosen);
    setConfirmed(chosen);
  };

  const snap = async () => {
    if (!overview?.snap) return;
    const refusal = await edit(overview.snap.edit);
    if (refusal) setProblem(refusal);
  };

  const replaceUnconfirmed =
    overview?.targetExists === true && confirmed !== destination;
  const blocked =
    target === null ||
    overview === null ||
    overview.problems.length > 0 ||
    replaceUnconfirmed ||
    busy;

  const start = async () => {
    if (!choice || target === null) return;
    setBusy(true);
    try {
      await invoke("submit_export", {
        target,
        overwrite: overview?.targetExists === true && confirmed === target,
        audio: choice.target,
      });
      await useExportJobs.getState().load();
      onOpenChange(false);
    } catch (cause) {
      setProblem(String(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open
      onOpenChange={onOpenChange}
      title="Export"
      description="What is copied and what is re-encoded is decided before anything is written, and said here."
      className="export-dialog"
      actions={
        <>
          <DialogClose>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <Button
            variant="primary"
            disabled={blocked}
            onClick={() => void start()}
            data-testid="start-export"
          >
            Export
          </Button>
        </>
      }
    >
      <div className="export-dialog__body">
        <section className="export-dialog__settings">
          <Select
            label="Preset"
            value={preset}
            onValueChange={applyPreset}
            options={[
              ...PRESETS.map((p) => ({ value: p.id, label: p.label })),
              { value: "custom", label: "Custom", disabled: true },
            ]}
          />
          <p className="export-dialog__note">
            {PRESETS.find((p) => p.id === preset)?.description ??
              "Your own choice of container and sound."}
          </p>
          <Select
            label="Container"
            value={container}
            onValueChange={chooseContainer}
            options={CONTAINERS.map((c) => ({
              value: c.value,
              label: c.label,
            }))}
          />
          <Select
            label="Sound that must be re-encoded"
            value={choice?.id ?? ""}
            onValueChange={(value) => {
              setPreset("custom");
              setAudio(value);
            }}
            options={choices.map((c) => ({ value: c.id, label: c.label }))}
          />
          <div className="export-dialog__destination">
            <span className="export-dialog__path" title={destination}>
              {target ?? "No destination chosen"}
            </span>
            <Button
              variant="secondary"
              size="sm"
              onClick={() => void chooseTarget()}
            >
              Choose…
            </Button>
          </div>
          <p className="export-dialog__note">
            {settings.width} × {settings.height} at{" "}
            {(settings.frameRate.num / settings.frameRate.den).toFixed(3)} fps:
            the sequence&apos;s own. Change them in Sequence settings, which
            says what the change costs.
          </p>
        </section>

        <section
          className="export-dialog__verdict"
          aria-live="polite"
          data-testid="export-verdict"
        >
          {overview === null ? (
            <p className="export-dialog__note">
              {problem ?? "Planning the export…"}
            </p>
          ) : (
            <Verdict
              overview={overview}
              frameRate={frameRate}
              onSnap={() => void snap()}
            />
          )}
          {replaceUnconfirmed ? (
            <p className="export-dialog__problem" role="alert">
              {destination} already exists. Choose it again with Choose… to
              replace it.
            </p>
          ) : null}
          {overview !== null && problem ? (
            <p className="export-dialog__problem" role="alert">
              {problem}
            </p>
          ) : null}
        </section>
      </div>
    </Dialog>
  );
}

function Verdict({
  overview,
  frameRate,
  onSnap,
}: {
  readonly overview: ExportOverview;
  readonly frameRate: { num: number; den: number };
  readonly onSnap: () => void;
}) {
  const snap = overview.snap;
  return (
    <>
      <p
        className="export-dialog__badge"
        data-state={overview.lossless ? "lossless" : "re-encoded"}
        data-testid="export-lossless"
      >
        {overview.lossless
          ? "Lossless: every packet is the source's"
          : "Not fully lossless"}
      </p>
      <ul className="export-dialog__claims">
        {overview.video ? <li>{picturesLine(overview.video)}</li> : null}
        {overview.audio ? (
          <li>{soundLine(overview.audio, overview.audioEncoding)}</li>
        ) : null}
      </ul>
      {overview.reasons.length > 0 ? (
        <ol className="export-dialog__reasons" data-testid="export-reasons">
          {overview.reasons.map((reason, index) => (
            <li
              key={index}
              data-declined={reason.declined ? "true" : undefined}
            >
              <span className="export-dialog__timecode">
                {timecodeAt(reason.atSeconds, frameRate)}
              </span>{" "}
              {reason.media === "video" ? "Pictures" : "Sound"}:{" "}
              {reason.sentence}
            </li>
          ))}
        </ol>
      ) : null}
      {snap && snap.cuts.length > 0 ? (
        <div className="export-dialog__snap" data-testid="snap-offer">
          <p>
            Move {snap.cuts.length === 1 ? "this cut" : "these cuts"} to the
            nearest keyframes and the pictures are copied with nothing
            re-encoded. The sequence becomes{" "}
            {Math.abs(snap.lengthChangeSeconds).toFixed(2)} s{" "}
            {snap.lengthChangeSeconds >= 0 ? "longer" : "shorter"}.
          </p>
          <ul>
            {snap.cuts.map((cut) => (
              <li key={cut.clip}>{cutLine(cut, frameRate)}</li>
            ))}
          </ul>
          {snap.unsnappable > 0 ? (
            <p className="export-dialog__note">
              {snap.unsnappable === 1
                ? "1 cut is made by a clip covering it from above and cannot move."
                : `${snap.unsnappable} cuts are made by clips covering them from above and cannot move.`}
            </p>
          ) : null}
          <Button variant="secondary" size="sm" onClick={onSnap}>
            Snap to keyframes
          </Button>
        </div>
      ) : null}
      <dl className="export-dialog__facts">
        <dt>Duration</dt>
        <dd>{duration(overview.durationSeconds)}</dd>
        <dt>Size</dt>
        <dd>{sizeLine(overview.size)}</dd>
        {overview.space ? (
          <>
            <dt>Free on the drive</dt>
            <dd>{size(overview.space.available)}</dd>
          </>
        ) : null}
      </dl>
      {overview.problems.map((problem) => (
        <p key={problem} className="export-dialog__problem" role="alert">
          Cannot export: {problem}.
        </p>
      ))}
    </>
  );
}
