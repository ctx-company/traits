# agents

Dependency-facing overview of the canonical `agents` trait package (consumption
mode 2 — see `packages/agents/README.md` for the full picture including
consumption mode 1, the TypeScript authoring import).

## What this package declares

Four generic agent roles built from the same `workerRole`/`reviewerRole`/
`scribeRole`/`clerkRole` builders that `@ctx-traits/agents`'s TypeScript face
exports:

- `agent:agents/worker` — implements a draft or agreed design, applies
  reviewer fixes.
- `agent:agents/reviewer` — drafts and/or reviews work in a bounded
  refinement loop.
- `agent:agents/scribe` — writes the commit message for a completed run.
- `agent:agents/clerk` — extracts and distills context for later steps.

One prompt, derived from the shared `REVIEW_VERDICT_DOCTRINE` constant via
`prompt.template`:

- `prompt:agents/review-verdict-doctrine` — the blocker-reporting and
  escalation doctrine, packaged as a standalone canonical prompt. Binds
  `{slot:phase-brief}`/`{slot:product-brief}` locally so the declaration is
  self-contained. As a dependency-qualified ref it is addressable but its
  prompt contract is left pending by validation (unchecked without loaded
  dependency contents) — see `resources/dependency-ref-index.md` for what is
  and isn't currently wired through a dependency.

Two schemas:

- `schema:agents/blocker` — one blocking defect: id, location, root cause,
  required fix, and a falsifiable done-when.
- `schema:agents/review-verdict` — the typed verdict a reviewer step
  returns: status, blockers, advisory notes, escalation.

## Why a dependency instead of a copy

A trait that pastes this doctrine into its own prompt text re-diverges the
moment either copy is edited — the whole point of extracting it once was to
make that divergence a reviewable CDK-drift diff instead of a silent split.
The doctrine and role text are shared at TypeScript-authoring time (import
`REVIEW_VERDICT_DOCTRINE`/`workerRole`/etc. from `@ctx-traits/agents` and
compose them into the consumer's own local prompt and agent declarations —
see `packages/agents/README.md` consumption mode 1). Declaring `agents` as a
canonical `[dependencies]` entry (consumption mode 2) additionally makes
`agent:agents/*`, `prompt:agents/review-verdict-doctrine`, and the two
schemas addressable through typed refs without auto-activating any
behavior — but a dependency-qualified agent ref cannot be assigned as a
sequence step's agent (agent refs must be local and unqualified), and a
dependency-qualified prompt ref used as a step's prompt is left pending by
prompt-contract validation rather than inlined. See
`resources/dependency-ref-index.md` for the exact scope of what each ref
kind supports today.

## Scope

This is a leaf schema/doctrine trait: its own `[dependencies]` table is
empty. It declares roles, a prompt, and schemas — no ports, slots (beyond the
two local ones the doctrine prompt binds to), steps, or sequences of its
own.
