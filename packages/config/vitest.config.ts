import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@ctx-traits/config": new URL("./src/index.ts", import.meta.url).pathname,
    },
  },
});
