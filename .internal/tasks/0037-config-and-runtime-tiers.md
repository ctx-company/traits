# 0037 — `config.toml` (committed, shared) and `runtime.toml` (local, mine)

**Status:** ready to implement · **Raised:** 2026-07-29 · **Supersedes the single-file layout in 0034/0036**

## The split

Two documents, one schema, distinguished by who they bind:

```
.ctx/traits/
├── vendor.toml           committed   what this project depends on
├── config.toml           committed   what the PROJECT decides, for everyone
├── runtime.example.toml  committed   the template for the file below
├── runtime.toml          IGNORED     what I decide, on THIS machine
└── packages/<id>/
    ├── package.toml      committed   identity + variants
    └── runtime.toml      committed   the author's budget defaults (budget only)
```

`config.toml` carries decisions that travel with the repository — "this project
runs in worktrees", "`pnpm install` runs first", the merge gate, seeds.
`runtime.toml` is machine-local: seats, models, credentials, absolute paths, and
any budget this machine wants different.

## Same schema in both tiers

**Both accept the same shape.** The tier carries the meaning, not the key set.

This is deliberate: partitioning keys by kind (worktree here, agents there)
creates a rule every author must memorise and every reviewer must enforce. With
one schema the only question is *"shared, or mine?"* — which is a question the
author can already answer.

It also allows a legitimate case the partitioned version forbids: a project
committing a seat decision, e.g. reviewers run at high effort. Models,
credentials and machine paths land in `runtime.toml` because they are
machine-specific — guidance in the scaffold comments, not a schema restriction.

## Precedence

```
built-in  <  package runtime.toml  <  machine ~/.config  <  config.toml  <  runtime.toml
             (trait author)          (this machine)        (project)       (mine)
```

Merging is **field-wise, no exceptions** — the rule already established for
`[harness.*]` (P568) and reused for `[trait.<id>]` seats: a stated field
replaces, an omitted field inherits, `field = ""` explicitly unsets. A local
`runtime.toml` stating one field overrides that field and nothing else.

## Scaffolding

`ctx traits init` writes both:

- **`config.toml`** — populated with real project defaults and comments
  explaining each. This is a file the project fills in.
- **`runtime.example.toml`** — fully commented, every knob present but commented
  out showing its default, per task 0019's rule: a written value freezes today's
  default forever, a commented one keeps inheriting.

`runtime.toml` itself is never scaffolded; it is copied from the example by
whoever needs it, and is gitignored.

## Watch

- **Merge once, at resolution** (0035). Five layers is more chances to get this
  wrong, not fewer. A raw `.get(id)` on the resolved document must always be
  correct.
- The package tier stays **budget-only** even though it shares a filename —
  permission narrows as authority moves away from the machine owner (0036). The
  shared name must not tempt anyone into widening it for symmetry.
- `doctor --config` must name the winning tier for every knob. With five layers
  this stops being a nicety.
- Committing a model in `config.toml` binds collaborators to a model they may
  not have access to. Allowed, but the scaffold comments should say so.

## Done when

Both documents resolve through one field-wise merge in the stated order; `init`
scaffolds a populated `config.toml` and a fully-commented
`runtime.example.toml`; `runtime.toml` is gitignored and never scaffolded;
`doctor --config` names the winning tier.
