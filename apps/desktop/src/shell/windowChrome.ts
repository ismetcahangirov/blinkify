import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * The window controls Blinkify draws itself.
 *
 * ── Why the application draws them at all ──────────────────────────────────
 *
 * `decorations: false`. A native Windows title bar above the application bar
 * would be two bars stacked, one of them repeating the project name the other
 * already shows, and the design language of the top 32 pixels would belong to
 * the operating system rather than to the product. The layout reference puts
 * the menus, the project name, undo and redo, the lossless indicator and
 * Export on one row; a native title bar means that row starts 32 pixels down.
 *
 * ── What that costs, stated rather than discovered ─────────────────────────
 *
 * Windows 11 Snap Layouts — the flyout when the pointer rests on the maximise
 * button — do not work on an undecorated Tauri window. That is upstream
 * (tauri-apps/tauri#4531, open since 2022, labelled `status: upstream`) and not
 * something this application can switch on. Dragging a window to a screen edge
 * is affected by the same limitation.
 *
 * It is a real loss for an editor, which people do put side by side with a
 * browser. It is recorded in the pull request and in a follow-up issue rather
 * than quietly not mentioned, because the alternative — native decorations —
 * is a one-line configuration change the owner may prefer once they have seen
 * both.
 *
 * ── Why every call is guarded ──────────────────────────────────────────────
 *
 * `pnpm dev` runs the renderer in an ordinary browser tab, where there is no
 * Tauri and no window to minimise. The guard is not defensive programming: it
 * is the difference between the shell being developable in a browser and only
 * being runnable through a full Tauri build.
 */

/** True when running inside the Tauri shell rather than a browser tab. */
export function isTauri(): boolean {
  return (
    typeof globalThis === "object" &&
    "__TAURI_INTERNALS__" in (globalThis as Record<string, unknown>)
  );
}

/**
 * Every failure here is swallowed deliberately.
 *
 * A window control that throws would surface as an unhandled rejection in the
 * middle of an edit, and the user's recourse — clicking the button again — is
 * the thing that just failed. There is nothing to recover to and nothing
 * useful to say, so the call is attempted and the interface stays as it was.
 */
async function withWindow(
  action: (window: ReturnType<typeof getCurrentWindow>) => Promise<unknown>,
): Promise<void> {
  if (!isTauri()) return;
  try {
    await action(getCurrentWindow());
  } catch {
    // Intentionally silent — see above.
  }
}

export async function minimiseWindow(): Promise<void> {
  await withWindow((window) => window.minimize());
}

export async function toggleMaximiseWindow(): Promise<void> {
  await withWindow((window) => window.toggleMaximize());
}

export async function closeWindow(): Promise<void> {
  /* `close()`, not `destroy()`. Close is a request: it fires `CloseRequested`,
     which is where the window's geometry is written to disk and where an
     unsaved-project prompt will live once there are projects (#54). Destroy
     skips both. */
  await withWindow((window) => window.close());
}

/** Whether the window is currently maximised, for the button's label and glyph. */
export async function isWindowMaximised(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    return await getCurrentWindow().isMaximized();
  } catch {
    return false;
  }
}

/**
 * Subscribe to maximise and restore, however they happened.
 *
 * The button is not the only way the state changes — the keyboard does it, the
 * task bar does it, and a double-click on the drag region does it. A control
 * that only updates when it is itself clicked is a control that lies as soon as
 * the user uses any other route.
 *
 * Returns an unsubscribe function, or a no-op outside Tauri.
 */
export function onWindowResized(listener: () => void): () => void {
  if (!isTauri()) return () => undefined;

  let unlisten: (() => void) | null = null;
  let cancelled = false;

  void getCurrentWindow()
    .onResized(() => {
      listener();
    })
    .then((stop) => {
      if (cancelled) {
        stop();
        return;
      }
      unlisten = stop;
    })
    .catch(() => {
      // No listener is better than a thrown promise during mount.
    });

  return () => {
    cancelled = true;
    unlisten?.();
  };
}
