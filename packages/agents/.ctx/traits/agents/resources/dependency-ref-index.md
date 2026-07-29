# Dependency ref index

The typed refs a dependent trait can address once it declares `agents` in
its `[dependencies]` table (see `body.md` for what each one is, `scenarios.md`
for consumption scenarios). This index is for lookup only — it does not
restate the doctrine text itself, which lives once in
`packages/agents/src/index.ts`'s `REVIEW_VERDICT_DOCTRINE` and nowhere else.

| Kind   | Ref                                    | Local alias form (as declared) | Dependency-qualified use today |
|--------|-----------------------------------------|---------------------------------|---------------------------------|
| agent  | `agent:agents/worker`                   | `worker`                        | Addressable, but cannot be assigned as a sequence step's agent — agent refs must be local and unqualified (`validate_agent_ref`). |
| agent  | `agent:agents/reviewer`                 | `reviewer`                      | Same limitation as above. |
| agent  | `agent:agents/scribe`                   | `scribe`                        | Same limitation as above. |
| agent  | `agent:agents/clerk`                    | `clerk`                         | Same limitation as above. |
| prompt | `prompt:agents/review-verdict-doctrine` | `review-verdict-doctrine`       | Addressable as a step's prompt ref, but its prompt contract is left pending by validation (unchecked without loaded dependency contents) — there is no splice/inline operation that pulls its body into another prompt's text. |
| schema | `schema:agents/blocker`                 | `blocker`                       | Usable as a declared output/field schema reference. |
| schema | `schema:agents/review-verdict`          | `review-verdict`                | Usable as a declared output/field schema reference. |

Replace `agents` with whatever local alias the dependent's own
`[dependencies]` entry uses if it differs from the package id.

To actually run the roles and doctrine as part of a dependent's own
sequence, compose the shared source at TypeScript-authoring time instead
(import `workerRole`/`reviewerRole`/`scribeRole`/`clerkRole`/
`REVIEW_VERDICT_DOCTRINE` from `@ctx-traits/agents` and declare local
agents/prompts built from them — see `packages/agents/README.md`
consumption mode 1). The canonical `[dependencies]` entry documented here
is for addressing these symbols through typed refs (e.g. as an output
schema), not for running them directly.
