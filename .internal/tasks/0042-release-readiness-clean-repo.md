# 0042 — Release readiness: what a clean repo must be able to do

**Status:** ordering task — each item links its own work · **Raised:** 2026-07-29

The acceptance test for release is one walkthrough in an empty directory:

```sh
ctx traits init
ctx traits new my-trait            # scaffolds, builds, checks
ctx traits dependency add @ctx-traits/implement
ctx traits trust approve implement
ctx traits run implement:quick --set phase=…
```

Everything below is something that walkthrough currently hits.

## A. The family shape is unfinished — this is the blocker

Three surfaces assume **one package = one canonical document**. Folded families
broke all three, and they are the packages worth shipping:

| surface | what happens today |
|---|---|
| `ctx traits list` | `implement`, `plan`, `refactor` report **`status=source-only`** |
| `ctx traits publish` | refuses: `publish requires exactly one trait package, found 5` |
| vendor layout | cannot carry a family at all |

Verified against a build of `8d24168`:

```
implement status=source-only source=…/packages/implement/source/index.ts
plan      status=source-only source=…/packages/plan/source/index.ts
refactor  status=source-only source=…/packages/refactor/source/index.ts
```

The inventory looks for `generated/index.toml`; a family has
`generated/quick/index.toml`, `generated/default/index.toml`, and so on. The
traits *run* correctly — `implement:quick` resolves and has driven every run
today — but the first command a new user types reports the flagship package as
unusable.

**LANDED 2026-07-29 — list and publish. Vendor (0013) remains.**

`list` now reports `traits: 8 · resolved: 8 · source-only: 0`, with each family
resolving through its DEFAULT leaf — the same leaf a bare-id run resolves to, so
`list` and `run` agree about what `implement` means. Two sites had to change
together (`discovery::trait_packages` and `inventory::repo_authored_candidate`):
fixing only one makes the id appear as a candidate and then resolve to nothing,
which is exactly how the source-only rows arose.

Publish landed as **0040** (archived). Vendor is **0013** and is still open —
until it lands, a family can be published but not vendored into a consumer.

## B. Templates are on the pre-P569 layout — LANDED 2026-07-29

All five templates renamed to `package.toml`/`package.lock`, and both places
`modules/core/build.rs` names the path updated (the read, and the path baked
into the generated `include_str!`). These were **not** cosmetic leftovers:
`build.rs` reads each template's manifest and panics if it is missing, so they
were load-bearing files on the retired layout.

`generated/` stays — `package_build_paths` deliberately routes template
packages there so a template can be built in place.

Verified by scaffolding from all five templates in a clean repository: each
builds and emits current-layout files.

The original finding follows.

All five built-in templates (`code-review`, `implement-phase`, `pr-description`,
`research-summarize`, `test-writer`) ship `trait.toml` and `trait.lock`. P569
renamed those to `package.toml` and `package.lock`.

`ctx traits new` is the second command in the walkthrough, and it scaffolds the
legacy shape — so a new user's first authored package is born stale, in a
naming scheme the docs no longer describe.

Decide per template whether it ships at all while renaming; five templates is a
lot of surface to keep correct for a first release, and each one that stays is a
thing that must never drift again.

## C. npm: decide the public surface

Five packages, all `0.1.0-alpha.0`, none marked private, no decision recorded:

```
@ctx-traits/agents          @ctx-traits/cdk       @ctx-traits/config
@ctx-traits/harness-client  @ctx-traits/rust
```

**0031** is the design task and is still `design needed`. It cannot be skipped:
publishing all five commits us to supporting all five, and `dependency add` in
the walkthrough resolves against whatever we publish. Its own proposal — cdk and
config public, agents/harness-client/rust arguably internal — needs a ruling,
plus the gate that keeps the split from rotting.

## D. Which repo traits ship

Eight packages in `.ctx/traits/packages/`, all `status = "ready"`:

```
auto-research  deep-research  engineering-standards  guarded-change
implement      implement-guided  plan  refactor
```

That is the publish list until someone says otherwise. Open questions worth
answering before it becomes a compatibility promise: whether
`engineering-standards` ships separately or folds into its consumer (the
standing preference is folding until a second consumer exists), and whether
`implement-guided` and `implement` are both public or one is a variant of the
other.

Seven built-ins ship inside the binary (`critique-trait`, `explain-trait`,
`generate-evals`, `generate-trait`, `import-trait`, `refine-trait`,
`trait-spec`) and need the same yes/no pass.

## E. Polish, not blocking

- **Trust store noise.** 215 `(digest-only)` entries and 226 orphaned, plus
  development leftovers still named in it (`check-probe`, `demo-trait`,
  `hid-trait`). A clean repo starts empty so the walkthrough is unaffected, but
  `trust list` is currently unreadable, and it is a command we point people at.
- **CLI surface.** 71 top-level commands, 50 hidden. Six bare verbs (`install`,
  `remove`, `update`, `outdated`, `info`, `publish`) are pre-P567 aliases now
  shadowed by the `dependency` group; `inspect` and `generate-evals` both claim
  the alias `map`. Both are cheap to settle before the surface is public and
  expensive after.

## Watch

- **A and B are the only true blockers.** C and D are decisions that must be
  made but cost nothing to make; E can ship after. Do not let the list grow past
  what the walkthrough actually hits.
- **Run the walkthrough in a real empty directory, not a fixture.** Every defect
  above was found by inspecting this repository, where everything is already
  built, approved, and warm. The clean-repo path is exactly the one nothing here
  exercises.
- Publishing sets a compatibility surface. Every id in C and D becomes a name we
  either keep or break; the cheapest moment to ship fewer things is now.

## Done when

The walkthrough completes in an empty directory: `list` reports every package
accurately including families, `new` scaffolds a current-layout package that
builds and checks, the published npm surface is exactly what was decided, and a
family package installs and runs from the registry with digests matching source.
