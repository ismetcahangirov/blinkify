import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri drives this dev server, so the port is fixed and failure to bind must
// be loud: a silent fallback to 1421 leaves the window pointing at nothing.
const DEV_PORT = 1420;

export default defineConfig({
  plugins: [react()],
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
