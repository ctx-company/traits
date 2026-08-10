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
import { cargoFixLoop, rustReviewerRole, rustWorkerRole } from "@ctx-traits/rust";
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

`cargoFixLoop` is the third export: a `@ctx-traits/toolkit` sequence factory
(re-exported here) that turns a real `cargo check` + `cargo clippy` run into
the fix loop described under "Consumption mode 3" below.

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

## Consumption mode 3 — the `cargo-fix` trait

The compiler is the reviewer (task 0119): `.ctx/traits/cargo-fix/` is a
standalone canonical trait wiring `cargoFixLoop` to a `rustWorkerRole` seat.
No model is ever on the diagnostic-parsing path — a deterministic Node
script (embedded in the built canonical, run via `node --eval`) spawns
`cargo check`/`cargo clippy --message-format json` itself, keeps only
`reason == "compiler-message"` lines, projects `{file, line, column, code,
level, message}` from each diagnostic's primary span, dedupes across both
commands on `(file, line, column, code)`, and sorts by that same key —
byte-identical evidence across runs. An unrecognized diagnostic shape
degrades loudly (stderr + non-zero exit) rather than silently dropping it.

The bounded loop that follows drives the worker through the reduced list,
recapturing after every round, until a fresh capture is clean (default
policy: errors block, warnings are reported but don't block —
`cargoFixLoop({ denyWarnings: true })` widens the guard to require zero
diagnostics of any level). A run that never clears the list exhausts into
`onExhausted: "abort"`: it blocks rather than reaching a commit with
unresolved compiler diagnostics.

```toml
# .ctx/traits/<consumer>/trait.toml
[dependencies]
cargo-fix = { npm = "@ctx-traits/rust", version = "0.1.0", package-path = ".ctx/traits/cargo-fix" }
```

Or compose the factory directly in your own trait source instead of
depending on the packaged trait:

```ts
import { cargoFixLoop, rustWorkerRole } from "@ctx-traits/rust";

const worker = rustWorkerRole("worker", "Fixes cargo diagnostics.");
// sequence: [...cargoFixLoop({ worker, rounds: 4 })]
```

Running the packaged trait is an owner step — see "Running `refactor-rust` /
`cargo-fix`" below.

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
  npm tarball; only `.ctx/traits/rust/` and `.ctx/traits/cargo-fix/` ship in
  the published tarball.
- `.ctx/traits/cargo-fix/` — the `cargo-fix` canonical trait for
  consumption mode 3: a `rustWorkerRole` seat wired to the
  `cargoFixLoop` factory, no `rust` trait dependency (it needs neither
  resource). Listed in `package.json`'s `files`, unlike `refactor-rust` —
  this is the package's product demo, not showcase source.

## Build

All of the following exit `0` when run from the repository root, in order:

```sh
pnpm --dir packages/rust run build        # tsc -> dist/
pnpm --dir packages/rust run typecheck
pnpm --dir packages/rust run lint
pnpm --dir packages/rust run format:check
ctx traits build packages/rust/.ctx/traits/rust/source/index.ts
ctx traits build packages/rust/.ctx/traits/refactor-rust/source/index.ts
ctx traits build packages/rust/.ctx/traits/cargo-fix/source/index.ts
ctx traits sync --file packages/rust/.ctx/traits/rust/generated/index.toml
ctx traits sync --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml
ctx traits sync --file packages/rust/.ctx/traits/cargo-fix/generated/index.toml
ctx traits check --file packages/rust/.ctx/traits/rust/generated/index.toml
ctx traits check --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml
ctx traits check --file packages/rust/.ctx/traits/cargo-fix/generated/index.toml
```

`ctx traits sync --file` takes the generated (synthesized) trait file, never the
hand-authored `trait.toml` manifest — this matches every other package in this
repo. The unlocked `ctx traits check` above is this package's green DoD gate;
it reports `passed: true` for all three traits.

### Known limitation — `--locked` checking

`ctx traits check --file <generated/index.toml> --locked` currently reports
`passed: false` for `rust` and `refactor-rust` (`cargo-fix` passes: it
declares no `[[dependency]]`, so neither reason below applies to it), for
two separate reasons that are both pre-existing CLI behavior outside this
package's scope:

- **Missing locked export evidence** (`rust` and `refactor-rust`) — only
  `ctx traits export --update-skill-lock` records that evidence, and no
  trait in this repo runs it by default; `packages/agents`'s own `agents`
  trait has the identical gap. This is parity, not a regression.
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

## Running `refactor-rust` / `cargo-fix`

`ctx traits run` needs live model credentials and an owner-configured
`.ctx/config.toml` (harness registry, `[agent.role.<role>]` routing, and
`[worktree]` preparation), so it is not run as part of this package's build
validation. The owner runs it directly, optionally layering `--assign
role=harness[:transport[:session-mode[:model[:reasoning-effort]]]]` overrides
for anything that should differ from their standing `.ctx/config.toml`
mapping:

```sh
ctx traits run --file packages/rust/.ctx/traits/refactor-rust/generated/index.toml --set target=<crate-or-module-path>
ctx traits run --file packages/rust/.ctx/traits/cargo-fix/generated/index.toml --set scope=<optional-package-name>
```

The run session ledger lands under `~/.config/ctx/runs/<repo-key>/`, this
repository's global per-repository run-ledger root — machine-local runtime
state, never repo-local and never committed (P426; run `ctx traits doctor
--migrate-state` to see the resolved path). `.ctx/config.toml` itself stays
gitignored, machine-local configuration.
