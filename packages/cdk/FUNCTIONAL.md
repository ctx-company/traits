# The functional authoring layer (0106/0107)

A registration-style DSL over the object-layer CDK: `step.*`/`agent.prompt`/`flow.*`
register steps in the order they run, and `procedure.from(fields, body)` compiles the
registered order into a `ProcedureHandle` — the same handle `procedure({...})` returns,
usable in a `variant({ procedure })` shell unchanged. 0107 adds the full-file trait
entry — `export default function (ctx) { ... }` with `defineTrait`/`use*`/`ctx.input`
inside — that needs no object-layer `trait({...})` shell at all.

Everything below is settled by
[0102 — the functional authoring layer: the contract](../../.internal/tasks/0102-the-functional-authoring-layer-the-contract.md);
this file only documents what 0106/0107 actually ship.

## The one rule

**Only registration order is semantic.** `procedure.from`'s `body` runs synchronously;
each `step.*`/`agent.prompt`/`flow.*` call registers one item, in call order, into the
procedure's sequence — exactly the order those steps run. Everything else (`loop.*`
configuration, `effect.*` hooks, which local variable a slot is assigned to) is
authoring convenience with no effect on the emitted canonical.

The functional layer never builds canonical JSON directly — every registrar lowers
through an existing object-layer constructor (`sequence.prompt`, `sequence.command`,
`sequence.loop`, `sequence.branch`, `sequence.parallel`, `sequence.forEach`). Two
procedures that differ only in whether they were authored functionally or with
`sequence.*` directly emit byte-identical draft JSON — this is what
`test/functional.hand.ts` proves.

## `procedure.from`

```ts
import { agent, condition, input, procedure, schema, slot, step, flow } from "@ctx-traits/cdk";

const worker = agent.worker("worker");
const smart = agent("smart", { description: "Plans and reviews." });

const draft = slot.text("draft");
const workSummary = slot.text("work-summary");
const verdict = slot({
  id: "review-verdict",
  schema: schema.object("review-verdict", { status: schema.enum(["approved", "needs-work"]) }),
});
const statusOut = slot.text("commit-status");
const commitMessage = slot.text("commit-message");
const receipt = slot.text("commit-receipt");

const guardedChangeProcedure = procedure.from(
  { description: "Implement one task, reviewed, committed behind ctx-gate approval." },
  () => {
    smart.prompt("Plan Draft", { input: input.prompt`Draft the approach.`, output: draft });

    flow.loop("Refinement Loop", (loop) => {
      loop.maxIterations(4);

      worker.prompt("Implement", {
        input: input.prompt`Apply ${draft}, extending your prior summary.`,
        include: workSummary.optional(),
        output: workSummary,
      });
      smart.prompt("Review", {
        input: input.prompt`Review ${workSummary}.`,
        output: verdict,
      });

      flow.until(condition.fieldEquals(verdict, "status", "approved"));
    });

    step.command("Status", { input: input.command`git status --porcelain`, output: statusOut });

    flow.when("Ship only if the tree is dirty", condition.not(condition.empty(statusOut)), () => {
      smart.prompt("Commit Message", { input: input.prompt`From ${workSummary}, write one line.`, output: commitMessage });
      step.command("Stage", { input: input.command`git add -A -- :!.agents/runs` });
      step.command("Commit (awaiting ctx-gate approval)", {
        input: input.command`ctx-gate run -- git commit -m ${commitMessage}`,
        output: receipt,
        timeoutMs: 14_400_000,
      });
    });
  },
);
```

`procedure.from` is attached to the same `procedure` export the object layer already
has — `procedure(...)` (object form) and `procedure.from(...)` (functional form) are
one symbol, two call shapes. The resulting handle drops into `variant({ procedure })`
exactly like a hand-authored one:

```ts
import { variant } from "@ctx-traits/cdk";

export default variant({ name: "Guarded Change", summary: "...", procedure: guardedChangeProcedure });
```

## The registrars

- **`agent.prompt(title, opts)`** — attached to every agent handle (`agent(...)`,
  `agent.worker(...)`/other templates, and the deprecated bare templates). Lowers to
  `sequence.prompt`; `opts` is every `sequence.prompt` field except `id`/`kind`/`agent`/
  `title`, which the registrar mints.
- **`step.command(title, opts)` / `step.check(title, opts)`** — lower to
  `sequence.command`/`sequence.check`. `step.check`'s return also carries `.pass`, same
  as `sequence.check`.
- **`items.forEach(title, opts, (item) => { ... })`** — attached to every declared slot
  handle (`slot.texts(...)`, etc.). THE for-each spelling (0102) — lowers to
  `sequence.forEach`, minting the per-item slot the same way `sequence.forEach`'s
  closure form does.
- **`flow.loop(title, (loop) => { ... })`** — opens a loop scope; `loop.maxIterations(n,
  { onExhausted })` is the only method on the callback param, required, callable once.
  A loop with no way out is not authorable — omitting it is a build error.
- **`flow.until(cond)`** — valid only directly inside a `flow.loop` body. Exactly one per
  loop. Positional: everything registered in the same loop scope *after* `flow.until` —
  a leaf step or a whole nested container — is guarded `not(cond)` automatically —
  `cond` itself becomes the loop's ordinary `until` guard. Where you call it decides the
  shape: at the top of the body, every step is guarded (`while`); at the end, nothing
  trails it (`do-while`); in the middle, only the trailing steps are (an early-exit
  checkpoint) — same emission every time, no new runtime kind. The guard is composed
  differently per container kind, since only some object-layer constructors carry a
  `when` of their own: a trailing `flow.loop`, `items.forEach`, or `step.command`/
  `step.check`/`agent.prompt` leaf gets `when: not(cond)` directly; a trailing
  `flow.when(title, cond2, () => {...})` block ANDs `not(cond)` into its `check`
  (`condition.all([cond2, not(cond)])`) since a `branch` has no `when` of its own. A
  trailing `flow.match` or `flow.parallel` cannot carry the guard without changing arm
  routing, so registering either directly after `flow.until` in the same loop scope is a
  build error naming the block's title instead of a silently unguarded emission.
- **`flow.when(title, cond, flow.Abort)`** — inside a loop, registers an abort arm
  (lowers to the loop's `abortIf`). Multiple abort arms in one loop combine with
  `condition.any`.
- **`flow.when(title, cond, () => { ... })`** — block form: the callback's registered
  steps become a guarded nested sequence (`sequence.branch` with only a `success` arm).
  Nesting two `flow.when` blocks composes as AND for free — the inner block only runs if
  both guards passed.
- **`flow.match(title, subject, arms)`** — arms are always callbacks (0102: object
  values evaluate eagerly, so a bare registrar call in arm position would register both
  branches). A guard subject (`condition.signal(...)`, a comparison, ...) takes
  `{ [flow.True]: () => {...}, [flow.False]: () => {...} }` and lowers to one
  `sequence.branch`. A slot-field subject (a `FieldRef` from `slot.foo` field access, or
  a scalar slot) takes value-keyed arms plus `[flow.Otherwise]`, and lowers to nested
  `sequence.branch` chains, one per value.
- **`flow.parallel(title, (par) => { ... })`** — registration order deliberately means
  nothing: each registrar call inside the callback becomes its own named branch
  (`sequence.linear` wrapping that one step), and `sequence.parallel` sorts declarations
  by generated id at emission (0104). `par.onFailure(policy)` sets the branch-failure
  policy applied to every branch in the block. `par.maxAtOnce(n)` is reserved surface —
  it accepts the call and throws; it must never emit anything (0102 ledger).
- **`effect.onComplete(signal)` / `effect.onAbort(signal)`** — position-free within an
  enclosing `flow.loop`; attach to that loop's own `onComplete`/`onAbort` fields.
  `effect.onFailure(...)` always throws — a loop declares no failure of its own to route
  (0102).

## Build rules

A synchronous stack enforces the following, all as thrown `Error`s naming the offending
step's title and (best-effort) the calling file:

- A registrar called with no active `procedure.from` build throws.
- Opening a second build while one is already open throws — one build at a time.
- A `flow.*` block callback that returns a thenable is a build error: bodies run
  synchronously only.
- Two `flow.until` calls in the same loop scope: build error.
- A loop whose callback never called `loop.maxIterations`: build error.
- `loop.maxIterations` called more than once: build error.
- Two steps in the same scope deriving the same id from their titles (0104's collision
  rule, reused here): build error naming both titles.
- A `flow.match` arm that isn't a callback: build error naming the arm.
- `par.maxAtOnce(...)`: always throws — parked surface, see the 0102 ledger.
- A `flow.match` or `flow.parallel` block registered after `flow.until` in the same loop
  scope: build error naming the block — neither construct's object-layer emission has a
  `when` to carry the positional guard without changing arm routing.

## The full-file trait entry (0107)

`export default function (ctx) { ... }`, evaluated by `evaluateTraitFunction` (the emit
harness calls this on a function default export — nothing an author calls directly).
One shape for both behavioral traits (no steps) and procedural traits (steps registered
via `step.*`/`agent.prompt`/`flow.*`, exactly as inside `procedure.from`): the object
layer shell is no longer required either way.

```ts
import { defineTrait, input, port, slot, step, useBehavior, tone } from "@ctx-traits/cdk";

export default function (ctx) {
  defineTrait("diff-review", {
    name: "Diff Review",
    summary: "Reviews a diff for a stated focus.",
    procedure: "Review a diff for the stated focus and return a summary.",
  });
  port.input.text({ id: "diff" });
  useBehavior({ tone: tone.Direct });
  const review = slot.text("review");
  step.command("Review", { output: review, input: input.command`echo ${ctx.input.diff}` });
  return { review };
}
```

- **`defineTrait(slug, fields)`** — exactly once, plain literal data only (`name`,
  `version`, `summary`, `metadata`, and `procedure`, a text description used only when
  the function registers steps). Missing, duplicate, computed, or non-literal fields are
  build errors. Runtime can only prove the resulting *value* is JSON-safe; the Rust-side
  build (`modules/io/src/cdk_build.rs`) additionally text-scans that the slug argument
  itself is a quoted string literal in the source — the guarantee runtime cannot see.
- **`useBehavior(fields)` / `useIntent(fields)`** — spread composition per 0102: the
  builder validates only the final object's outcome — unknown keys, `undefined` array
  entries (the enum-typo catch), a facet already set by an earlier call in the same
  trait function, and (for `useIntent`) a guidance slug declared in both `require` and
  `avoid`. Union never subtracts.
- **`useResource(handle)`** — adds one or more resource handles to the trait's explicit
  resource declarations and marks them referenced, for a resource consumed only through
  `useResource` (never interpolated into a prompt).
- **`ctx.input.*`** — a declared input port (declared with `port.input.*` inside the
  trait function, before first access) resolves by camelCase→kebab id:
  `ctx.input.diff` reads the declared `port:diff` input port. An access with no matching
  declared input port is a build error listing every declared input port id.
- **The return statement** — `return { commitReport: receipt }` mints one output port
  per key (kebab-cased id) bound to the returned slot; `undefined` (no return) means no
  output ports — the behavioral shape. A non-slot return value is a build error naming
  the key.
- **The derived manifest** — `resource:`/`signal:`/`port:` are collected from actual
  use (the same reachability collection `assembleSingleTraitDraft` already runs for the
  object layer), not hand-written lists. A resource/signal/port/slot declared inside the
  trait function but never referenced anywhere in the assembled trait — not returned, not
  interpolated, not passed to `useResource` — is a build error naming its kind, id, and
  authoring location.

Two independent authorings of the same trait — once as a full-file function, once with
the object-layer `trait({...})` shell directly — emit byte-identical draft JSON; this is
what `test/functional.hand.ts`'s 0107 suite proves, the same claim 0106 makes for
`procedure.from`.

## Out of scope here

The byte-identical pilot on an existing production trait (`quick`) is 0108; the rest of
the family migrates in 0109, including scoped `use*` inside `flow.*` blocks (parked) and
unioning references across family leaves for the never-referenced check (0107 checks
per-entry only). `$` field paths, new condition sugar, and `effect.session.*` are parked
per the 0102 ledger.
