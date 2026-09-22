import { TooltipProvider } from "@blinkify/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App.js";
import "./styles.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error(
    "#root is missing from index.html — the renderer cannot mount",
  );
}

/*
 * One tooltip provider around the whole application (#17).
 *
 * It is the group-delay clock, not a context for convenience. Tooltips mounted
 * without it each run their own timer, and a toolbar of independent timers is
 * the flickering row the delay exists to prevent — so it belongs at the root,
 * once, rather than at each panel.
 */
createRoot(container).render(
  <StrictMode>
    <TooltipProvider>
      <App />
    </TooltipProvider>
  </StrictMode>,
);
