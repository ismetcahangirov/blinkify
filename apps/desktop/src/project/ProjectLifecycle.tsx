import type { RecoveryOffer, SequenceSettings } from "@blinkify/types";
import { Button, Dialog, DialogClose, Select } from "@blinkify/ui";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { create } from "zustand";
import { isTyping } from "./HistoryControls.js";
import { projectName, useProjectStore } from "./project.store.js";
import { FRAME_RATES, RESOLUTIONS } from "./SequenceSettingsDialog.js";

/**
 * The project lifecycle in the interface (#54): the dialogs for a new
 * project, for unsaved work found at launch, and for closing on unsaved
 * work; the file dialogs; the File keys; and the autosave tick.
 *
 * The rules are the engine's (`project::session`): autosave never touches
 * the project file, dirty is exact, a recovery is offered with both times.
 * This only asks the user and sends their answer.
 */

/** How often unsaved work is written to its recovery file. */
export const AUTOSAVE_SECONDS = 60;

/** The event the shell sends when the window is closed on unsaved work. */
const CLOSE_REQUESTED_EVENT = "project://close-requested";

const FILTERS = [{ name: "Blinkify project", extensions: ["blinkify"] }];

interface LifecycleUi {
  creating: boolean;
  /** The window is closing, or the project is, on unsaved work. */
  closing: "window" | "project" | null;
  setCreating: (creating: boolean) => void;
  setClosing: (closing: "window" | "project" | null) => void;
}

export const useLifecycleUi = create<LifecycleUi>((set) => ({
  creating: false,
  closing: null,
  setCreating: (creating) => set({ creating }),
  setClosing: (closing) => set({ closing }),
}));

/** Ask where, then open the project there. */
export async function openFromDialog(): Promise<void> {
  const path = await openDialog({
    multiple: false,
    directory: false,
    filters: FILTERS,
  });
  if (typeof path === "string")
    await useProjectStore.getState().openProject(path);
}

/** Save, asking where first if the project has never been saved. Resolves
 * true when it is saved. */
export async function saveProject(): Promise<boolean> {
  const store = useProjectStore.getState();
  const refusal = await store.save();
  if (refusal === "untitled") return saveProjectAs();
  return refusal === null;
}

export async function saveProjectAs(): Promise<boolean> {
  const store = useProjectStore.getState();
  const path = await saveDialog({
    filters: FILTERS,
    defaultPath: `${projectName(store.view)}.blinkify`,
  });
  if (!path) return false;
  return (await store.saveAs(path)) === null;
}

/** Close the open project, asking first if it has unsaved work. */
export function requestClose(): void {
  const { view, closeProject } = useProjectStore.getState();
  if (view?.dirty) useLifecycleUi.getState().setClosing("project");
  else void closeProject(false);
}

export type FileAction = "new" | "open" | "save" | "save-as";

/** The File keys: CapCut's Ctrl+N, Ctrl+O, Ctrl+S; Ctrl+Shift+S saves as. */
export function fileActionForKey(event: {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}): FileAction | null {
  if (!(event.ctrlKey || event.metaKey) || event.altKey) return null;
  switch (event.key.toLowerCase()) {
    case "n":
      return event.shiftKey ? null : "new";
    case "o":
      return event.shiftKey ? null : "open";
    case "s":
      return event.shiftKey ? "save-as" : "save";
    default:
      return null;
  }
}

export function runFileAction(action: FileAction): void {
  switch (action) {
    case "new":
      useLifecycleUi.getState().setCreating(true);
      return;
    case "open":
      void openFromDialog();
      return;
    case "save":
      void saveProject();
      return;
    case "save-as":
      void saveProjectAs();
      return;
  }
}

/** When `at` was, in the user's own clock. */
export function when(at: number | null): string {
  if (at === null) return "never saved";
  return new Date(at).toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  });
}

function NewProjectDialog() {
  const creating = useLifecycleUi((state) => state.creating);
  const setCreating = useLifecycleUi((state) => state.setCreating);
  const newProject = useProjectStore((state) => state.newProject);
  const [name, setName] = useState("Untitled project");
  const [match, setMatch] = useState(true);
  const [resolution, setResolution] = useState(RESOLUTIONS[2]?.label ?? "");
  const [rate, setRate] = useState("30/1");

  const create = async () => {
    const preset = RESOLUTIONS.find((p) => p.label === resolution);
    const frameRate = FRAME_RATES.find(
      (r) => `${r.rate.num}/${r.rate.den}` === rate,
    )?.rate;
    const settings: SequenceSettings | null =
      match || !preset || !frameRate
        ? null
        : {
            width: preset.width,
            height: preset.height,
            frameRate,
            pixelAspect: { num: 1, den: 1 },
            colour: "sdr",
          };
    await newProject(name.trim() || "Untitled project", settings);
    setCreating(false);
  };

  return (
    <Dialog
      open={creating}
      onOpenChange={setCreating}
      title="New project"
      description="The sequence matches the first clip you add unless you choose its settings now — which is what keeps that clip lossless."
      actions={
        <>
          <DialogClose>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <Button variant="primary" onClick={() => void create()}>
            Create
          </Button>
        </>
      }
    >
      <div className="sequence-settings">
        <label className="lifecycle__field">
          <span>Name</span>
          <input
            className="lifecycle__input"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label className="lifecycle__check">
          <input
            type="checkbox"
            checked={match}
            onChange={(event) => setMatch(event.target.checked)}
          />
          <span>Match the first clip (recommended)</span>
        </label>
        {match ? null : (
          <>
            <Select
              label="Resolution"
              value={resolution}
              onValueChange={setResolution}
              options={RESOLUTIONS.map((p) => ({
                value: p.label,
                label: p.label,
              }))}
            />
            <Select
              label="Frame rate"
              value={rate}
              onValueChange={setRate}
              options={FRAME_RATES.map((r) => ({
                value: `${r.rate.num}/${r.rate.den}`,
                label: r.label,
              }))}
            />
          </>
        )}
      </div>
    </Dialog>
  );
}

function RecoveryDialog() {
  const offers = useProjectStore((state) => state.recovery);
  const restore = useProjectStore((state) => state.restore);
  const discard = useProjectStore((state) => state.discardRecovery);
  const offer: RecoveryOffer | undefined = offers[0];
  if (!offer) return null;
  return (
    <Dialog
      open
      onOpenChange={() => undefined}
      title="Recover unsaved work?"
      description="Blinkify closed without saving. This work was kept."
      actions={
        <>
          <Button variant="secondary" onClick={() => void discard(offer)}>
            Discard
          </Button>
          <Button variant="primary" onClick={() => void restore(offer)}>
            Restore
          </Button>
        </>
      }
    >
      <dl className="sequence-settings__facts" data-testid="recovery-offer">
        <dt>Project</dt>
        <dd>{offer.name || "Untitled project"}</dd>
        <dt>Unsaved work from</dt>
        <dd>{when(offer.recoveredAt)}</dd>
        <dt>Last saved</dt>
        <dd>{when(offer.savedAt)}</dd>
      </dl>
    </Dialog>
  );
}

function CloseDialog() {
  const closing = useLifecycleUi((state) => state.closing);
  const setClosing = useLifecycleUi((state) => state.setClosing);
  const view = useProjectStore((state) => state.view);
  const closeProject = useProjectStore((state) => state.closeProject);

  const finish = async (discard: boolean) => {
    const target = closing;
    setClosing(null);
    if (target === "window") await invoke("quit_app", { discard });
    else await closeProject(discard);
  };

  return (
    <Dialog
      open={closing !== null}
      onOpenChange={(open) => {
        if (!open) setClosing(null);
      }}
      title={`Save changes to ${projectName(view)}?`}
      description="Your changes will be lost if you don't save them."
      actions={
        <>
          <DialogClose>
            <Button variant="secondary">Cancel</Button>
          </DialogClose>
          <Button variant="secondary" onClick={() => void finish(true)}>
            Don&apos;t save
          </Button>
          <Button
            variant="primary"
            onClick={() =>
              void saveProject().then((saved) => {
                if (saved) void finish(false);
              })
            }
          >
            Save
          </Button>
        </>
      }
    />
  );
}

/** Everything the lifecycle needs mounted once: the launch, the dialogs, the
 * keys, the autosave tick, and the answer to a window closed on unsaved work. */
export function ProjectLifecycle() {
  const start = useProjectStore((state) => state.start);
  const autosave = useProjectStore((state) => state.autosave);
  const loadRecent = useProjectStore((state) => state.loadRecent);

  useEffect(() => {
    void start();
    void loadRecent();
  }, [start, loadRecent]);

  useEffect(() => {
    const timer = setInterval(() => void autosave(), AUTOSAVE_SECONDS * 1000);
    return () => clearInterval(timer);
  }, [autosave]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (isTyping(event.target)) return;
      const action = fileActionForKey(event);
      if (!action) return;
      event.preventDefault();
      runFileAction(action);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let closed = false;
    void listen(CLOSE_REQUESTED_EVENT, () =>
      useLifecycleUi.getState().setClosing("window"),
    )
      .then((unlisten) => {
        if (closed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      closed = true;
      stop?.();
    };
  }, []);

  return (
    <>
      <NewProjectDialog />
      <RecoveryDialog />
      <CloseDialog />
    </>
  );
}
