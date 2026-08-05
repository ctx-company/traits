import { defineConfig } from "vitest/config";
import baseConfig from "./vitest.config.js";

/**
 * Runs hand-authored `*.hand.ts` tests (normalization property test, 0104)
 * that stay out of the gate per the standing 2026-07-24 validation ruling.
 * Not referenced by `pnpm test`/`just ts-test` — invoke explicitly:
 * `pnpm exec vitest run --config vitest.hand.config.ts`.
 */
export default defineConfig({
  ...baseConfig,
  test: {
    ...baseConfig.test,
    include: ["test/**/*.hand.ts"],
  },
});
