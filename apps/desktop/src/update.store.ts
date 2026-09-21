import type { UpdateOffer } from "@blinkify/types";
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

/**
 * Renderer state for the update offer.
 *
 * `CLAUDE.md` section 20 rule 3: nothing is applied silently. The engine-side
 * check runs at launch and stops at "there is a newer version"; this store is
 * how that fact reaches a person, and `install` is only ever called from
 * something they clicked.
 *
 * Note what this store cannot do. It has no timer, no retry, and no code path
 * from `offered` to `installing` that does not pass through a caller. Those are
 * the guarantees, and they are guarantees because the transitions do not exist
 * rather than because a flag is set correctly.
 */
export type UpdateStatus =
  | "idle"
  | "offered"
  | "installing"
  | "failed"
  /** The user said no. Nothing asks again this session. */
  | "dismissed";

interface UpdateState {
  status: UpdateStatus;
  offer: UpdateOffer | null;
  /** Why the install failed, in the words the shell gave us. */
  error: string | null;
  /** Read what the launch check found. Goes nowhere near the network. */
  refresh: () => Promise<void>;
  /** Download, install and restart. Only from a deliberate action. */
  install: () => Promise<void>;
  dismiss: () => void;
}

export const useUpdateStore = create<UpdateState>((set, get) => ({
  status: "idle",
  offer: null,
  error: null,

  refresh: async () => {
    // Never overwrite a decision the user already made, and never interrupt an
    // install in progress. A second mount must not resurrect a dismissed offer.
    const { status } = get();
    if (status === "dismissed" || status === "installing") return;

    try {
      const offer = await invoke<UpdateOffer | null>("pending_update");
      set(
        offer
          ? { status: "offered", offer, error: null }
          : { status: "idle", offer: null },
      );
    } catch {
      // No update is the normal case: no release yet, no network, or the
      // manifest failed verification. None of those is worth a banner on
      // launch, and a failed check is not a failed application.
      set({ status: "idle", offer: null });
    }
  },

  install: async () => {
    set({ status: "installing", error: null });
    try {
      await invoke("install_update");
      // Unreachable in practice: the shell restarts the application. If control
      // does come back, the install did not happen, and saying so is better
      // than a spinner that never stops.
      set({ status: "failed", error: "the application did not restart" });
    } catch (cause) {
      set({
        status: "failed",
        error: cause instanceof Error ? cause.message : String(cause),
      });
    }
  },

  dismiss: () => {
    set({ status: "dismissed" });
  },
}));
