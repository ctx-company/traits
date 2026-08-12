import {
  defineBudget,
  defineConfig,
  defineHarness,
  definePricing,
  defineRepo,
  defineRole,
  configureTrait,
  evaluateConfigFunction,
} from "@ctx-traits/config";
import { describe, expect, it } from "vitest";

describe("evaluateConfigFunction", () => {
  it("assembles singleton and keyed tables from define* calls", () => {
    const config = evaluateConfigFunction(() => {
      defineBudget({ frameSeconds: 1200, maxFrames: 1000 });
      defineRole("master", { harness: "claude", model: "claude-opus-5" });
      defineRole("smart", [
        { harness: "claude", model: "claude-opus-5" },
        { harness: "codex", model: "gpt-5.2" },
      ]);
      definePricing("claude-opus-5", { usdPerMtok: 15 });
      configureTrait("refactor", {});
    });

    expect(config).toEqual({
      budget: { frameSeconds: 1200, maxFrames: 1000 },
      agent: {
        role: {
          master: { harness: "claude", model: "claude-opus-5" },
          smart: [
            { harness: "claude", model: "claude-opus-5" },
            { harness: "codex", model: "gpt-5.2" },
          ],
        },
      },
      pricing: { "claude-opus-5": { usdPerMtok: 15 } },
      trait: { refactor: {} },
    });
  });

  it("rejects a second call to a singleton registrar", () => {
    expect(() =>
      evaluateConfigFunction(() => {
        defineBudget({ frameSeconds: 1 });
        defineBudget({ frameSeconds: 2 });
      }),
    ).toThrow(/defineBudget was already called/);
  });

  it("rejects a duplicate key for a keyed registrar", () => {
    expect(() =>
      evaluateConfigFunction(() => {
        defineHarness("claude", { bin: "claude" });
        defineHarness("claude", { bin: "claude-2" });
      }),
    ).toThrow(/defineHarness\("claude", \.\.\.\) was already called/);
  });

  it("rejects a registrar called outside the config build", () => {
    expect(() => defineBudget({ frameSeconds: 1 })).toThrow(/called outside the config build/);
  });

  it("rejects an async default export", async () => {
    expect(() =>
      evaluateConfigFunction((async () => {
        defineBudget({ frameSeconds: 1 });
      }) as unknown as () => unknown),
    ).toThrow(/must be synchronous/);
  });

  it("rejects mixing defineConfig with define* calls inside the frame", () => {
    expect(() =>
      evaluateConfigFunction(() => {
        defineBudget({ frameSeconds: 1 });
        defineConfig({});
      }),
    ).toThrow(/mixes defineConfig/);
  });

  it("rejects mixing a module-top-level defineConfig with a hook default export", () => {
    defineConfig({});
    expect(() =>
      evaluateConfigFunction(() => {
        defineBudget({ frameSeconds: 1 });
      }),
    ).toThrow(/mixes defineConfig/);
  });

  it("rejects defineRepo outside the user-global layer", () => {
    expect(() =>
      evaluateConfigFunction(
        () => {
          defineRepo("my-repo", {});
        },
        { layer: "repo" },
      ),
    ).toThrow(/defineRepo\("my-repo", \.\.\.\) is only allowed in the user-global config/);
  });

  it("allows defineRepo in the user-global layer", () => {
    const config = evaluateConfigFunction(
      () => {
        defineRepo("my-repo", { git: { longSeconds: 60 } });
      },
      { layer: "user-global" },
    );

    expect(config).toEqual({ repo: { "my-repo": { git: { longSeconds: 60 } } } });
  });
});
