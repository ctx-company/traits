import { assignment, defineConfig } from "@ctx-traits/config";
import { describe, expect, it } from "vitest";

describe("defineConfig", () => {
  it("returns the config object unchanged (identity + type anchor)", () => {
    const config = defineConfig({
      agent: {
        role: {
          default: assignment({ harness: "claude", model: "sonnet" }),
        },
      },
      run: {
        worktree: true,
        maxRetries: 2,
      },
    });

    expect(config).toEqual({
      agent: {
        role: {
          default: { harness: "claude", model: "sonnet" },
        },
      },
      run: {
        worktree: true,
        maxRetries: 2,
      },
    });
  });

  it("accepts an empty config", () => {
    expect(defineConfig({})).toEqual({});
  });

  it("accepts the complete narrow personal repo override shape", () => {
    const config = defineConfig({
      repo: {
        "repo-key": {
          agent: {
            modelTier: { top: assignment({ model: "top-model" }) },
            role: { worker: assignment({ model: "personal-model" }) },
            variant: { fast: { role: { worker: assignment({ model: "fast-model" }) } } },
          },
          harness: { local: { kind: "custom", bin: "agent" } },
          host: { local: { profile: "default" } },
          worktree: {
            seed: [".cache"],
            warm: ["warm"],
            env: { CACHE_DIR: ".cache" },
            tripwire: { sentinel: [".sentinel"] },
          },
          run: { wait: true, story: "detailed", buildCache: { cargo: { env: "CARGO_HOME" } } },
          merge: { wait: true, auto: false, deep: true },
          git: { longSeconds: 60 },
          registry: { base: "https://registry.example" },
          publish: { exclude: [".cache"] },
        },
      },
    });

    expect(config.repo?.["repo-key"]?.agent?.role?.worker).toEqual({ model: "personal-model" });
  });
});

describe("closed shapes reject typos and bad enum values at compile time", () => {
  it("rejects an unrecognized top-level key", () => {
    defineConfig({
      // @ts-expect-error unrecognized key is not part of CtxConfig
      unknownTopLevelKey: true,
    });
  });

  it("rejects an unrecognized key inside a nested table", () => {
    defineConfig({
      run: {
        // @ts-expect-error unrecognized key is not part of RunTable
        notARealField: 1,
      },
    });
  });

  it("rejects a bad transport literal", () => {
    assignment({
      // @ts-expect-error transport must be "cli" | "mcp"
      transport: "not-a-real-transport",
    });
  });

  it("rejects a bad session-mode literal", () => {
    assignment({
      // @ts-expect-error sessionMode must be "per-frame" | "persistent"
      sessionMode: "not-a-real-mode",
    });
  });
});
