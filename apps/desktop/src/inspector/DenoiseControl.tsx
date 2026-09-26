import { Button, Slider, Switch } from "@blinkify/ui";
import { useEffect, useRef, useState } from "react";
import { useProjectStore } from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import {
  DEFAULT_STRENGTH,
  NO_DENOISE,
  denoiseOf,
  formatStrength,
  sameDenoise,
  setDenoise,
  type DenoiseSettings,
} from "./denoise.js";
import { editGesture, type EditGesture } from "./editGesture.js";
import { shared } from "./mixedValue.js";

/**
 * Noise reduction (#47): RNNoise's speech model, at a strength that blends
 * the denoised sound with the original.
 *
 * "Hear original" is the A/B comparison: it bypasses the step, keeping its
 * strength, and the preview switches on the next sample without stopping —
 * the way to hear whether the denoiser is helping or doing harm.
 */
export function DenoiseControl({ clips }: { clips: readonly Placement[] }) {
  const edit = useProjectStore((state) => state.edit);
  const [dragged, setDragged] = useState<number | null>(null);
  const gesture = useRef<EditGesture | null>(null);
  const committed = useRef<number | null>(null);

  const settings = shared(clips.map(denoiseOf), sameDenoise);
  const current: DenoiseSettings =
    settings.kind === "same" ? settings.value : NO_DENOISE;
  const mixed = settings.kind === "mixed";
  const ids = clips.map((clip) => clip.clip);
  const off = !mixed && current.strength === 0;

  const during = (next: DenoiseSettings) => {
    gesture.current ??= editGesture("Change noise reduction");
    gesture.current.change(setDenoise(ids, next));
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

  return (
    <div className="gain-control">
      <Slider
        label="Strength"
        min={0}
        max={100}
        step={1}
        value={dragged ?? Math.round(current.strength * 100)}
        onValueChange={(percent) => {
          if (gesture.current === null && committed.current === percent) {
            committed.current = null;
            return;
          }
          committed.current = null;
          setDragged(percent);
          during({ ...current, strength: percent / 100 });
        }}
        onValueCommit={(percent) => {
          during({ ...current, strength: percent / 100 });
          committed.current = percent;
          finish();
        }}
        formatValue={(percent) =>
          mixed && dragged === null ? "Mixed" : formatStrength(percent / 100)
        }
      />
      <div className="gain-control__row">
        {off ? (
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              void edit(
                setDenoise(ids, {
                  strength: DEFAULT_STRENGTH,
                  bypassed: false,
                }),
              )
            }
          >
            Reduce noise
          </Button>
        ) : (
          <Switch
            label="Hear original"
            checked={current.bypassed}
            onCheckedChange={(bypassed) =>
              void edit(setDenoise(ids, { ...current, bypassed }))
            }
          />
        )}
        <Button
          size="sm"
          variant="ghost"
          disabled={off}
          onClick={() =>
            void edit({
              edit: "reset-audio",
              clips: [...ids],
              stage: "denoise",
            })
          }
        >
          Reset
        </Button>
      </div>
      <p className="inspector-note">
        A speech denoiser, for voices over fans, air conditioning and computers.
        On music it removes what it takes for noise: compare with the original
        before keeping it. Full strength on clean speech can sound underwater.
      </p>
    </div>
  );
}
