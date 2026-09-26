import {
  Button,
  DropdownMenu,
  IconButton,
  type MenuGroupDefinition,
} from "@blinkify/ui";
import { useEffect, useState } from "react";
import {
  closeWindow,
  isTauri,
  isWindowMaximised,
  minimiseWindow,
  onWindowResized,
  toggleMaximiseWindow,
} from "./windowChrome.js";
import { ExportDialog } from "../export/ExportDialog.js";
import { ExportQueueButton } from "../export/ExportQueue.js";
import { LosslessIndicator } from "../export/LosslessIndicator.js";
import { HistoryControls } from "../project/HistoryControls.js";
import { SequenceSettingsDialog } from "../project/SequenceSettingsDialog.js";
import { ShortcutReference } from "../shortcuts/ShortcutReference.js";
import { ThirdPartyNotices } from "./ThirdPartyNotices.js";
import { keyFor } from "../shortcuts/shortcuts.js";
import { useShortcutsUi } from "../shortcuts/shortcuts.store.js";
import { useTimelineStore } from "../timeline/timeline.store.js";
import { requestClose, runFileAction } from "../project/ProjectLifecycle.js";
import {
  fileName,
  projectTitle,
  useProjectStore,
} from "../project/project.store.js";

/**
 * The application bar (#19), laid out as the layout reference specifies:
 * menus, project name, undo and redo, the lossless indicator, Export, and the
 * window controls at the far right.
 *
 * ── The lossless indicator is the planner's ────────────────────────────────
 *
 * It shows the export plan (#39) in one state and a count, asked again after
 * every edit, and says the tier is not computed whenever there is no plan. It
 * never shows `Lossless` optimistically: "every pixel was preserved" is the
 * one claim the product is built on — `CLAUDE.md` section 1 — and the planner
 * decides it, the bar displays it (section 2).
 *
 * ── Export ──────────────────────────────────────────────────────────────────
 *
 * Opens the export dialog (#50); the queue beside it (#51) follows the exports
 * in the background.
 */

const noop = (): void => undefined;

/** What the menus can do that depends on the open project. */
interface MenuActions {
  /** Open the sequence settings (#57); `null` with no project open. */
  readonly sequenceSettings: (() => void) | null;
  /** Whether a project is open, for Save and Close (#54). */
  readonly hasProject: boolean;
  /** Recently opened projects (#54), most recent first. */
  readonly recent: readonly { path: string; name: string }[];
  /** Show the third-party notices (#73). */
  readonly notices: () => void;
}

const menus = (
  actions: MenuActions,
): readonly {
  readonly label: string;
  readonly groups: readonly MenuGroupDefinition[];
}[] => [
  {
    label: "File",
    groups: [
      {
        items: [
          {
            id: "new",
            label: "New project…",
            shortcut: keyFor("new-project"),
            onSelect: () => runFileAction("new"),
          },
          {
            id: "open",
            label: "Open…",
            shortcut: keyFor("open-project"),
            onSelect: () => runFileAction("open"),
          },
          {
            id: "save",
            label: "Save",
            shortcut: keyFor("save"),
            disabled: !actions.hasProject,
            onSelect: () => runFileAction("save"),
          },
          {
            id: "save-as",
            label: "Save as…",
            shortcut: keyFor("save-as"),
            disabled: !actions.hasProject,
            onSelect: () => runFileAction("save-as"),
          },
          {
            id: "close",
            label: "Close project",
            disabled: !actions.hasProject,
            onSelect: requestClose,
          },
        ],
      },
      ...(actions.recent.length === 0
        ? []
        : [
            {
              label: "Recent",
              items: actions.recent.slice(0, 8).map((project) => ({
                id: `recent-${project.path}`,
                label: project.name || fileName(project.path),
                onSelect: () =>
                  void useProjectStore.getState().openProject(project.path),
              })),
            },
          ]),
      {
        items: [
          {
            id: "sequence-settings",
            label: "Sequence settings…",
            disabled: actions.sequenceSettings === null,
            onSelect: actions.sequenceSettings ?? noop,
          },
        ],
      },
      {
        items: [
          {
            id: "exit",
            label: "Exit",
            onSelect: () => {
              void closeWindow();
            },
          },
        ],
      },
    ],
  },
  {
    label: "Edit",
    groups: [
      {
        items: [
          {
            id: "undo",
            label: "Undo",
            shortcut: keyFor("undo"),
            onSelect: () => void useProjectStore.getState().undo(),
          },
          {
            id: "redo",
            label: "Redo",
            shortcut: keyFor("redo"),
            onSelect: () => void useProjectStore.getState().redo(),
          },
        ],
      },
    ],
  },
  {
    label: "View",
    groups: [
      {
        items: [
          {
            id: "zoom-fit",
            label: "Zoom to fit",
            shortcut: keyFor("zoom-to-fit"),
            onSelect: () => useTimelineStore.getState().fitAll(),
          },
        ],
      },
    ],
  },
  {
    label: "Help",
    groups: [
      {
        items: [
          {
            id: "shortcuts",
            label: "Keyboard shortcuts…",
            shortcut: keyFor("show-shortcuts"),
            onSelect: () => useShortcutsUi.getState().setReferenceOpen(true),
          },
          {
            id: "about",
            label: "About Blinkify",
            disabled: true,
            onSelect: noop,
          },
          {
            id: "third-party-notices",
            label: "Third-party notices…",
            onSelect: actions.notices,
          },
        ],
      },
    ],
  },
];

export function AppBar() {
  const [maximised, setMaximised] = useState(false);
  const name = useProjectStore((state) => projectTitle(state.view));
  const hasProject = useProjectStore((state) => state.view !== null);
  const recent = useProjectStore((state) => state.recent);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [noticesOpen, setNoticesOpen] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const refresh = (): void => {
      void isWindowMaximised().then((value) => {
        if (!cancelled) setMaximised(value);
      });
    };

    refresh();
    // The button must follow the window however the state changed — the
    // keyboard, the task bar, a double-click on the drag region.
    const stop = onWindowResized(refresh);

    return () => {
      cancelled = true;
      stop();
    };
  }, []);

  return (
    <header
      className="shell__bar"
      /* Tauri's drag region: dragging the bar moves the window, and a
         double-click on it maximises and restores. Both are Tauri's own
         implementation rather than a hand-rolled `startDragging()`, because a
         manual drag swallows the double-click and then neither works. Its
         restore behaviour on Windows has an open upstream bug
         (tauri-apps/tauri#11945), which is why double-click is called out by
         name in the manual verification list rather than assumed. */
      data-tauri-drag-region
      data-testid="app-bar"
    >
      <nav className="shell__menus" aria-label="Main menu">
        {menus({
          sequenceSettings: hasProject ? () => setSettingsOpen(true) : null,
          hasProject,
          recent,
          notices: () => setNoticesOpen(true),
        }).map((menu) => (
          <DropdownMenu
            key={menu.label}
            groups={menu.groups}
            trigger={
              <Button variant="ghost" size="sm">
                {menu.label}
              </Button>
            }
          />
        ))}
      </nav>

      {/* Not an input yet: renaming a project is #54. It is the reference's
          "project name, editable in place", in the place the reference puts
          it; the name is the open project's since #32. */}
      <span className="shell__project" data-testid="project-name">
        {name}
      </span>

      <HistoryControls />

      <LosslessIndicator />

      <ExportQueueButton />

      <Button
        variant="primary"
        size="sm"
        disabled={!hasProject}
        onClick={() => setExportOpen(true)}
        data-testid="export-button"
      >
        Export
      </Button>

      {/* Hidden in a browser tab, where the tab has its own chrome and these
          would be three buttons that do nothing. */}
      {isTauri() ? (
        <div className="shell__window-controls" data-testid="window-controls">
          <IconButton
            label="Minimise"
            size="sm"
            icon={<WindowGlyph shape="minimise" />}
            onClick={() => {
              void minimiseWindow();
            }}
          />
          <IconButton
            label={maximised ? "Restore" : "Maximise"}
            size="sm"
            icon={<WindowGlyph shape={maximised ? "restore" : "maximise"} />}
            onClick={() => {
              void toggleMaximiseWindow();
            }}
          />
          <IconButton
            label="Close"
            size="sm"
            variant="danger"
            icon={<WindowGlyph shape="close" />}
            onClick={() => {
              void closeWindow();
            }}
          />
        </div>
      ) : null}
      <SequenceSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
      />
      <ExportDialog open={exportOpen} onOpenChange={setExportOpen} />
      <ShortcutReference />
      <ThirdPartyNotices open={noticesOpen} onOpenChange={setNoticesOpen} />
    </header>
  );
}

/** The three window glyphs, at the proportions Windows draws them. */
function WindowGlyph({
  shape,
}: {
  readonly shape: "minimise" | "maximise" | "restore" | "close";
}) {
  const paths: Record<typeof shape, string> = {
    minimise: "M3 8h10",
    maximise: "M3.5 3.5h9v9h-9z",
    restore: "M5.5 5.5h7v7h-7zM3.5 3.5h7v2M10.5 10.5h2",
    close: "M4 4l8 8M12 4l-8 8",
  };

  return (
    <svg viewBox="0 0 16 16" aria-hidden="true">
      <path
        d={paths[shape]}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
      />
    </svg>
  );
}
