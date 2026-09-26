import type { LoudnessReport } from "@blinkify/types";
import { Button, NumberInput, Select, Switch } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { CEILING_MIN } from "./gain.js";
import {
  DEFAULT_NORMALISE,
  PRESETS,
  TARGET_MAX,
  TARGET_MIN,
  describeLoudness,
  normaliseOf,
  presetOf,
  reportStatement,
  sameNormalise,
  setNormalise,
  setSequenceLoudness,
  type NormaliseSettings,
} from "./loudness.js";
import { shared } from "./mixedValue.js";

type Scope = "clip" | "sequence";

/**
 * Loudness normalisation (#48): to a platform's target, for the selected
 * clips or for the whole sequence, with the level before and after in LUFS.
 *
 * Two passes: the sound is measured, then moved by one gain with a
 * true-peak limiter after it. "Whole sequence" measures the mix and moves
 * every clip by the same gain, so the levels between them are kept.
 */
export function LoudnessControl({ clips }: { clips: readonly Placement[] }) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const sequence = view?.project.sequence.loudness ?? null;
  const [scope, setScope] = useState<Scope>(sequence ? "sequence" : "clip");
  const ids = clips.map((clip) => clip.clip);

  const own = shared(clips.map(normaliseOf), sameNormalise);
  const mixed = own.kind === "mixed";
  const clipSettings = own.kind === "same" ? own.value : null;
  const current: NormaliseSettings | null =
    scope === "sequence"
      ? sequence && { ...sequence, bypassed: false }
      : clipSettings;

  const apply = (next: NormaliseSettings) =>
    void edit(
      scope === "sequence"
        ? setSequenceLoudness(next)
        : setNormalise(ids, next),
    );
  const remove = () =>
    void edit(
      scope === "sequence"
        ? setSequenceLoudness(null)
        : { edit: "reset-audio", clips: [...ids], stage: "normalise" },
    );

  return (
    <div className="gain-control">
      <Select
        label="Normalise"
        value={scope}
        onValueChange={(value) => {
          setScope(value === "sequence" ? "sequence" : "clip");
        }}
        options={[
          { value: "clip", label: "This clip on its own" },
          { value: "sequence", label: "The whole sequence, levels kept" },
        ]}
      />
      {current === null && !mixed ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            apply(DEFAULT_NORMALISE);
          }}
        >
          {scope === "sequence" ? "Normalise the sequence" : "Normalise"}
        </Button>
      ) : (
        <>
          <Select
            label="Target"
            value={current ? presetOf(current) : "custom"}
            onValueChange={(id) => {
              const preset = PRESETS.find((p) => p.id === id);
              if (preset)
                apply({
                  targetLufs: preset.targetLufs,
                  ceilingDbtp: preset.ceilingDbtp,
                  bypassed: current?.bypassed ?? false,
                });
            }}
            options={[
              ...PRESETS.map((preset) => ({
                value: preset.id,
                label: preset.label,
              })),
              { value: "custom", label: "Custom", disabled: true },
            ]}
          />
          <div className="gain-control__fields">
            <NumberInput
              label="Loudness"
              value={current?.targetLufs ?? DEFAULT_NORMALISE.targetLufs}
              mixed={mixed}
              min={TARGET_MIN}
              max={TARGET_MAX}
              step={0.5}
              precision={1}
              unit="LUFS"
              onValueChange={(targetLufs) => {
                apply({ ...(current ?? DEFAULT_NORMALISE), targetLufs });
              }}
            />
            <NumberInput
              label="Ceiling"
              value={current?.ceilingDbtp ?? DEFAULT_NORMALISE.ceilingDbtp}
              mixed={mixed}
              min={CEILING_MIN}
              max={0}
              step={0.1}
              precision={1}
              unit="dBTP"
              onValueChange={(ceilingDbtp) => {
                apply({ ...(current ?? DEFAULT_NORMALISE), ceilingDbtp });
              }}
            />
          </div>
          <div className="gain-control__row">
            {scope === "clip" ? (
              <Switch
                label="Bypass normalisation"
                checked={current?.bypassed ?? false}
                onCheckedChange={(bypassed) => {
                  apply({ ...(current ?? DEFAULT_NORMALISE), bypassed });
                }}
              />
            ) : null}
            <Button size="sm" variant="ghost" onClick={remove}>
              {scope === "sequence" ? "Stop normalising" : "Reset"}
            </Button>
          </div>
          <Report scope={scope} clips={clips} settings={current} />
        </>
      )}
    </div>
  );
}

/** What a report request came to, for the request it answers. */
interface Answer {
  readonly key: string;
  readonly state: ReportState;
}

/** Ask the engine for a report, and hand over the answer for `key` unless
 * the returned function was called first. */
function ask(
  setAnswer: (answer: Answer) => void,
  key: string,
  command: string,
  args: Record<string, unknown>,
): () => void {
  let live = true;
  invoke<LoudnessReport | null>(command, args)
    .then((report) => {
      if (live)
        setAnswer({
          key,
          state: report ? { kind: "ready", report } : { kind: "none" },
        });
    })
    .catch((cause: unknown) => {
      if (live)
        setAnswer({ key, state: { kind: "failed", reason: String(cause) } });
    });
  return () => {
    live = false;
  };
}

type ReportState =
  | { readonly kind: "idle" }
  | { readonly kind: "measuring" }
  | { readonly kind: "none" }
  | { readonly kind: "failed"; readonly reason: string }
  | { readonly kind: "ready"; readonly report: LoudnessReport };

/**
 * Before and after, in LUFS. A clip is measured as soon as it is shown —
 * once; the engine keeps the answer — and the sequence when asked, since
 * measuring it renders the whole mix. An answer is shown only for the
 * request it answers, so a stale one never stands for the current settings.
 */
function Report({
  scope,
  clips,
  settings,
}: {
  scope: Scope;
  clips: readonly Placement[];
  settings: NormaliseSettings | null;
}) {
  const [answer, setAnswer] = useState<Answer | null>(null);
  const [asked, setAsked] = useState<string | null>(null);
  const [single] = clips;
  const clip = clips.length === 1 ? single?.clip : undefined;
  const key = JSON.stringify([
    scope,
    clip,
    settings,
    single?.sourceIn,
    single?.sourceOut,
    single?.audio,
  ]);

  const wanted = scope === "clip" && settings !== null && clip !== undefined;
  useEffect(() => {
    if (!wanted || clip === undefined) return undefined;
    return ask(setAnswer, key, "loudness_report", { clip });
  }, [wanted, clip, key]);

  const state: ReportState =
    answer?.key === key
      ? answer.state
      : wanted || asked === key
        ? { kind: "measuring" }
        : { kind: "idle" };

  if (scope === "clip" && clips.length !== 1)
    return (
      <p className="inspector-note">
        Select one clip to see its loudness before and after.
      </p>
    );
  switch (state.kind) {
    case "idle":
      return scope === "sequence" ? (
        <Button
          size="sm"
          variant="ghost"
          onClick={() => {
            setAsked(key);
            ask(setAnswer, key, "sequence_loudness_report", {});
          }}
        >
          Measure the mix
        </Button>
      ) : null;
    case "measuring":
      return (
        <p className="inspector-note">
          {scope === "sequence"
            ? "Measuring the whole mix…"
            : "Measuring the clip…"}
        </p>
      );
    case "none":
      return <p className="inspector-note">There is no sound to measure.</p>;
    case "failed":
      return (
        <p className="inspector-warning" role="alert">
          The loudness could not be measured: {state.reason}
        </p>
      );
    case "ready":
      return (
        <>
          <dl className="inspector-facts">
            <dt>Before</dt>
            <dd>{describeLoudness(state.report.before)}</dd>
            <dt>After</dt>
            <dd>{describeLoudness(state.report.after)}</dd>
          </dl>
          <p className="inspector-statement" data-tone="lossless">
            {reportStatement(state.report)}
          </p>
        </>
      );
  }
}
