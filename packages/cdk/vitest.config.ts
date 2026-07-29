import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@ctx-traits/cdk": new URL("./src/index.ts", import.meta.url).pathname,
    },
  },
});
