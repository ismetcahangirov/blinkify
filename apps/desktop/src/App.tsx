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
 * `styles.css` (#15, #16). `text-text-secondary` is the semantic token
 * `--text-secondary`, not a colour: the repetition is the price of the
 * utility name matching the token name exactly. `text-display` and
 * `text-caption` are type roles from the scale, each carrying its own size,
 * line height, weight and tracking — Tailwind's own `text-4xl` and `text-sm`
 * were cleared in `styles.css`, so an off-scale size is now a class that does
 * not exist rather than one nobody notices.
 */
export function App() {
  const engineStatus = useShellStore((state) => state.engineStatus);

  return (
    <main className="flex h-full flex-col items-center justify-center gap-1">
      <UpdateBanner />
      <h1 className="text-display">Blinkify</h1>
      <p className="text-body text-text-secondary">
        Edit video without degrading it.
      </p>
      <p
        className="text-caption text-text-secondary"
        data-testid="engine-status"
      >
        Engine: {engineStatus}
      </p>
    </main>
  );
}
