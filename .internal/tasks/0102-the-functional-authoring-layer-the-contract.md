# 0102 — the functional authoring layer: the contract

**Status:** filed, contract (0097-style: the decisions here were settled deliberately across the 2026-08-04/05 owner design sessions — do not re-open them without a concrete contradiction, recorded here as an amendment) · **Depends on:** — · **Raised:** 2026-08-05 · **Touches:** nothing directly; 0103–0109 implement it and cite it

A registration-style TS authoring layer over the existing CDK. Traits read top-to-bottom in the
order they execute; the plumbing — parameter threading, spread arrays, hand-maintained manifest
lists — disappears; the typed dataflow that makes a run provable stays exactly where it is: in
slots, ports, and their bindings.

## The pipeline

Functional TS → object-based TS (today's CDK) → TOML canonical. Both TS layers are public
authoring surfaces. The functional layer compiles 1:1 into the object layer and can never
express what the object layer cannot — the object layer stays the semantic authority, and every
canonical change lands there first.

## The law

Only the relative position of steps and `flow.*` statements is semantic. Everything else —
`loop.*` configuration, `effect.*` hooks, `define*`/`use*` calls, declarations, formatting —
normalizes away: two sources that differ only in those emit byte-identical TOML.

## The taxonomy

Every namespace names a role:

- `define*` / `use*` — context and composition; position-free.
- `loop.*` — configuration on the loop callback param; position-free.
- `flow.*` — control flow and its constants; positional, always: where a `flow.*` statement
  sits is when it happens.
- `effect.*` — reactive effects; position-free. Signal hooks (`effect.on*`) attach to the
  nearest enclosing scope; session host sinks (`effect.session.*`, 0110) are session-global by
  name.
- `step.*` — agentless steps (`step.command`, `step.check`). Settled here: the `sequence`
  namespace dissolves into `step.*` + `flow.*`.

## Decisions

- **One shape for every trait.** `export default function (ctx)` — behavioral and procedural
  alike; a behavioral trait simply registers no steps. `defineTrait(slug, {…})` inside, exactly
  once, plain literal data (name, version, summary, metadata, procedure description) — missing,
  duplicate, or computed identity is a build error. `ctx` carries run-side inputs
  (`ctx.input.*`); the return statement maps output ports.
- **Prompts return nothing; slots are the wiring.** Slots, ports, signals, resources, agents are
  module-level declarations; steps bind them via `input:` / `output:` / `include:` (each taking
  single, array, or template). Registration order is sequence order.
- **Templates are side-named.** `input.prompt` / `input.command` / `output.prompt` tagged
  templates (`prompt.text` dies, 0103). An output template's interpolated slots ARE the step's
  output contract (0105). `${slot.optional()}` works on both sides — optional input (the
  cumulative-ledger self-read) and may-be-left-unfilled output. `include:` lists non-inlined
  references on any step kind and composes with `.optional()`.
- **Constructors only for canonical identity.** `resource()`, `slot()`, `signal()`, `port()`,
  `defineTrait()` mint identity and become canonical nodes. Behavior/intent fragments are plain
  typed objects (`export const behavior: Behavior = {…}`) or inlined — fragment constructors
  (`defineBehavior` et al.) rejected.
- **Composition is plain JS; the builder validates outcomes.** Fragments compose by object
  spread inside `useBehavior` / `useIntent` / `useResource` — no builder merge algebra
  (`useTrait` and variadic merge both rejected as silent or ambiguous). The `use*` gate checks:
  final-object shape, require/avoid contradictions, undefined or unknown list entries (this is
  what catches enum typos — the pipeline evaluates, never type-checks), and overlapping keys
  across same-scope `use*` calls (order across calls never matters; order inside one literal is
  explicit syntax and the author's). Union can add doctrine, never silently remove it;
  subtraction is unsupported until a real consumer records the need here.
- **The manifest is derived.** `resource:` / `signal:` / `port:` lists are collected from use —
  `include:`, interpolations, `ctx.input.*`, the return statement, effect/on-* attachments. A
  declared-but-never-referenced handle is a build error. Provenance is package-level: the import
  graph plus the lockfile, exactly like any package manager.
- **Positional `until` and `when`.** `flow.until(cond)` and `flow.when(title, cond, flow.Abort)`
  inside a loop lower to: steps registered after them wrapped in `when(not cond)`, the condition
  becoming the loop's ordinary end-of-round guard. Top of body = while, end = do-while, mid =
  early-exit checkpoint. No new runtime kind. Exactly one `flow.until` per loop; a loop whose
  callback never called `loop.maxIterations` is a build error — no-way-out is not authorable.
- **`match`, not `branch`.** `flow.match(title, subject, arms)` — subject is a guard
  (`[flow.True]` / `[flow.False]` arms) or a slot field (value keys plus `[flow.Otherwise]`).
  Arms are ALWAYS callbacks: object values evaluate eagerly, so bare registrar calls in arm
  position would register both branches. Multi-way lowers to nested canonical branches.
- **`when` has two homes, no more.** The `when:` field on any step (single-step guard, no
  nesting) and `flow.when(title, cond, cb)` (block; nesting composes as AND). Guard-handle
  fluent forms rejected: `.then` makes guards thenable (Promise assimilation), and any renamed
  variant is a third spelling.
- **Loops: config, exits, effects.** `loop.maxIterations(n, { onExhausted: flow.Continue |
  flow.Abort })` (Continue is the default, matching the canonical) is the only `loop.*` call.
  Three distinguishable exits: `flow.until` matched (success), an abort rule matched
  (`flow.when(…, flow.Abort)` — subsumes `abortIf`, which does not exist), budget exhausted
  (policy governs the run's consequence). Signal hooks: `effect.onComplete` /
  `effect.onFailure` (exhaustion is one of failures) / `effect.onAbort` — the runtime keeps
  recording the accurate mechanism in the ledger independently, so provability loses nothing.
  Steps have no scope and keep field spellings: `onComplete:` (replacing `emits:`),
  `onFailure:`, `timeoutMs:`.
- **`slot.forEach` and `flow.parallel`.** `items.forEach(title, { limit, maxItems, concurrent,
  onComplete }, (item) => …)` is THE for-each spelling. `flow.parallel(title, (par) => …)` is
  the one block where registration order deliberately means nothing; `par.onFailure(flow.*)`
  maps the existing branch-failure policy; `par.maxAtOnce(n)` is reserved surface (parked
  canonical addition).
- **Build context.** A synchronous stack: a registrar called outside a build throws; an async
  callback is a build error; one build at a time; block callbacks push/pop scope. Composites
  (`commitTail`, `familyProcedure`, guarded production) are plain functions calling registrars —
  no arrays returned, no spread at call sites.
- **`$` field paths — staged.** `verdict.$.status.match(…)`: the `$` hop keeps user schema
  fields and API methods in separate namespaces (a schema field named `id` or `match` must never
  shadow the surface). Until it lands, `condition.fieldEquals` and friends. Condition sugar is
  earned by usage — `condition.notEmpty` qualifies today — never speculative.

## The surface, in one file

```ts
import * as standards from "@ctx-traits/engineering-standards";
import { clerk, smart, worker } from "./agents.ts";

const taskBoard = resource({ id: "task-board", path: ".internal/tasks", root: "repo", trigger: "on-demand", hint: "…" });
const taskBrief = slot.text({ id: "task-brief", description: "Verbatim copy of the task file." });
const draft = slot.text({ id: "draft", description: "The implementation draft." });
const workSummary = slot.text({ id: "work-summary", description: "Worker's cumulative account." });
const verdict = slot({ id: "review-verdict", schema: reviewVerdictSchema });
const statusOut = slot.text({ id: "commit-status", description: "Porcelain status output." });
const commitMessage = slot.text({ id: "commit-message", description: "One-line commit message." });
const receipt = slot.text({ id: "commit-receipt", description: "Hash + subject of the landed commit." });

export default function (ctx) {
  defineTrait("guarded-change", {
    name: "Guarded Change",
    version: "0.1.0",
    summary: "Implement one task, reviewed, committed behind ctx-gate approval.",
    metadata: { tag: ["first-party", "implementation"] },
    procedure: "Extract the task, draft, refine until approved, commit.",
  });

  useBehavior({ ...standards.behavior, verbosity: verbosity.Brief });
  useIntent(standards.intent);
  useResource(standards.doctrine);

  clerk.prompt("Task Request", {
    input: input.prompt`Copy the task file for ${ctx.input.task} verbatim.`,
    include: [taskBoard],
    output: taskBrief,
  });

  smart.prompt("Plan Draft", {
    input: input.prompt`Draft the approach from ${taskBrief}.`,
    output: draft,
  });

  flow.loop("Refinement Loop", (loop) => {
    loop.maxIterations(4);

    worker.prompt("Implement", {
      input: input.prompt`Apply ${draft}, extending your prior summary ${workSummary.optional()}.`,
      output: workSummary,
    });
    smart.prompt("Review", {
      input: input.prompt`Review ${workSummary} against ${taskBrief}.`,
      output: verdict,
    });

    flow.until(condition.fieldEquals(verdict, "status", "approved"));
  });

  step.command("Status", { input: input.command`git status --porcelain`, output: statusOut });

  flow.when("Ship only if the tree is dirty", condition.notEmpty(statusOut), () => {
    clerk.prompt("Commit Message", {
      input: input.prompt`From ${workSummary}, write the one-line message.`,
      output: commitMessage,
    });
    step.command("Stage", { input: input.command`git add -A -- :!.agents/runs` });
    step.command("Commit the work (awaiting ctx-gate approval)", {
      input: input.command`ctx-gate run -- git commit -m ${commitMessage}`,
      output: receipt,
      timeoutMs: 14_400_000,
    });
  });

  return { commitReport: receipt };
}
```

And the behavioral package it composes — same function shape, fragments outside, no steps:

```ts
export const behavior: Behavior = {
  tone: [tone.Direct, tone.Technical],
  method: method.EvidenceFirst,
};
export const intent: Intent = {
  require: [intent.require.ReviewBeforeFinal, intent.require.BoundedRefinement],
  avoid: [intent.avoid.UnboundedLoop, intent.avoid.RubberStampReview],
};
export const doctrine = resource({ id: "standards-doctrine", path: "doctrine.md", hint: "…" });

export default function () {
  defineTrait("engineering-standards", {
    name: "Engineering Standards",
    version: "0.3.0",
    summary: "House doctrine: evidence-first, reviewed, bounded.",
  });
  useBehavior(behavior);
  useIntent(intent);
  useResource(doctrine);
}
```

## The ledger

- **Renames (0103, end to end):** `stop-if` → `abort-if` · `on-stop` → `on-abort` · `emits` →
  `on-complete` · exhaustion policy `block` → `abort` · `prompt.text` → `input.prompt` ·
  `flow.Block` → `flow.Abort`.
- **Canonical/runtime additions:** normalized emission (0104) · optional outputs (0105) ·
  session host sinks — `effect.session.title` first (0110) · `parallel.maxAtOnce` (parked) ·
  scoped `use*` inside `flow.*` blocks (parked).
- **Pure sugar, no canonical change:** positional lowering, `flow.match`, `slot.forEach`, the
  `when:` field, `$` field paths, condition sugar, derived manifests, spread composition.

## Watch

- The object layer must stay the single emission path through 0106–0107 — 0108's byte-identical
  pilot is only proof if the functional layer cannot reach around it.
- Per the standing validation ruling (2026-07-24), no behavior-freezing tests while these
  surfaces churn: the gate stays drift/embed + builds + cargo test.

## Amendments (0108 pilot findings)

`implement:quick` ported functionally at
`.ctx/traits/packages/implement/source/variants/quick-functional.ts` (0106 `variant({...})`
shape, `procedure.from`). `diff generated/quick/index.toml <scratch>/quick/index.toml` is
**not** empty; every surviving delta traces to one of the two gaps below — nothing else
differs (loop bounds, `until`/`abort-if` guards, produce/review input ordering, the
shipping-maybe-commit branch, and every prompt body all matched byte-for-byte). Neither gap
was worked around; the port omits what it cannot express and cites this amendment.

- **F2 — no explicit step-id override.** `step.command`/`step.check`/`agent.prompt` always mint
  `id = mintId(title)` (`registrars.ts:107,118,133`; `agent.ts:80`); there is no `id:` field on
  any registrar's options type. The baseline carries seven steps whose id was hand-chosen
  distinct from `kebab(title)`: `draft-writing`/"Draft the work", `repo-gates`/"Run the
  repository gate chain", `capture-diff`/"Capture the changed-file inventory",
  `shipping-status`/"Check working tree status", `shipping-message`/"Write the commit message
  (scribe)", `shipping-stage`/"Stage all changes except runtime state",
  `shipping-commit`/"Commit the work" — all invented inline by `commitTail`/quick's own
  `sequence.prompt`/`sequence.check`/`sequence.command` calls in
  `packages/agents/src/process.ts` and `source/variants/quick.ts`. The pilot reproduces these
  steps' content exactly (title, prompt/argv, inputs, outputs) but their ids differ
  (`draft-the-work`, `run-the-repository-gate-chain`, `capture-the-changed-file-inventory`,
  `check-working-tree-status`, `write-the-commit-message-scribe`,
  `stage-all-changes-except-runtime-state`, `commit-the-work`), which also renames the
  corresponding `[prompt.*]` tables. 0109 needs a resolution — either an authored `id:` escape
  on the registrars, or a sanctioned baseline id-reset — before any variant with a hand-chosen
  id can migrate.
- **F3 — no `step.project` registrar.** `step` exposes only `command`/`check`
  (`registrars.ts:104-124`); there is no functional way to author a `kind = "project"` step, and
  no escape hatch exists to lift a pre-built object-layer `SequenceHandle` (e.g.
  `deriveParkReportStep`'s output) into a `procedure.from` scope — `registerItem`/`currentScope`
  are internal to `registrars.ts` and not exported from `@ctx-traits/cdk`. The baseline's
  `park-report-clear` → `park-report-record` (branch) → `park-report-append` triad
  (`family.ts:553-575`) is entirely unreachable functionally and is omitted from the pilot;
  as a direct consequence `slot:park-report` is never written by the ported variant, so its
  auto-inferred `procedure.output` also drops `port:park-report` (present in the baseline,
  absent in the port). Any variant using `deriveParkReportStep`, or any composite that needs a
  deterministic project step, cannot migrate until `step.project` (or an object-handle lift
  escape) exists.
- **F1 (confirmed, no diff impact)** — a full-file `defineTrait` entry still cannot carry family
  identity (`DefineTraitFields` has no `variant`; `variant.import` rejects a function default
  export), so 0108's port necessarily used the 0106 `variant({...})` shape, never a 0107
  full-file trait function. 0109 needs an answer for this before any variant can migrate as a
  whole file.
- **F4 (confirmed clean)** — `combineAbortIfArms` with a single `flow.when(..., flow.Abort)` arm
  emits the bare condition, matching the object layer's `abortIf:` byte-for-byte; no arm title
  leaks into emission. No amendment needed.
- **R3 tooling gap (process note, not a CDK gap)** — `ctx traits build`'s `--out` flag is
  rejected for any native-family source ("`--out` is not supported for native trait family
  sources"), and a family is recognized only at a package's own `source/index.ts` — an
  arbitrary `.ts` path importing `variant.import(...)` is refused ("declares a native trait
  family but is not under a recognized package's source/index.{ts,mjs} root"). There is
  currently no supported way to build a single-variant family to a scratch location without
  standing up a full scratch package (own `package.toml` + copied `source/` tree, `id` aligned
  to the target family, built once, diffed, then deleted — never committed). This pilot's
  scratch artifact was produced that way; no `functional-pilot/` shell file is checked in,
  since none of the explored forms worked without it becoming, in effect, a second package.

## Done when

0103–0109 cite this file instead of re-deciding; any expressiveness gap found downstream is
recorded here as an amendment, not worked around in an implementation task.
