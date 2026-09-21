import { UpdateBanner } from "./UpdateBanner.js";
import { useShellStore } from "./shell.store.js";

/**
 * The application shell.
 *
 * Deliberately almost empty. The CapCut-structured layout — media library,
 * preview, inspector, timeline — is Epic #2 (#18, #19), and building it before
 * the design system exists would mean building it twice.
 */
export function App() {
  const engineStatus = useShellStore((state) => state.engineStatus);

  return (
    <main className="shell">
      <UpdateBanner />
      <h1 className="shell__wordmark">Blinkify</h1>
      <p className="shell__tagline">Edit video without degrading it.</p>
      <p className="shell__status" data-testid="engine-status">
        Engine: {engineStatus}
      </p>
    </main>
  );
}
