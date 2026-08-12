# @ctx-traits/config

Typed TypeScript surface for `ctx` config documents. `config.ts` never
evaluates at load time — `ctx traits config build` compiles it into
`config.toml` once, ahead of any command that reads config.

## Install

```sh
pnpm add -D @ctx-traits/config
```

## Author `config.ts`

Default-export a function of `define*` registrar calls — one registrar per
`config.toml` table. Singleton registrars (`defineRun`, `defineWorktree`,
...) may only be called once per file; keyed registrars (`defineRole`,
`definePricing`, ...) may only be called once per key.

```ts
// .ctx/config.ts — evaluated only by `ctx traits config build`
import { definePricing, defineRole, defineRun, configureTrait, defineWorktree } from "@ctx-traits/config";

export default function () {
  defineRole("master", {
    harness: "claude",
    model: "claude-opus-5",
    budget: { frameSeconds: 1200, maxRetries: 3 },
  });
  defineRole("smart", [
    { harness: "claude", model: "claude-opus-5" },
    { harness: "codex", model: "gpt-5.2" },
  ]);
  defineRun({ frameSeconds: 1200, maxFrames: 1000, maxCostUsd: 25 });
  defineWorktree({ env: { CARGO_TARGET_DIR: "{cache-slot}" } });
  definePricing("claude-opus-5", { usdPerMtok: 15 });
  configureTrait("refactor", {/* per-trait defaults */});
}
```

`defineRepo` is only legal in the user-global config (`~/.config/ctx/config.ts`
or equivalent) — `ctx traits config build` rejects it in a repo-local build,
the same rule the loader already enforces at read time.

The object-literal form, `export default defineConfig({...})`, still works
during a deprecation window but is not the recommended surface — mixing
`defineConfig` with `define*` registrar calls in one file is an error.

## Source

https://github.com/ctx-company/traits (`packages/config`).
