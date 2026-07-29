# @ctx-traits/agents

Dual-use package: shared `ctx.traits` CDK doctrine for reviewed multi-agent
procedures, and a complete vendorable canonical trait package built from that
same doctrine.

## Consumption mode 1 — TypeScript authoring import

Trait CDK sources (`.ctx/traits/<id>/source/index.ts`) import the shared
builders as ordinary TypeScript, alongside `@ctx-traits/cdk`:

```sh
pnpm add -D @ctx-traits/agents
```

```ts
import {
  clerkRole,
  deviationReportSchema,
  planAmendmentSchema,
  reviewerRole,
  REVIEW_VERDICT_DOCTRINE,
  scribeRole,
  VARIANT_DOCTRINE,
  workerRole,
} from "@ctx-traits/agents";
import { prompt, trait } from "@ctx-traits/cdk";

const smart1 = reviewerRole("smart-1", "Strong model: drafts the implementation plan, and reviews the work.");
const workerAgent = workerRole("worker", "Implements the draft and applies reviewer fixes.");
const scribe = scribeRole("scribe", "Writes the commit message for the completed phase from the execution plan");
const clerk = clerkRole("clerk", "Copies the phase section out of the execution plan verbatim.");
const phase = /* port.input.text(...) */;
const workSummary = /* slot.text(...) */;
const phaseBrief = /* slot.text(...) */;
const productBrief = /* slot.text(...) */;
const reviewText = prompt.template(
  `Review the implemented work for {phase}. Current work summary: {workSummary}.
  ${REVIEW_VERDICT_DOCTRINE}`,
  { phase, workSummary, phaseBrief, productBrief },
);
```

`VARIANT_DOCTRINE` is the shared reviewer-authority ladder: one static prompt
fragment per variant (`quick`, `default`, `smart`, `strict`), each stating how
much plan-fidelity forgiveness or amendment authority that variant grants
WITHOUT ever excusing a genuine correctness defect — splice the selected
fragment (e.g. `${VARIANT_DOCTRINE.smart}`) alongside
`REVIEW_VERDICT_DOCTRINE` in a review-step prompt. Each fragment opens with a
precedence statement declaring itself authoritative over
`REVIEW_VERDICT_DOCTRINE`'s own default completion-criteria paragraph for how
strictly the plan must be matched, so composing a fragment overrides that
paragraph without requiring any change to `REVIEW_VERDICT_DOCTRINE` itself;
genuine-defect, house-rule, required-gate, and duplication blocking stay
unconditional under every fragment. `deviationReportSchema`
and `planAmendmentSchema` are the typed records those fragments describe —
a strict-variant deviation report (planned/actual/rationale/disposition) and
a smart-variant plan amendment (change/contradiction-resolved/rationale/
provenance) — exported standalone for a consuming trait to compose into its
own slots or verdict schema as it chooses; neither is attached to
`reviewVerdictSchema` by this package.

This import is authoring-time only: it resolves through ordinary package
management (`pnpm install` links the in-repo workspace copy; external
consumers install the published package), never through `ctx.traits`
dependency resolution. It does not make the consuming trait depend on the
`agents` canonical trait package, and the consumer's synthesized
`generated/index.toml` stays fully self-contained — editing this package's
`src/index.ts` surfaces as reviewable CDK drift on every consumer that
rebuilds, never as a silent behavior change.

## Consumption mode 2 — canonical trait dependency

The package also carries a complete canonical trait package at
`.ctx/traits/agents/`, built from the same `src/index.ts` doctrine — the
generic `worker`/`reviewer`/`scribe`/`clerk` agent roles, the
`review-verdict-doctrine` prompt (spliced from `REVIEW_VERDICT_DOCTRINE`),
the `leftover-doctrine` prompt (spliced from `LEFTOVER_DOCTRINE`), and the
blocker/review-verdict/leftover schema — for `ctx traits sync` dependency use:

```toml
# .ctx/traits/<consumer>/trait.toml
[dependencies]
agents = { npm = "@ctx-traits/agents", version = "0.1.0", package-path = ".ctx/traits/agents" }
```

`package-path` is required: the npm package root has no `trait.toml`, only the
nested canonical package under `.ctx/traits/agents/` does. The trait's own
`trait.toml` declares an empty `[dependencies]` table: this is a leaf
schema/doctrine trait with no cross-trait dependencies of its own.

`ctx traits sync` installs, vendors, and locks it like any other npm-sourced
trait dependency; declaring it makes its symbols addressable through typed
refs (`agent:agents/worker`, `prompt:agents/review-verdict-doctrine`,
`prompt:agents/leftover-doctrine`, `schema:agents/blocker`,
`schema:agents/review-verdict`, `schema:agents/leftover`) without
auto-activating any behavior. This addressability has real limits today: a
dependency-qualified agent ref cannot be assigned as a sequence step's agent
(agent refs must be local and unqualified), and a dependency-qualified
prompt ref used as a step's prompt is left pending by prompt-contract
validation rather than inlined — there is no operation that splices a
dependency prompt's body into another prompt's text. See
`.ctx/traits/agents/resources/dependency-ref-index.md` for the exact scope
per ref kind. To run the roles and doctrine as part of a dependent's own
sequence, use consumption mode 1 (the TypeScript authoring import above) and
compose them into the dependent's own local prompt/agent declarations.

## Package contents

- `src/index.ts` — the single authored source: `workerRole`, `scribeRole`,
  `reviewerRole`, `clerkRole`, `SCOPE_SPLIT_DOCTRINE`,
  `REVIEW_VERDICT_DOCTRINE`, `LEFTOVER_DOCTRINE`, `VARIANT_DOCTRINE`,
  `blockerSchema`, `ownerItemSchema`, `leftoverSchema`, `reviewVerdictSchema`,
  `deviationReportSchema`, `planAmendmentSchema`. Pure draft builders only: no
  filesystem access, commands, provider calls, synthesis, or output writes.
- `dist/` — compiled TypeScript exports (`tsc` output). The workspace's own
  `exports` field resolves consumption mode 1 to `./src/index.ts` directly
  (so in-repo edits surface immediately, never masked by stale `dist/`
  bytes); `publishConfig.exports` points published consumers at `dist/`.
- `.ctx/traits/agents/` — the canonical `agents` trait package (manifest,
  CDK source, generated TOML/map, lock, `body.md`, `scenarios.md`,
  `resources/`) for consumption mode 2, exposing `agent`, `prompt`, and
  `schema` symbols built from the same shared role/doctrine source as
  consumption mode 1. `body.md` is the dependency-facing overview,
  `scenarios.md` walks illustrative consumption scenarios, and
  `resources/dependency-ref-index.md` is a typed-ref lookup table — none of
  the three restate `REVIEW_VERDICT_DOCTRINE`'s text, which lives once in
  `src/index.ts`.

## Build

```sh
pnpm --dir packages/agents run build        # tsc -> dist/
pnpm --dir packages/agents run typecheck
pnpm --dir packages/agents run lint
pnpm --dir packages/agents run format:check
ctx traits build packages/agents/.ctx/traits/agents/source/index.ts
```
