import {
  Button,
  DropdownMenu,
  IconButton,
  Tooltip,
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
import { HistoryControls } from "../project/HistoryControls.js";
import { SequenceSettingsDialog } from "../project/SequenceSettingsDialog.js";
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
 * ── The lossless indicator says it does not know ───────────────────────────
 *
 * The export planner is #39 and does not exist. The reference is explicit about
 * what the indicator does until then: it reports that the tier has not been
 * computed. It does not show `Lossless` optimistically.
 *
 * That is not caution for its own sake. "Every pixel was preserved" is the one
 * claim the product is built on — `CLAUDE.md` section 1 — and an indicator that
 * says it before anything has decided it is worse than an indicator that says
 * nothing, because the user has no way to tell the two apart.
 *
 * The renderer will never compute this. The planner decides and the bar
 * displays, which is section 2's rule about the direction of the arrow.
 *
 * ── Every command here is inert on purpose ─────────────────────────────────
 *
 * New, Open, Save and Export are wired to nothing, because the project file
 * is #32 and #54 and the export dialog is #50. Undo and redo are live since
 * #37. They are present and disabled rather than absent, so the bar is the
 * shape the reference describes and the issues that fill it have somewhere to
 * attach. A disabled control that will work is honest; a working control that
 * silently does nothing is not.
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
            shortcut: "Ctrl+N",
            onSelect: () => runFileAction("new"),
          },
          {
            id: "open",
            label: "Open…",
            shortcut: "Ctrl+O",
            onSelect: () => runFileAction("open"),
          },
          {
            id: "save",
            label: "Save",
            shortcut: "Ctrl+S",
            disabled: !actions.hasProject,
            onSelect: () => runFileAction("save"),
          },
          {
            id: "save-as",
            label: "Save as…",
            shortcut: "Ctrl+Shift+S",
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
            shortcut: "Ctrl+Z",
            onSelect: () => void useProjectStore.getState().undo(),
          },
          {
            id: "redo",
            label: "Redo",
            shortcut: "Ctrl+Y",
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
            disabled: true,
            onSelect: noop,
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
            id: "about",
            label: "About Blinkify",
            disabled: true,
            onSelect: noop,
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

      <Tooltip
        label="The export planner has not run. Blinkify will not claim a tier it has not computed."
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

      <Button variant="primary" size="sm" disabled>
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
