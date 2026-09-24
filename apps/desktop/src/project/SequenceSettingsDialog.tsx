import type {
  Rational,
  SequenceSettings,
  SettingsImpact,
} from "@blinkify/types";
import { Button, Dialog, DialogClose, NumberInput, Select } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useProjectStore } from "./project.store.js";

/**
 * The sequence settings dialog (#57): resolution, frame rate, pixel aspect
 * and the colour policy — and, before anything is applied, what the change
 * costs in copying.
 *
 * The cost is the engine's answer (`preview_settings`), from the model's one
 * copy-eligibility predicate; the dialog only states it. A change that would
 * turn copied clips into re-encoded ones says how many and how long before
 * the user commits to it, because a sequence at the wrong size or rate
 * silently switches the whole product off.
 */

interface Preset {
  readonly label: string;
  readonly width: number;
  readonly height: number;
}

export const RESOLUTIONS: readonly Preset[] = [
  { label: "3840 × 2160 (4K UHD)", width: 3840, height: 2160 },
  { label: "2560 × 1440 (1440p)", width: 2560, height: 1440 },
  { label: "1920 × 1080 (1080p)", width: 1920, height: 1080 },
  { label: "1280 × 720 (720p)", width: 1280, height: 720 },
  { label: "2160 × 3840 (4K vertical)", width: 2160, height: 3840 },
  { label: "1080 × 1920 (1080p vertical)", width: 1080, height: 1920 },
  { label: "1080 × 1080 (square)", width: 1080, height: 1080 },
];

export const FRAME_RATES: readonly { label: string; rate: Rational }[] = [
  { label: "23.976 fps", rate: { num: 24000, den: 1001 } },
  { label: "24 fps", rate: { num: 24, den: 1 } },
  { label: "25 fps", rate: { num: 25, den: 1 } },
  { label: "29.97 fps", rate: { num: 30000, den: 1001 } },
  { label: "30 fps", rate: { num: 30, den: 1 } },
  { label: "50 fps", rate: { num: 50, den: 1 } },
  { label: "59.94 fps", rate: { num: 60000, den: 1001 } },
  { label: "60 fps", rate: { num: 60, den: 1 } },
];

const CUSTOM = "custom";

const rateKey = (rate: Rational) => `${rate.num}/${rate.den}`;

function gcd(a: number, b: number): number {
  return b === 0 ? Math.abs(a) : gcd(b, a % b);
}

/** `16:9` — the shape on screen, for reading, not for deciding anything. */
export function displayAspect(settings: SequenceSettings): string {
  const num = settings.width * settings.pixelAspect.num;
  const den = settings.height * settings.pixelAspect.den;
  const divisor = gcd(num, den) || 1;
  return `${num / divisor}:${den / divisor}`;
}

/** `1:42` or `1:02:03` for a duration in seconds. */
export function duration(seconds: number): string {
  const whole = Math.round(seconds);
  const h = Math.floor(whole / 3600);
  const m = Math.floor((whole % 3600) / 60);
  const s = whole % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

const clips = (count: number) => (count === 1 ? "1 clip" : `${count} clips`);

/** What a change costs, in words: the lossless consequence, stated first. */
export function impactStatement(impact: SettingsImpact): string {
  const parts: string[] = [];
  if (impact.losingClips > 0)
    parts.push(
      `${clips(impact.losingClips)} (${duration(impact.losingSeconds)}) would no longer be copied losslessly and would be re-encoded to fit.`,
    );
  if (impact.gainingClips > 0)
    parts.push(
      `${clips(impact.gainingClips)} (${duration(impact.gainingSeconds)}) would be copied losslessly instead of re-encoded.`,
    );
  if (parts.length === 0)
    parts.push(
      impact.ineligibleAfter > 0
        ? `No clip changes how it is exported; ${clips(impact.ineligibleAfter)} still cannot be copied.`
        : "Every clip that can be copied losslessly now still can.",
    );
  return parts.join(" ");
}

export function SequenceSettingsDialog({
  open,
  onOpenChange,
}: {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
}) {
  const hasProject = useProjectStore((state) => state.view !== null);
  // Mounted only while open, so each opening starts from what the sequence
  // has rather than from a draft left over from the last one.
  if (!open || !hasProject) return null;
  return <SettingsDialogBody onOpenChange={onOpenChange} />;
}

function SettingsDialogBody({
  onOpenChange,
}: {
  readonly onOpenChange: (open: boolean) => void;
}) {
  const view = useProjectStore((state) => state.view);
  const edit = useProjectStore((state) => state.edit);
  const current = view?.project.sequence.settings;
  const [draft, setDraft] = useState<SequenceSettings | null>(() =>
    current
      ? {
          ...current,
          frameRate: { ...current.frameRate },
          pixelAspect: { ...current.pixelAspect },
        }
      : null,
  );
  const [impact, setImpact] = useState<SettingsImpact | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  // Ask the engine what the draft would cost, before it is applied.
  useEffect(() => {
    if (!draft) return;
    let stale = false;
    invoke<SettingsImpact>("preview_settings", { settings: draft })
      .then((answer) => {
        if (stale) return;
        setImpact(answer);
        setProblem(null);
      })
      .catch((cause: unknown) => {
        if (stale) return;
        setImpact(null);
        setProblem(String(cause));
      });
    return () => {
      stale = true;
    };
  }, [draft]);

  if (!view || !draft || !current) return null;

  const resolution =
    RESOLUTIONS.find(
      (p) => p.width === draft.width && p.height === draft.height,
    )?.label ?? CUSTOM;
  const rateOptions = FRAME_RATES.some(
    (option) => rateKey(option.rate) === rateKey(draft.frameRate),
  )
    ? FRAME_RATES
    : [
        ...FRAME_RATES,
        {
          label: `${(draft.frameRate.num / draft.frameRate.den).toFixed(3)} fps`,
          rate: draft.frameRate,
        },
      ];
  const unchanged =
    draft.width === current.width &&
    draft.height === current.height &&
    rateKey(draft.frameRate) === rateKey(current.frameRate) &&
    rateKey(draft.pixelAspect) === rateKey(current.pixelAspect);
  const waiting = view.project.sequence.matchFirstClip;

  const apply = async () => {
    const refusal = await edit({ edit: "set-settings", settings: draft });
    if (refusal) setProblem(refusal);
    else onOpenChange(false);
  };

  return (
    <Dialog
      open
      onOpenChange={onOpenChange}
      title="Sequence settings"
      description="The shape every clip is placed into. A clip is copied losslessly only where the sequence matches it."
      actions={
        <>
          <DialogClose>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <Button
            variant="primary"
            disabled={problem !== null || (unchanged && !waiting)}
            onClick={() => void apply()}
          >
            Apply
          </Button>
        </>
      }
    >
      <div className="sequence-settings">
        {waiting ? (
          <p className="sequence-settings__note" data-testid="match-first-clip">
            Until you choose, the sequence takes its resolution and frame rate
            from the first video clip you add, so that clip is copied
            losslessly.
          </p>
        ) : null}
        <Select
          label="Resolution"
          value={resolution}
          onValueChange={(value) => {
            const preset = RESOLUTIONS.find((p) => p.label === value);
            if (preset)
              setDraft({
                ...draft,
                width: preset.width,
                height: preset.height,
              });
          }}
          options={[
            ...RESOLUTIONS.map((p) => ({ value: p.label, label: p.label })),
            { value: CUSTOM, label: "Custom", disabled: true },
          ]}
        />
        <div className="sequence-settings__size">
          <NumberInput
            label="Width"
            value={draft.width}
            min={2}
            max={8192}
            step={2}
            unit="px"
            onValueChange={(width) => setDraft({ ...draft, width })}
          />
          <NumberInput
            label="Height"
            value={draft.height}
            min={2}
            max={8192}
            step={2}
            unit="px"
            onValueChange={(height) => setDraft({ ...draft, height })}
          />
        </div>
        <Select
          label="Frame rate"
          value={rateKey(draft.frameRate)}
          onValueChange={(value) => {
            const option = rateOptions.find((o) => rateKey(o.rate) === value);
            if (option) setDraft({ ...draft, frameRate: { ...option.rate } });
          }}
          options={rateOptions.map((o) => ({
            value: rateKey(o.rate),
            label: o.label,
          }))}
        />
        <dl className="sequence-settings__facts">
          <dt>Pixel aspect</dt>
          <dd>
            {draft.pixelAspect.num === draft.pixelAspect.den
              ? "Square (1:1)"
              : `${draft.pixelAspect.num}:${draft.pixelAspect.den}`}
          </dd>
          <dt>Display aspect</dt>
          <dd data-testid="display-aspect">{displayAspect(draft)}</dd>
          <dt>Colour</dt>
          <dd>
            SDR (Rec. 709). HDR clips are copied as recorded; a part of one that
            would have to be re-encoded is declined, never converted.
          </dd>
        </dl>
        <p
          className="sequence-settings__impact"
          role="status"
          data-testid="settings-impact"
          data-state={
            problem
              ? "refused"
              : impact && impact.losingClips > 0
                ? "costly"
                : "ok"
          }
        >
          {problem ?? (impact ? impactStatement(impact) : "Checking…")}
        </p>
      </div>
    </Dialog>
  );
}
