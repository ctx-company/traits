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
});

describe("closed shapes reject typos and bad enum values at compile time", () => {
  it("rejects an unknown top-level key", () => {
    defineConfig({
      // @ts-expect-error unknown key is not part of CtxConfig
      unknownTopLevelKey: true,
    });
  });

  it("rejects an unknown key inside a nested table", () => {
    defineConfig({
      run: {
        // @ts-expect-error unknown key is not part of RunTable
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
