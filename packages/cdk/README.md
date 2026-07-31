# @ctx-traits/cdk

Pure TypeScript builders for authoring `ctx.traits` draft JSON. The package
does not validate canonical traits, read files, run commands, call providers, or
write output. It builds deterministic draft JSON and explicit synth delegation
plans for Rust/WASM/CLI to execute.

## Install

`@ctx-traits/cdk@0.1.0-alpha.0` targets a restricted (private) npm release; until
that publication lands, build from this repo. Once published, installing requires
an authenticated, authorized npm account:

```sh
pnpm add -D @ctx-traits/cdk
```

This alpha targets `ctx` CLI `0.1.0`. The CDK emits draft JSON only; `ctx traits build`
delegates validation and canonical synthesis to the Rust/WASM core, so the published
CDK version and the CLI version must be paired as above.

Source: https://github.com/ctx-company/ctx-traits (`packages/cdk`).

```ts
import {
  agent,
  method,
  port,
  procedure,
  prompt,
  sequence,
  slot,
  toDraftJson,
  tone,
  trait,
  verbosity,
} from "@ctx-traits/cdk";

const codeDiff = port.input.text({ id: "code-diff" });
const changeSummary = slot.text("change-summary");
const riskNotes = slot.text("risk-notes");
const reviewComment = slot.text("review-comment");
const reviewOutput = port.output.text({ id: "review-comment", value: reviewComment });
const worker = agent("worker", { description: "Writes the draft review." });
const reviewer = agent("reviewer", { description: "Checks the draft review." });

const prRiskTriage = trait({
  id: "pr-risk-triage",
  name: "PR Risk Triage",
  description: "Turns a code diff into one concise PR review comment.",
  behavior: {
    tone: [tone.direct, tone.technical],
    method: method.evidenceFirst,
    verbosity: verbosity.brief,
  },
  agents: [worker, reviewer],
  procedure: procedure({
    description: "Review a PR diff through summary, risk notes, and final comment.",
    input: codeDiff,
    output: reviewOutput,
    sequence: [
      sequence.prompt({
        id: "summarize-code-diff",
        agent: worker,
        input: codeDiff,
        prompt: prompt.text`Summarize what changed in ${codeDiff}.`,
        output: changeSummary,
      }),
      sequence.prompt({
        id: "find-risks-in-code-diff",
        agent: reviewer,
        input: [codeDiff, changeSummary],
        prompt: prompt.text`Using ${codeDiff} and ${changeSummary}, list concrete risks.`,
        output: riskNotes,
      }),
      sequence.prompt({
        id: "write-pr-comment",
        agent: worker,
        input: [changeSummary, riskNotes],
        prompt: prompt.text`Write one concise PR review comment using ${changeSummary} and ${riskNotes}.`,
        output: reviewComment,
      }),
    ],
  }),
});

console.log(JSON.stringify(toDraftJson(prRiskTriage), null, 2));
```

Useful helpers:

- `ref.port("code-diff")`, `ref.slot("risk-notes")`, `ref.prompt("summarize")`, `ref.schema("text")`, and `ref.agent("worker")` create typed refs.
- `agent(...)` declares abstract multi-harness roles; sequence `agent` fields emit `agent:<id>` refs. `agent.worker(...)`, `agent.reviewer(...)`, `agent.planner(...)`, `agent.oracle(...)`, and `agent.searcher(...)` declare roles from the built-in templates; the bare top-level `worker`/`reviewer`/`planner`/`oracle`/`searcher` exports are deprecated aliases kept for the migration window.
- `session(id, opts?)` declares a named, shareable session identity: pass the same handle to more than one agent's `session` field (`agent("a", { session: shared })`, `agent("b", { session: shared })`) and both bindings lower to the identical `session:<id>` ref. `session.PerFrame`/`session.Persistent` are the two no-sharing lifecycle constants — always agent-local, never implying sharing even when the same constant is reused across agents. Only a named `session(...)` handle creates a shared declaration; a bare lifecycle value never does. Assignment-dependent sharing compatibility (same harness, compatible transport, serialized concurrent access) is runtime scope, not a CDK/authoring-time guarantee.
- `schema.text()`, `schema.boolean()`, `schema.number()`, `schema.any()`, `schema.list(...)`, `schema.enum(...)`, and `schema.object(...)` cover built-in, enum, and object schema refs.
- `schema.zod(id, source, { toJsonSchema })` and `schema.typebox(id, jsonSchema)` map supported JSON Schema into inline canonical fields. They support scalar properties, scalar arrays, scalar enums, `description`, and required fields; `title`, `examples`, and `$schema` are unsupported keywords and fail the build like any other, so a real adapter callback strips them before calling `schema.zod`/`schema.typebox`.
- `schema.extend(...schemas, additions?)` combines several object-schema field-record maps (or a plain fields map), throwing a field-specific collision error naming both sources instead of a plain `{ ...a, ...b }` spread's silent last-wins. `schema.optional(fieldValue)` marks an existing field/field record `required: false`. `schema.template(templateId, defaultSpots, build)` is a monomorphizing factory over `schema.object`: declare named default spots once, then call the returned specialization function with a fresh schema id and (optionally) per-spot overrides.
- `slot.text(...)`, `slot.boolean(...)`, `slot.number(...)`, `slot.any(...)`, and `slot.of(...)` declare procedure slots.
- `port.input.text(...)`, `port.output.text(...)`, `port.input.of(...)`, `port.output.of(...)`, and `port(...)` declare trait boundary ports.
- `prompt.text\`...${typedRef}...\`` preserves typed interpolation refs for synth diagnostics.
- `prompt.template("Use {request} and {context}.", { request, context })` rewrites named placeholders to canonical typed refs and infers prompt inputs. `prompt.text` remains the positional tagged-template form.
- `resource.file("guide", { path: "resources/guide.md" })` declares a file-backed resource; `resource.inline("checklist", "## Checklist\n")` declares canonical inline content. `checklist(...)` and `rubric(...)` produce deterministic inline Markdown.
- `sequence.prompt(...)` and `sequence.command(...)` lower to canonical procedure sequence items.
- `input.optional(slotHandle)` marks a sequence-item slot input as optional: it never blocks production, recovery routing, or dry-plan readiness, is omitted from the frame entirely while no accepted value exists, and appears through the normal available-input path once one does. Write it alongside inferred prompt refs, e.g. `input: input.optional(priorVerdict)`. It cannot satisfy required prompt interpolation, an explicit prompt contract, or command argv interpolation, since those need a value unconditionally.
- `toDraftJson(...)` emits stable draft JSON; `synth(...)` returns a delegation plan and does not spawn a subprocess.
- `ctx traits build .ctx/traits/<id>/source/index.ts` executes the module at the CLI/IO boundary, then calls pure synth. Commit the CDK source, generated tree (`generated/index.toml` and `generated/index.map`), and `trait.lock` evidence under `.ctx/traits/<id>/`; its canonical runtime output is `generated/index.toml`. `ctx traits check` fails on proven stale CDK output, warns when the local runtime is unavailable, and supports `--skip-cdk-drift` for hermetic checks.

The CDK boundary is unchanged: builders emit draft JSON, `ctx traits build` validates and builds canonical TOML, and consumers read canonical artifacts without executing TypeScript.

## Package layout

A variant with more than one source module follows one authoring pattern.
`.ctx/traits/refactor/source/variants/direct/` in this repo is the worked example:

```
variants/direct/
  index.ts               # declares only: imports + one variant()/procedure() call
  data.ts                # domain nouns — ports, slots, shared values
  schema.ts               # shapes referenced from data.ts / sequence/*
  agent.ts                # roles, if the trait declares any
  sequence/
    annotation.ts          # one file per step, named for the domain concept
    checklist.ts
    implementation.ts
```

The rules:

- **`index.ts` declares, it does not define.** It imports from the other
  modules and makes exactly one `trait(...)`/`procedure(...)` call. Bodies —
  prompt text, checklists, long descriptions — live in `data.ts` or a
  step-named `sequence/*.ts` module, not inline in `index.ts`.
- **Domain nouns in `data.ts`; shapes in `schema.ts`; roles in `agent.ts`.**
  Each file earns its name by what it holds, not by convention alone.
- **One `sequence/<concept>.ts` per step, named for the domain concept it
  performs** (`annotation.ts`, not `step1.ts`).
- **Variants carry only their differences.** A family of variants (a `-quick`,
  a `-strict`, …) shares its common modules and each variant's own source
  differs only in what it actually changes.
- **Pragmatism is a first-class clause, not an afterthought.** Split by
  entity, domain, or intent; share by primitive. Never oversplit: a module
  earns its filename with a domain word, or it doesn't exist. No helper
  layers, no factory patterns, no generic `utils.ts`. A small trait that fits
  in one file should stay one file — fewer, well-named files beat many tidy
  ones.

This is documented judgment, not a style a linter can fully enforce. `ctx
traits build` does check the mechanical parts of it — the parts that are
unambiguous from source text alone — and emits WARN-severity authoring
diagnostics (never a build failure) when they're violated:

| Code | Fires when | Escape |
| --- | --- | --- |
| `cdk-index-defines` | The package has ≥2 source modules and `source/index.ts` contains a string/template literal past the prompt-body threshold. | Single-file packages are exempt by construction — a trait that fits in one file is the never-oversplit rule working as intended. |
| `cdk-generic-module-name` | A source module is imported by exactly one other module and its basename is a generic, non-domain word (`utils`, `util`, `helpers`, `helper`, `common`, `misc`, `lib`, `shared`). | Rename it to a domain word — the escape is the fix; import count no longer matters once the name carries meaning. |
| `cdk-inline-prompt-body` | Any source module other than `data.ts` (and other than a single-file package) contains a string/template literal past the prompt-body threshold. | Move the body to `data.ts`, or keep it step-local under the threshold — bodies below the threshold never warn. |

All three are advisory text-scanning checks (no TypeScript parser), biased to
silence: anything the scan cannot decide confidently does not warn. They
carry no suppression-comment mechanism — each rule's escape is structural
(single-file, rename, or the threshold itself), so there is nothing
legitimate to suppress that the escape doesn't already cover.

`ctx traits new` scaffolds new packages in this layout directly; a template
small enough to justify one file stays one file, per the same pragmatism
clause.

Authoring notes:

- Auto-collected declarations preserve authoring order. Reordering declarations in `source/index.ts` can change canonical TOML order and digests, matching the old manual-array behavior.
- `schema.object(...)` field entries default to `required: true` and serialize that explicitly. Raw TOML still defaults omitted `required` to `false`, so generated canonical files stay unambiguous.

Command-step shorthand such as `sequence.command({ id: "run-npm-lint", cmd: "npm run lint", output })` lowers to explicit argv in the draft and rejects shell-only syntax. Actual process execution belongs to the controlled CLI/IO runtime and requires explicit command permission.

Schema adapters are pure author-time transforms. They never import Zod at runtime, write resources, or infer TypeScript types. The supported subset maps to inline fields because canonical resources are package-relative paths owned by the CLI/IO boundary. Richer JSON Schema must use `ctxSchemaResource` with an already-materialized resource path. Nested objects, object arrays, composition, refs, defaults, coercions, and semantic constraints are rejected rather than silently widened. This inline-fields boundary diverges from the product's resource-backed adapter wording and needs product sign-off before resource materialization is authorized.

Runtime wrappers belong in a separate runtime SDK package that calls `ctx-traits-wasm-core`, `ctx-traits-wasm-plugin`, or `ctx traits ... --json`. They must preserve unsupported-capability reports for host behavior. See `../SDK_BOUNDARIES.md`.
