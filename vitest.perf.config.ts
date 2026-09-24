import { defineConfig } from "vitest/config";
import base from "./vitest.config.js";

/**
 * The heavy workflow's benchmarks (`pnpm bench:timeline`): the same setup as
 * the unit suite, over `*.perf.ts` only, so a pull request never runs them.
 * Spread rather than merged, because merging concatenates `include`.
 */
export default defineConfig({
  ...base,
  test: {
    ...base.test,
    include: ["apps/**/*.perf.ts"],
  },
});
