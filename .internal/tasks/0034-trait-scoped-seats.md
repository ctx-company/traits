# 0034 — `[trait.<id>]` seat overrides: one shape, three scopes

**Status:** ready to implement · **Raised:** 2026-07-29 · **Pairs with:** 0025 (role arrays), 0037 (config/runtime tiers)

## What exists today

Seats are global. `[agent.role.worker]` means *the* worker for every trait, and
a trait that needs a different one is handled with `--assign` at invocation —
baked into a Justfile recipe, invisible to `doctor --config`, and unversioned
next to the config it contradicts. The repo's own Justfile documents the
workaround: deep-research's worker differs from the implement family's, "so
this recipe layers its own override instead of changing the shared default."

## What to add

A third scope carrying the SAME `AgentDefaults` shape the other two already do:

```
[agent…]                                   global
[repo.<key>.agent…]                        this checkout   (exists; machine-tier only)
[trait.<id>.agent…]                        this trait      (new)
[trait.<id>.variant.<vid>.agent…]          one variant     (new)
```

```toml
[agent.role.worker]
harness = "claude-code"
model = "claude-sonnet-5"

[trait.deep-research.agent.role.worker]
harness = "opencode"          # inherits model + effort from the global worker

[trait.plan.agent.role.smart]
count = 1                     # narrows the global count = 2
```

## Why this shape and not a new mechanism

`RepoOverride` already holds an `AgentDefaults`, so qualified seat config is an
established pattern here — this adds a qualifier, not a concept. Anyone who
understands `[repo.*]` understands this without reading anything.

Merge semantics are the ones P568 established for `[harness.*]`: a stated field
REPLACES, an omitted field INHERITS, and `field = ""` explicitly unsets. That
is why the deep-research example above can state `harness` alone and keep the
global model — same rule, different table.

Role arrays (0025) compose for free, because `count` is just another field.

## The line that must hold

Only `agent` is trait-scopable. `worktree`, `run`, `merge` and `harness` stay
unqualified: they describe THIS MACHINE AND CHECKOUT, and a trait has no
business changing a build cache, a confinement policy, or a merge gate.

This is also what keeps the doctrine intact. A vendored trait package still
cannot bind seats — its sidecar remains budget-only (P312 makes `[assign]` a
hard decode error there). Scoping is a power the MACHINE OWNER gets in their
own machine-local runtime config, never one a dependency gets over its consumer.

## Decisions taken

- `[trait.<id>]` is allowed in the repo tier as well as the machine tier —
  unlike `[repo.*]`, which is machine-tier-only because it names checkouts.
- When both `[repo.X]` and `[trait.Y]` apply, **trait wins**: it is the more
  specific qualifier.

## Watch

- Resolution order must be one function, not a rule re-implemented per consumer.
  P568's narrator bug came from exactly this: the merge existed, but a lookup
  site read the raw map and saw a half-defined harness. Merge once, at config
  resolution, so every consumer reads an already-resolved seat.
- `doctor --config` must show the EFFECTIVE seat with provenance naming the
  winning scope, otherwise the feature reintroduces the invisibility that makes
  the `--assign`-in-a-recipe workaround bad.
- An unknown trait id in `[trait.<id>]` should warn, not fail — a config may
  legitimately outlive a trait — but it must not be silent.
- Retire the `--assign` overrides from Justfile recipes once this lands, or the
  two mechanisms will disagree and the recipe will win invisibly.

## Done when

A trait-scoped seat overrides the global one field-wise; `count` narrows per
trait; trait beats repo scope; `doctor --config` names the winning scope;
package sidecars still cannot bind seats; the Justfile's `--assign` workarounds
are gone.

## Qualifier grammar

Qualifier tables are named by their KIND — `trait.<id>`, `variant.<vid>`,
`repo.<key>` — never by a bare name. A bare `[quick]` table would collide with
any future top-level key of that name, and reads as a section rather than a
scope.

The same grammar carries budgets (0037), so `trait.<id>` and `variant.<vid>`
mean the same thing wherever they appear:

```toml
[trait.implement.budget]                  frame-seconds = 3600
[trait.implement.variant.quick.budget]    frame-seconds = 900
[trait.implement.agent.role.worker]       harness = "opencode"
```
