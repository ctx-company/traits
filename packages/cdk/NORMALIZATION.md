# Normalization: how the emitter keeps moving code from reading as drift

Binding authority: `.internal/tasks/0102` ("only the relative position of steps and `flow.*`
statements is semantic") and `.internal/tasks/0104` (this file). Read alongside `src/normalize.ts`
and `src/sequence.ts`, which this documents as landed — not aspirationally.

## What emits in a fixed order, regardless of authoring order

- **Object keys.** Every JSON object the emitter produces goes through `stableObject`
  (`normalize.ts`), which sorts keys by `String.localeCompare`. This has always been true — it is
  not part of 0104's change.
- **Declaration-array kind order.** A trait draft's top-level declaration arrays
  (`agent`, `condition`, `port`, `prompt`, `resource`, `schema`, `sequence`, `session`, `signal`,
  `slot`) always appear in that fixed list order (`declKinds` in `normalize.ts`).
- **Declaration-array item order, per kind.** Within one kind's array, items are sorted by
  `id.localeCompare` (`mergeDeclarationSets`, `normalize.ts`). There is no registration counter,
  no authoring-order tiebreak, and no "first declared wins" — the sort key is the id alone. Two
  declarations sharing an id must be byte-identical (the same declaration re-collected through a
  different path); anything else is a genuine id collision and throws.
- **Generated ids.** A `sequence.branch`/`sequence.loop`/`sequence.forEach` step authored with an
  inline `body:`/`then:`/`otherwise:` array (CDK grammar v2 rule 7) gets its generated sub-sequence
  id from a deterministic base id derived from the enclosing scope and the step's own id
  (`generatedBranchArmId`/`generatedLoopGateBodyId`, `sequence.ts`) — never a counter suffix. A
  generated id colliding with another declaration (generated or authored) is a build error naming
  both source anchors (`resolveGeneratedBranchArmIds`, `normalize.ts`), not a silent uniquify —
  uniquifying by counter would smuggle authoring-order-dependence right back into the "stable"
  array this section describes.

## What stays positional (the semantic part, per 0102)

- **Step arrays inside one `sequence:` literal.** The order of steps inside one authored
  `sequence.linear(id, [...])` array, or a procedure's top-level `sequence: [...]` array, is
  explicit syntax and the author's — it is never reordered. Moving a `flow.until`/`flow.abortIf`
  call, or reordering two steps, changes the canonical document, correctly.
- **`flow.*` statement position.** Where a `flow.until`/`flow.abortIf` guard sits relative to the
  steps around it inside one loop body is semantic and stays exactly as authored.

## Id derivation: title → id

`idFromTitle` (`sequence.ts`) derives a slug id from a declared title: lowercase, non-alphanumeric
runs collapsed to a single hyphen, validated against the same slug grammar every other id in the
CDK uses (`validateSlug`, `normalize.ts`). It is `titleFromId`'s inverse. An explicit `id` passed
to a step builder's first argument always stays authoritative over anything `idFromTitle` would
derive from that step's title — `idFromTitle` exists as the entry point future title-only
(functional-layer) step authoring calls to mint its own id, not to override an explicit one.

**Duplicate-title validation applies now, independent of whether any step actually uses
`idFromTitle` to derive its own id.** Within one scope — one `sequence.linear(...)` items array, or
a procedure's top-level `sequence:` array — two steps whose titles derive the same id
(`validateNoDuplicateTitles`, `sequence.ts`) is a build error, even though today every step's `id`
is authored explicitly and would never actually collide on its own. This is deliberate: it fails
loudly now, before any functional-layer surface starts deriving ids from titles, rather than
letting a latent title collision surface as a confusing error only once that surface lands.

## What is scoped out of 0104

- The `<step-id>-2`, `<step-id>-3`, ... auto-declared output-slot suffix (a per-step counter over
  that one step's own `output:` list, `sequence.ts`) stays a counter. Per 0102, order inside one
  literal — here, one step's own `output:` array — is explicit syntax and the author's, so
  counting through it is not an authoring-order leak in the sense this document is about.
- Rust-side display/import fallbacks (`format!("step-{}", index + 1)` in
  `modules/cli/src/app/report_check.rs` and `import_analysis.rs`) are not part of the emission
  path this document covers.
