import type { ClipsCost, CropFrame, CropRect } from "@blinkify/types";
import { Button, NumberInput, Select } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  useProjectStore,
  type DeepReadonly,
} from "../project/project.store.js";
import type { Placement } from "../timeline/draw.js";
import {
  ASPECTS,
  SIDES,
  costStatement,
  presetOf,
  shownRect,
  sideEdit,
  sideOf,
  type Side,
} from "./crop.js";
import { useCropFramingStore } from "../player/cropFraming.store.js";
import { usePreviewStore } from "../player/preview.store.js";
import { editGesture, type EditGesture } from "./editGesture.js";
import { shared } from "./mixedValue.js";

/**
 * The crop section (#130): the layout reference's Transform section, holding
 * the one transform that exists. Left, top, width and height in the source's
 * pixels as displayed; the presets the engine fits; a reset; and what the
 * crop costs, from the plan, where the crop is made.
 *
 * - **Every change is one undoable edit.** A preset or a reset is one edit;
 *   a field typed, stepped or scrubbed by its label is one gesture (#37).
 * - **Several clips edit together.** A side they agree on shows its value,
 *   one they do not shows as mixed; changing a side sets that side on every
 *   selected clip and leaves each one's others (`set-crop-sides`).
 * - **The numbers are the engine's.** A preset is fitted and rounded to the
 *   source's grid by the engine; the fields step on that grid; a value the
 *   engine refuses is shown at the field it came from, with its reason.
 * - **The cost is the plan's**, asked again after every edit as the
 *   application bar's indicator is, so it returns to "copied bit for bit"
 *   the moment the crop is reset.
 *
 * No scale or rotation fields: they are not built, and a field for a thing
 * that does not exist would be a lie.
 */
export function CropSection({ clips }: { clips: readonly Placement[] }) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const [refusal, setRefusal] = useState<{
    readonly where: Side | "aspect" | "reset";
    readonly reason: string;
    /** The selection it was made in: it is shown for that one only. */
    readonly selected: string;
  } | null>(null);
  const gesture = useRef<EditGesture | null>(null);
  const framing = useCropFramingStore((state) => state.clip);
  const frame = useCropFramingStore((state) => state.enter);
  const leaveFraming = useCropFramingStore((state) => state.leave);
  const previewing = usePreviewStore((state) => state.kind === "project");
  // Framing on the preview is one clip at a time (#131).
  const single = clips.length === 1 ? clips[0] : undefined;

  const ids = clips.map((clip) => clip.clip);
  const selected = ids.join(",");
  const frames = clips.map((clip) => view?.frames[clip.source]);
  const known = frames.every((frame) => frame !== undefined);
  const cropped = clips.some((clip) => clip.crop !== undefined);
  const rects = clips.map((clip, index) => shownRect(clip.crop, frames[index]));

  const during = (side: Side, value: number) => {
    gesture.current ??= editGesture("Crop clip", (reason, sent) => {
      const where = sideOf(sent) ?? side;
      setRefusal(reason === null ? null : { where, reason, selected });
    });
    gesture.current.change(sideEdit(ids, side, value));
  };
  const finish = () => {
    const open = gesture.current;
    gesture.current = null;
    if (open) void open.end();
  };
  // Unmounted mid-drag, the gesture it opened is still closed.
  useEffect(
    () => () => {
      const open = gesture.current;
      gesture.current = null;
      if (open) void open.end();
    },
    [],
  );

  const once = async (
    where: "aspect" | "reset",
    sent: Parameters<typeof edit>[0],
  ) => {
    const reason = await edit(sent);
    setRefusal(reason === null ? null : { where, reason, selected });
  };

  const aspect = shared(
    rects.map((rect, index) => {
      const frame = frames[index];
      return rect && frame ? presetOf(rect, frame) : "free";
    }),
  );
  const cost = useClipCost(ids);
  const statement = cost.kind === "ready" ? costStatement(cost.cost) : null;
  const refusalAt = (where: Side | "aspect" | "reset") =>
    refusal?.where === where && refusal.selected === selected ? (
      <p className="inspector-warning" role="alert">
        {refusal.reason}
      </p>
    ) : null;

  return (
    <section className="inspector-section" aria-labelledby="inspector-crop">
      <header className="inspector-section__header">
        <h3 id="inspector-crop" className="inspector-section__title">
          Crop
        </h3>
        {single && known && previewing ? (
          <Button
            size="sm"
            variant="ghost"
            aria-pressed={framing === single.clip}
            onClick={() =>
              void (framing === single.clip ? leaveFraming() : frame(single))
            }
          >
            {framing === single.clip ? "Done framing" : "Frame on preview"}
          </Button>
        ) : null}
        <Button
          size="sm"
          variant="ghost"
          disabled={!cropped}
          onClick={() =>
            void once("reset", { edit: "reset-crop", clips: [...ids] })
          }
        >
          Reset crop
        </Button>
      </header>
      {refusalAt("reset")}
      {known ? null : (
        <p className="inspector-warning" role="status">
          {clips.length === 1 ? "The clip's" : "A selected clip's"} source is
          offline or could not be read, so its picture size is not known. Relink
          it to crop.
        </p>
      )}
      <Select
        label="Aspect"
        disabled={!known}
        value={aspect.kind === "same" ? aspect.value : "mixed"}
        onValueChange={(value) => {
          const chosen = ASPECTS.find((option) => option.value === value);
          if (chosen && chosen.value !== "free")
            void once("aspect", {
              edit: "crop-to-aspect",
              clips: [...ids],
              aspect: chosen.value,
            });
        }}
        options={[
          ...ASPECTS.map((option) => ({
            value: option.value,
            label: option.label,
            // Free is what the sides are when they match no preset: it is
            // reached by typing them, not chosen.
            disabled: option.value === "free",
          })),
          ...(aspect.kind === "mixed"
            ? [{ value: "mixed", label: "Mixed", disabled: true }]
            : []),
        ]}
      />
      {refusalAt("aspect")}
      <div
        className="crop-section__fields"
        onBlur={finish}
        onPointerUp={finish}
        onKeyDown={(event) => {
          if (event.key === "Enter") finish();
        }}
      >
        {SIDES.map(({ side, label }) => (
          <SideField
            key={side}
            side={side}
            label={label}
            clips={clips}
            rects={rects}
            frames={frames}
            disabled={!known}
            onChange={(value) => during(side, value)}
          >
            {refusalAt(side)}
          </SideField>
        ))}
      </div>
      <CostView state={cost} statement={statement} />
    </section>
  );
}

type Frames = readonly (DeepReadonly<CropFrame> | undefined)[];

/** One side, over every selected clip: its value where they agree, "Mixed"
 * where they do not, stepping on the coarsest grid among their sources. */
function SideField({
  side,
  label,
  clips,
  rects,
  frames,
  disabled,
  onChange,
  children,
}: {
  readonly side: Side;
  readonly label: string;
  readonly clips: readonly Placement[];
  readonly rects: readonly (DeepReadonly<CropRect> | null)[];
  readonly frames: Frames;
  readonly disabled: boolean;
  readonly onChange: (value: number) => void;
  readonly children: ReactNode;
}) {
  const values = rects.map((rect) => rect?.[side] ?? 0);
  const value = shared(values);
  const known = frames.filter((frame) => frame !== undefined);
  const across = side === "x" || side === "width";
  const step = Math.max(
    1,
    ...known.map((frame) => (across ? frame.across : frame.down)),
  );
  const smallest = Math.max(1, ...known.map((frame) => frame.minSize));
  const extent = Math.min(
    ...known.map((frame) => (across ? frame.width : frame.height)),
  );
  const sized = side === "width" || side === "height";
  return (
    <div className="crop-section__field">
      <NumberInput
        label={label}
        value={value.kind === "same" ? value.value : (values[0] ?? 0)}
        mixed={value.kind === "mixed" && clips.length > 1}
        min={sized ? smallest : 0}
        {...(Number.isFinite(extent)
          ? { max: sized ? extent : extent - smallest }
          : {})}
        step={step}
        unit="px"
        disabled={disabled}
        onValueChange={onChange}
      />
      {children}
    </div>
  );
}

type CostState =
  | { readonly kind: "asking" }
  | { readonly kind: "ready"; readonly cost: DeepReadonly<ClipsCost> }
  | { readonly kind: "failed"; readonly reason: string };

/** The statement at the point of use, or why there is none yet. */
function CostView({
  state,
  statement,
}: {
  readonly state: CostState;
  readonly statement: ReturnType<typeof costStatement>;
}) {
  if (state.kind === "failed")
    return (
      <p className="inspector-note" data-testid="crop-cost">
        The cost is not stated: {state.reason}. Blinkify will not claim a tier
        it has not computed.
      </p>
    );
  if (state.kind === "asking" || statement === null)
    return (
      <p className="inspector-note" data-testid="crop-cost">
        Asking the export plan…
      </p>
    );
  return (
    <p
      className="inspector-statement"
      data-tone={statement.tone}
      data-testid="crop-cost"
    >
      {statement.text}
    </p>
  );
}

/**
 * What the export does to `clips`, asked of the engine's plan again after
 * every change to the graph — the crop's own edits included.
 */
function useClipCost(clips: readonly number[]): CostState {
  const view = useProjectStore((state) => state.view);
  const [state, setState] = useState<CostState>({ kind: "asking" });
  const key = clips.join(",");
  useEffect(() => {
    if (!view) return;
    let live = true;
    invoke<ClipsCost | undefined>("clip_cost", {
      clips: key.split(",").map(Number),
    })
      .then((cost) => {
        if (!live) return;
        setState(
          cost
            ? { kind: "ready", cost }
            : { kind: "failed", reason: "the engine gave no answer" },
        );
      })
      .catch((cause: unknown) => {
        if (live) setState({ kind: "failed", reason: String(cause) });
      });
    return () => {
      live = false;
    };
  }, [view, key]);
  return state;
}
