# 0042 — Release readiness: what a clean repo must be able to do

**Status:** DONE 2026-07-29 — every item landed, ruled, or recorded below · **Raised:** 2026-07-29

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

Publish landed as **0040** (archived). Vendor landed as **0013** (archived):
a family vendors, resolves, and trust-approves in a clean consumer — the
staged-package root-resolution bug behind "does not match locked evidence" is
root-caused and fixed (`has_package_manifest`).

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

**RULED 2026-07-29: cdk + config public; agents, harness-client, rust
internal.** The three internal packages carry `"private": true`, so `npm
publish` mechanically refuses them — that is 0031's anti-rot gate. Verified
that neither public package imports an internal one (no `@ctx-traits/*` in
cdk's or config's dependencies), so the split is clean at the import graph too.

## D. Which repo traits ship

Eight packages in `.ctx/traits/packages/`, all `status = "ready"`:

```
auto-research  deep-research  engineering-standards  guarded-change
implement      implement-guided  plan  refactor
```

**Standing answer 2026-07-29: all eight ship** — every one stages clean
through `publish --dry-run`. The two sub-questions (fold
`engineering-standards` into its consumer; `implement-guided` vs `implement`)
stay open as post-v1 shaping, explicitly not release blockers.

Seven built-ins ship inside the binary (`critique-trait`, `explain-trait`,
`generate-evals`, `generate-trait`, `import-trait`, `refine-trait`,
`trait-spec`) and need the same yes/no pass.

## E. Polish, not blocking

- **CLI surface — PRUNED 2026-07-29 (owner ruling).** The six pre-P567 bare
  verbs are deleted from the enum and dispatch; the `dependency` group is the
  one spelling. The reported `map` alias collision was a false finding — both
  are `--source-map` FLAG aliases on different commands, scoped per command,
  no conflict. Correction recorded so nobody "fixes" it.
- **Trust-store noise — residual, deliberately unshipped.** 215 digest-only /
  226 orphaned entries plus dev leftovers (`check-probe`, `demo-trait`,
  `hid-trait`) on this machine. No prune verb exists (`trust list --stale`
  only reports). Machine-local and empty in a clean repo, so the walkthrough
  never sees it; a `trust prune` verb is post-v1 work.

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

**Closing status 2026-07-29:** every leg verified except the literal
registry round-trip, which cannot exist before the first `npm publish` —
`path:` install stands in for it and exercises the same staging, lock, vendor,
and approval machinery. The registry leg is the first thing to verify the day
the packages go up.
