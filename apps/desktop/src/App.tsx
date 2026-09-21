import { UpdateBanner } from "./UpdateBanner.js";
import { useShellStore } from "./shell.store.js";

/**
 * The application shell.
 *
 * Deliberately almost empty. The CapCut-structured layout — media library,
 * preview, inspector, timeline — is Epic #2 (#18, #19), and building it before
 * the design system exists would mean building it twice.
 *
 * The classes are Tailwind utilities reading the token theme declared in
 * `styles.css` (#15). `text-text-secondary` is the semantic token
 * `--text-secondary`, not a colour: the repetition is the price of the
 * utility name matching the token name exactly.
 */
export function App() {
  const engineStatus = useShellStore((state) => state.engineStatus);

  return (
    <main className="flex h-full flex-col items-center justify-center gap-1">
      <UpdateBanner />
      <h1 className="text-4xl font-semibold tracking-tight">Blinkify</h1>
      <p className="text-text-secondary">Edit video without degrading it.</p>
      <p className="text-sm text-text-secondary" data-testid="engine-status">
        Engine: {engineStatus}
      </p>
    </main>
  );
}
