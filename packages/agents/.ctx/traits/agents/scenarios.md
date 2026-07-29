# Scenarios

Illustrative dependency-consumption scenarios for `agents`. These describe
how a dependent trait uses the declared symbols; they are not executable
tests (this repo's current validation stage is CLI-gate based, not a test
harness — see `PRODUCT.md`'s Validation Gates).

## Scenario: a two-reviewer refinement loop reuses the shared doctrine

A trait like `implement-phase` imports `REVIEW_VERDICT_DOCTRINE`,
`workerRole`, and `reviewerRole` from `@ctx-traits/agents` at TypeScript
authoring time (consumption mode 1) and composes them into its own local
prompt and agent declarations — e.g. its reviewer step's prompt is built with
`prompt.template` from its own phase-specific opening line plus the imported
`REVIEW_VERDICT_DOCTRINE` string, bound to the dependent's own phase-brief
and product-brief slots. The dependent may additionally declare `agents` in
`[dependencies]` (consumption mode 2) so `schema:agents/review-verdict` is
addressable as the reviewer step's output schema through a typed ref. No
behavior activates from declaring the dependency alone, and no operation
splices a dependency prompt's body into another prompt at the canonical
layer — the composition happens once, in TypeScript, before the trait is
built.

## Scenario: editing the shared doctrine surfaces as reviewable drift

An owner edits `REVIEW_VERDICT_DOCTRINE` in `packages/agents/src/index.ts`
(say, to tighten the escalation wording) and rebuilds the canonical `agents`
trait (`ctx traits build`). Every dependent that declares `agents` as an npm
dependency sees the change on its next `ctx traits sync` as an updated
locked digest, and `ctx traits check --locked` fails until the dependent
re-syncs and reviews the diff — the change never reaches a dependent's
behavior silently.

## Scenario: a fresh consumer resolves the dependency without executing TypeScript

A new trait's `trait.toml` declares:

```toml
[dependencies]
agents = { npm = "@ctx-traits/agents", version = "0.1.0", package-path = ".ctx/traits/agents" }
```

`ctx traits sync --locked` resolves `@ctx-traits/agents` from
`node_modules` (or vendors it), reads the nested canonical package at
`package-path`, and records its digests in the consumer's lockfile —
without running `node` against any TypeScript inside the dependency. The
consumer only ever reads the dependency's already-synthesized
`generated/index.toml`.
