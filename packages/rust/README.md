# @ctx-traits/rust

Dual-use package: shared `ctx.traits` CDK doctrine for Rust-specialized
reviewed-change procedures, and a complete vendorable canonical trait package
built from that same doctrine.

## Consumption mode 1 — TypeScript authoring import

Trait CDK sources (`.ctx/traits/<id>/source/index.ts`) import the shared
Rust-specialized role builders as ordinary TypeScript, alongside
`@ctx-traits/cdk` and `@ctx-traits/agents`:

```sh
pnpm add -D @ctx-traits/rust
```

```ts
import { rustReviewerRole, rustWorkerRole } from "@ctx-traits/rust";
import { port, prompt, sequence, slot, trait } from "@ctx-traits/cdk";

const worker = rustWorkerRole("worker", "Implements the requested Rust refactor.");
const reviewer = rustReviewerRole("reviewer", "Reviews the implemented Rust refactor.");
```

`rustWorkerRole`/`rustReviewerRole` wrap the generic `workerRole`/
`reviewerRole` builders from `@ctx-traits/agents` — they add only
Rust-specific description text, never a reimplementation of the shared
doctrine. This import is authoring-time only: it resolves through ordinary
package management, never through `ctx.traits` dependency resolution, and
the consumer's synthesized `generated/index.toml` stays fully self-contained.

## Consumption mode 2 — canonical trait dependency

The package also carries a complete canonical trait package at
`.ctx/traits/rust/`, built from the same `src/index.ts` doctrine — the
Rust-specialized `worker`/`reviewer` agent roles, and the
`engineering-standards`/`gate-conventions` resources this repo's own Rust
core is held to — for `ctx traits sync` dependency use:

```toml
# .ctx/traits/<consumer>/trait.toml
[dependencies]
rust = { npm = "@ctx-traits/rust", version = "0.1.0", package-path = ".ctx/traits/rust" }
```

For an in-repo consumer (no npm publish involved), use a local path instead:

```toml
[dependencies]
rust = { version = "0.1.0", path = "../rust" }
```

`package-path` (npm/git sources) or `path` (local sources) points at the
nested canonical package root — the npm package root has no `trait.toml`,
only `.ctx/traits/rust/` does. The trait's own `trait.toml` declares an empty
`[dependencies]` table: this is a leaf schema/doctrine trait with no
cross-trait dependencies of its own.

`ctx traits sync` installs, vendors, and locks it like any other trait
dependency; declaring it makes `agent:rust/worker`, `agent:rust/reviewer`,
`resource:rust/engineering-standards`, and `resource:rust/gate-conventions`
addressable through typed refs without auto-activating any behavior. The
`refactor-rust` showcase trait in this same package (`.ctx/traits/refactor-rust/`)
demonstrates consuming the two resources via `ref.resource({ id, dependency: "rust" })`.

## Package contents

- `src/index.ts` — the single authored source: `rustWorkerRole`,
  `rustReviewerRole`. Pure draft builders only, built on top of
  `@ctx-traits/agents`'s `workerRole`/`reviewerRole` — no filesystem access,
  commands, provider calls, synthesis, or output writes.
- `dist/` — compiled TypeScript exports (`tsc` output). The workspace's own
  `exports` field resolves consumption mode 1 to `./src/index.ts` directly;
  `publishConfig.exports` points published consumers at `dist/`.
- `.ctx/traits/rust/` — the canonical `rust` trait package (manifest, CDK
  source, generated TOML/map, lock, `resources/`) for consumption mode 2,
  exposing `agent` and `resource` symbols built from the same shared
  role/doctrine source as consumption mode 1.
- `.ctx/traits/refactor-rust/` — a showcase trait (`family = "refactor"`,
  `variant = "rust"`): a minimal, non-looping implement-then-review
  procedure for a Rust change, declaring `rust` as a `[[dependency]]` and
  referencing its two resources by typed ref. This is repository showcase
  source only — it demonstrates consuming `rust` as a dependency and is not
  listed in `package.json`'s `files`, so it is excluded from the published
  npm tarball; only `.ctx/traits/rust/` ships in consumption mode 2.

## Build

All of the following exit `0` when run from the repository root, in order:

```sh
pnpm --dir packages/rust run build        # tsc -> dist/
pnpm --dir packages/rust run typecheck
pnpm --dir packages/rust run lint
pnpm --dir packages/rust run format:check
ctx traits build packages/rust/.ctx/traits/rust/source/index.ts
ctx traits build packages/rust/.ctx/traits/refactor-rust/source/index.ts
ctx traits sync --file packages/rust/.ctx/traits/rust/generated/index.toml
ctx traits sync --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml
ctx traits check --file packages/rust/.ctx/traits/rust/generated/index.toml
ctx traits check --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml
```

`ctx traits sync --file` takes the generated (synthesized) trait file, never the
hand-authored `trait.toml` manifest — this matches every other package in this
repo. The unlocked `ctx traits check` above is this package's green DoD gate;
it reports `passed: true` for both traits.

### Known limitation — `--locked` checking

`ctx traits check --file <generated/index.toml> --locked` currently reports
`passed: false` for both traits, for two separate reasons that are both
pre-existing CLI behavior outside this package's scope:

- **Missing locked export evidence** (`rust` and `refactor-rust`) — only
  `ctx traits export --update-skill-lock` records that evidence, and no trait
  in this repo runs it by default; `packages/agents`'s own `agents` trait has
  the identical gap. This is parity, not a regression.
- **Model-view digest mismatch** (`refactor-rust` only) —
  `ctx_traits_io::export::infer_repo_root_from_trait_file`
  (`modules/io/src/export.rs:106`) unwraps exactly one `.ctx/traits/<id>`
  level, so for the doubly-nested path
  `packages/rust/.ctx/traits/refactor-rust/...` it infers `packages/rust` as
  the repo root. `ctx traits sync` (manifest discovery from cwd) correctly
  vendors the `rust` dependency at the real repo root
  `.ctx/traits/vendor/rust`, so the two codepaths disagree on the vendor root
  and `check --locked` renders the model view without the dependency's
  materialized resources. This is a CLI resolver bug that predates this
  package and needs a fix in `modules/io/src/export.rs`, not a change in
  `packages/rust` — tracked as a follow-up phase.

## Running `refactor-rust`

`ctx traits run` needs live model credentials and an owner-configured
`.ctx/config.toml` (harness registry, `[agent.role.<role>]` routing, and
`[worktree]` preparation), so it is not run as part of this package's build
validation. The owner runs it directly, optionally layering `--assign
role=harness[:transport[:session-mode[:model[:reasoning-effort]]]]` overrides
for anything that should differ from their standing `.ctx/config.toml`
mapping:

```sh
ctx traits run --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml --set target=<crate-or-module-path>
```

The run session ledger lands under `~/.config/ctx/runs/<repo-key>/`, this
repository's global per-repository run-ledger root — machine-local runtime
state, never repo-local and never committed (P426; run `ctx traits doctor
--migrate-state` to see the resolved path). `.ctx/config.toml` itself stays
gitignored, machine-local configuration.
