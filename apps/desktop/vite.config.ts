import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri drives this dev server, so the port is fixed and failure to bind must
// be loud: a silent fallback to 1421 leaves the window pointing at nothing.
const DEV_PORT = 1420;

export default defineConfig({
  // Tailwind v4 resolves the `@import "@blinkify/ui/tokens.css"` in styles.css
  // itself, through the workspace package's exports map. Without this plugin
  // the import still resolves but no utility is generated, and the shell
  // renders unstyled with every token present and unused — a failure that
  // looks like a CSS mistake rather than a missing build step.
  plugins: [react(), tailwindcss()],
  // Tauri's CLI owns the terminal; Vite clearing it eats the Rust build output.
  clearScreen: false,
  server: {
    port: DEV_PORT,
    strictPort: true,
    watch: {
      // Rust build artefacts churn constantly and are not renderer input.
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // WebView2 is evergreen Chromium, so there is no reason to ship ES5.
    target: "chrome120",
    sourcemap: true,
  },
});
