import type { GainAdvice } from "@blinkify/types";
import { Button, NumberInput, Slider, Switch } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import { LevelMeter } from "../player/LevelMeter.js";
import { usePreviewStore } from "../player/preview.store.js";
import { DenoiseControl } from "./DenoiseControl.js";
import { LoudnessControl } from "./LoudnessControl.js";
import { editGesture, type EditGesture } from "./editGesture.js";
import {
  CEILING_MIN,
  GAIN_MAX,
  GAIN_MIN,
  NO_GAIN,
  formatDb,
  gainOf,
  limiterStatement,
  measuredLevel,
  sameGain,
  setGain,
  suggestionText,
  type GainSettings,
} from "./gain.js";
import { shared } from "./mixedValue.js";

/**
 * The audio section (#46–#49): what the selected clips' sound goes
 * through, in the order the chain runs it — noise reduction, gain, loudness
 * — with a bypass for each step and for the whole chain, the level after the
 * chain, and a statement that none of it touches the pictures.
 *
 * Every change is heard while playing, without a gap: an edit that changes
 * only audio chains retunes the running preview (ADR-0012).
 *
 * A view of the selection and of the graph, like the video section: every
 * control sends an edit, and a drag is one gesture and one undo entry.
 */
export function AudioInspector({ clips }: { clips: readonly Placement[] }) {
  const edit = useProjectStore((state) => state.edit);
  const ids = clips.map((clip) => clip.clip);
  const untouched = clips.every((clip) => clip.audio.length === 0);
  const steps = clips.flatMap((clip) => clip.audio);
  const allBypassed = steps.length > 0 && steps.every((step) => step.bypassed);
  const session = usePreviewStore((state) => state.session);

  return (
    <div className="audio-inspector">
      <section className="inspector-section" aria-labelledby="inspector-audio">
        <header className="inspector-section__header">
          <h3 id="inspector-audio" className="inspector-section__title">
            {clips.length === 1
              ? "Audio"
              : `Audio of ${String(clips.length)} clips`}
          </h3>
          <Button
            size="sm"
            variant="ghost"
            disabled={untouched}
            onClick={() =>
              void edit({ edit: "reset-audio", clips: [...ids], stage: null })
            }
          >
            Reset audio
          </Button>
        </header>
        <p className="inspector-note">
          Audio changes re-encode the sound only. The video stream is still
          copied, bit for bit.
        </p>
        <Switch
          label="Bypass the whole chain"
          checked={allBypassed}
          disabled={steps.length === 0}
          onCheckedChange={(bypassed) =>
            void edit({ edit: "bypass-audio", clips: [...ids], bypassed })
          }
        />
        {session === null ? null : (
          <div className="audio-inspector__meter">
            <span className="inspector-note">Output, after the chain</span>
            <div className="player__monitor">
              <LevelMeter />
            </div>
          </div>
        )}
      </section>

      <section
        className="inspector-section"
        aria-labelledby="inspector-denoise"
      >
        <h3 id="inspector-denoise" className="inspector-section__title">
          Noise reduction
        </h3>
        <DenoiseControl clips={clips} />
      </section>

      <section className="inspector-section" aria-labelledby="inspector-gain">
        <h3 id="inspector-gain" className="inspector-section__title">
          Gain
        </h3>
        <GainControl clips={clips} />
      </section>

      <section
        className="inspector-section"
        aria-labelledby="inspector-loudness"
      >
        <h3 id="inspector-loudness" className="inspector-section__title">
          Loudness
        </h3>
        <LoudnessControl clips={clips} />
      </section>
    </div>
  );
}

/**
 * Gain in decibels, the limiter's ceiling, and what the limiter is doing.
 * The limiter is part of the gain: it exists to catch what the gain pushes
 * over the ceiling, and it says when it does.
 */
function GainControl({ clips }: { clips: readonly Placement[] }) {
  const edit = useProjectStore((state) => state.edit);
  const [dragged, setDragged] = useState<number | null>(null);
  const gesture = useRef<EditGesture | null>(null);
  // As in the speed control: a slider commit is followed by an echo of the
  // same position, which is not a new drag.
  const committed = useRef<number | null>(null);

  const settings = shared(clips.map(gainOf), sameGain);
  const current: GainSettings =
    settings.kind === "same" ? settings.value : NO_GAIN;
  const mixed = settings.kind === "mixed";
  const ids = clips.map((clip) => clip.clip);

  const during = (next: GainSettings) => {
    gesture.current ??= editGesture("Change gain");
    gesture.current.change(setGain(ids, next));
  };
  const finish = () => {
    const open = gesture.current;
    gesture.current = null;
    setDragged(null);
    if (open) void open.end();
  };
  useEffect(
    () => () => {
      const open = gesture.current;
      gesture.current = null;
      if (open) void open.end();
    },
    [],
  );

  const [single] = clips;
  const advice = useGainAdvice(clips.length === 1 ? single : undefined);

  return (
    <div className="gain-control">
      <Slider
        label="Gain"
        min={GAIN_MIN}
        max={GAIN_MAX}
        step={0.1}
        value={dragged ?? current.db}
        onValueChange={(db) => {
          if (gesture.current === null && committed.current === db) {
            committed.current = null;
            return;
          }
          committed.current = null;
          setDragged(db);
          during({ ...current, db });
        }}
        onValueCommit={(db) => {
          during({ ...current, db });
          committed.current = db;
          finish();
        }}
        formatValue={(db) =>
          mixed && dragged === null ? "Mixed" : `${formatDb(db)} dB`
        }
      />
      <div
        className="gain-control__fields"
        onBlur={finish}
        onPointerUp={finish}
        onKeyDown={(event) => {
          if (event.key === "Enter") finish();
        }}
      >
        <NumberInput
          label="Level"
          value={current.db}
          mixed={mixed}
          min={GAIN_MIN}
          max={GAIN_MAX}
          step={0.1}
          precision={1}
          unit="dB"
          onValueChange={(db) => {
            during({ ...current, db });
          }}
        />
        <NumberInput
          label="Ceiling"
          value={current.ceilingDbtp}
          mixed={mixed}
          min={CEILING_MIN}
          max={0}
          step={0.1}
          precision={1}
          unit="dBTP"
          disabled={current.db === 0}
          onValueChange={(ceilingDbtp) => {
            during({ ...current, ceilingDbtp });
          }}
        />
      </div>
      <div className="gain-control__row">
        <Switch
          label="Bypass gain"
          checked={current.bypassed}
          disabled={current.db === 0}
          onCheckedChange={(bypassed) =>
            void edit(setGain(ids, { ...current, bypassed }))
          }
        />
        <Button
          size="sm"
          variant="ghost"
          disabled={!mixed && current.db === 0}
          onClick={() =>
            void edit({ edit: "reset-audio", clips: [...ids], stage: "gain" })
          }
        >
          Reset gain
        </Button>
      </div>
      <AdviceView
        clips={clips.length}
        advice={advice}
        onApply={(db) => void edit(setGain(ids, { ...current, db }))}
      />
    </div>
  );
}

/** The clip's level, what the limiter is doing, and the suggestion. */
function AdviceView({
  clips,
  advice,
  onApply,
}: {
  clips: number;
  advice: AdviceState;
  onApply: (db: number) => void;
}) {
  if (clips !== 1)
    return (
      <p className="inspector-note">
        Select one clip to see its level and what the limiter is doing.
      </p>
    );
  switch (advice.kind) {
    case "none":
    case "measuring":
      return <p className="inspector-note">Measuring the clip&apos;s level…</p>;
    case "failed":
      return (
        <p className="inspector-warning" role="alert">
          The clip&apos;s level could not be measured: {advice.reason}
        </p>
      );
    case "silent":
      return <p className="inspector-note">The clip has no sound.</p>;
    case "ready": {
      const { suggestedDb } = advice.advice;
      return (
        <>
          <dl className="inspector-facts">
            <dt>Before gain</dt>
            <dd>{measuredLevel(advice.advice)}</dd>
          </dl>
          <p
            className="inspector-statement"
            data-tone={advice.advice.limitingDb > 0 ? "re-encode" : "lossless"}
          >
            {limiterStatement(advice.advice)}
          </p>
          <div className="gain-control__row">
            <p className="inspector-note">
              Suggested: {suggestionText(advice.advice)}
            </p>
            {suggestedDb === null ? null : (
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  onApply(suggestedDb);
                }}
              >
                Apply
              </Button>
            )}
          </div>
        </>
      );
    }
  }
}

type AdviceState =
  | { readonly kind: "none" }
  | { readonly kind: "measuring" }
  | { readonly kind: "silent" }
  | { readonly kind: "failed"; readonly reason: string }
  | { readonly kind: "ready"; readonly advice: GainAdvice };

/**
 * The engine's advice for `clip`, asked again whenever the clip's range or
 * audio chain changes. The first answer for a stretch decodes it; the rest
 * come from the engine's cache.
 */
function useGainAdvice(clip: Placement | undefined): AdviceState {
  const [state, setState] = useState<AdviceState>({ kind: "none" });
  const id = clip?.clip;
  const key = clip
    ? JSON.stringify([clip.sourceIn, clip.sourceOut, clip.audio])
    : "";
  useEffect(() => {
    if (id === undefined) return;
    let live = true;
    invoke<GainAdvice | null>("gain_advice", { clip: id })
      .then((advice) => {
        if (live)
          setState(advice ? { kind: "ready", advice } : { kind: "silent" });
      })
      .catch((cause: unknown) => {
        if (live) setState({ kind: "failed", reason: String(cause) });
      });
    return () => {
      live = false;
    };
  }, [id, key]);
  return state;
}
