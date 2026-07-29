# 0036 — Package budgets move to `runtime.toml`, one file with variant tables

**Status:** ready to implement · **Raised:** 2026-07-29 · **Do together with:** 0026 (variant rename) · **Tiers defined by:** 0037

## Today

```
.ctx/traits/packages/implement/run-config/default.toml
                                          /phase.toml
                                          /quick.toml
```

Each declared separately in the package manifest as
`run-config = "run-config/<variant>.toml"`, and each containing only:

```toml
schema-version = "0.1"
[budget]
max-frames = 20
frame-seconds = 1200
```

Two problems: `run-config` names a scope it does not have (the schema permits
`[budget]` and nothing else — P312 makes `[assign]`, `[worktree]`, harness or
model a hard decode error), and N near-identical files exist where one would do.

## Target

**One `runtime.toml` per package**, same filename as the machine-tier document,
disambiguated by location:

```
.ctx/traits/runtime.toml                        machine   full schema
.ctx/traits/packages/<id>/runtime.toml          package   [budget] only
```

Top-level keys are the package default; a variant table overrides:

```toml
frame-seconds = 1200
total-seconds = 3600

[variant.quick]
frame-seconds = 900
```

The qualifier is `variant.<vid>`, matching `trait.<id>` — named by kind, so a
bare `[quick]` cannot collide with a future top-level key.

## Where this sits

The package tier is the AUTHOR's layer: budget defaults shipped with the trait.
A machine owner overrides them in their own `runtime.toml` via
`[trait.<id>.budget]` or `[trait.<id>.variant.<vid>.budget]` (0037). Both tiers
share the filename; only the package tier is restricted to `[budget]`.

## Why one filename, not a third

Both documents answer the same question — how a run is bounded — so they should
share a name. A third filename (`budget.toml`) would imply a third concept.

The rule that makes this safe, and that should be written down as doctrine:

> **Permission narrows as authority moves away from the machine owner.**
> Machine tier: the full schema. Package tier: `[budget]` only.

The restriction is ENFORCED by schema, not conveyed by the filename, so the
shared name cannot mislead anyone into thinking a vendored package can bind
seats or touch a worktree. This also matches the seat-scoping model in 0034,
where scope narrows the same way.

## Why it is not a file rename

The per-variant paths are declared in the FAMILY MANIFEST, so consolidating
means retiring `[family.leaf.<selector>].run-config`, changing manifest
validation, and changing the CDK that emits it. Every family manifest on disk
in both repos is invalidated.

That is also why this pairs with 0026: both edit the same manifest table, and
doing them separately means rewriting every package manifest twice.

## Watch

- The manifest key is persisted. Needs a compatibility read of the old
  `run-config` declaration, like the P569 path renames, or every committed
  package breaks.
- Variant-table resolution must use the SAME merge rule as everything else:
  stated replaces, omitted inherits. A variant table is not a full replacement
  of the package defaults.
- Do not let the shared filename tempt anyone into widening the package schema
  "for symmetry". The asymmetry is the point.

## Done when

Each package carries one `runtime.toml` holding budgets with variant tables;
`run-config` declarations no longer exist but old ones still read; the package
schema still rejects everything but budget; the permission-narrowing rule is
documented where an author will see it.
