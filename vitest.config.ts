import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@blinkify/types": here("./packages/types/src/index.ts"),
      "@blinkify/ui": here("./packages/ui/src/index.ts"),
    },
  },
  test: {
    environment: "jsdom",
    globals: false,
    setupFiles: [here("./vitest.setup.ts")],
    include: ["apps/**/*.test.{ts,tsx}", "packages/**/*.test.{ts,tsx}"],
    // Rust is tested by `cargo test`; Vitest reaching into crates/ would only
    // find files it cannot run.
    exclude: ["**/node_modules/**", "**/dist/**", "**/target/**", "crates/**"],
  },
});
