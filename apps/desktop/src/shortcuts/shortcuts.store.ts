import { create } from "zustand";

/** Whether the keyboard shortcut reference (#38) is open. */
interface ShortcutsUi {
  referenceOpen: boolean;
  setReferenceOpen: (open: boolean) => void;
}

export const useShortcutsUi = create<ShortcutsUi>((set) => ({
  referenceOpen: false,
  setReferenceOpen: (referenceOpen) => set({ referenceOpen }),
}));
