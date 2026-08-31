# Smarter trait selection & behavior: right-sizing review for simple UI tasks

## The idea

UI cuts (the desktop/TUI rendering tasks — the 0265–0269 family) are dispatched through `implement-basic` or `implement-complex` and routinely run for multiple hours. The review loop is doing its job — it still finds real issues — but its depth is miscalibrated to the change: a simple UI tweak gets the same unbounded, smart-seated, (sometimes doubly-)reviewed refinement loop as a cross-module architectural change. The ask: make trait selection and in-run behavior scale with task complexity — dynamic model assignment and dynamic review depth — without giving up the review that catches real issues.

## Researched grounding

- **Complexity routing is a solved pattern with two shapes.** Classifier-based routing decides *before* work starts (a lightweight judgment picks the cheap or expensive path); cascade routing starts cheap and *escalates on miss*. RouteLLM reports ~85% cost reduction at ~95% of top-model quality; FrugalGPT up to 98% via cascades. Classifiers cost one cheap call; cascades cost a wasted cheap round on the queries that needed the frontier model anyway.
  - https://neuraltrust.ai/blog/llm-model-routing
  - https://www.truefoundry.com/blog/llm-routing-cost-quality-aware-model-selection
- **Risk-calibrated review automation is production-proven.** Meta's RADAR calibrates review depth to a risk score for low-risk diffs; the threshold is an explicit dial between risk and automation yield. The transferable idea: review depth keyed on a *typed, evidence-based risk verdict*, not on task origin.
  - https://arxiv.org/pdf/2605.30208
- **Self-review loops hit diminishing returns fast.** Practitioner guidance converges on capping self-review/refinement at 2–3 rounds for small changes; agentic-review frameworks add stagnation detection (insubstantial revision → stop) as a loop exit alongside approval.
  - https://addyosmani.com/blog/agentic-code-review/
  - https://agentpatterns.ai/code-review/agent-self-review-loop/
- **Per-agent model/effort assignment is the industry direction.** Claude Code supports per-subagent model selection today and per-agent effort is the top open ask; harnesses expose "seat → model" as configuration, which is exactly the indirection this repo already has.
  - https://code.claude.com/docs/en/model-config
  - https://github.com/anthropics/claude-code/issues/25669

## Repo grounding: the seams already exist

Almost every primitive this needs is already in the tree; nothing here requires new runtime machinery in Rust for the recommended stage.

| Seam | Where | What it gives |
|---|---|---|
| Variant families | `.ctx/traits/authored/implement/trait.toml` (`[family.variant]` basic/quick/complex; `[variant.quick.budget]`) | Per-variant procedures *and* per-variant budgets already resolve |
| Variant-scoped seats (P451) | `modules/io/src/harness_config.rs:1699` (`[agent.variant.<v>.role.<r>]`), `resolve_run_variant` at `harness_config.rs:3200` | A run's variant (from `metadata.variant` or trait-id suffix) already selects different model seats per role — variant-level "dynamic model assign" is config, not code |
| Model-tier indirection | `AgentModelTier` (`modules/core/src/trait/agent.rs:20`, top/fast) + `model_tier` map in `AgentDefaults` (`harness_config.rs:1686`) | Roles can declare a tier intent instead of a model |
| Triage-gate prior art | `feasibilityGate` in `packages/toolkit/src/sequence/feasibility.ts` | The exact shape needed: a 1-frame prompt step returning a typed verdict inside an `iterations: 1` loop, with block/warn modes — proof the CDK expresses bounded gates today |
| Conditional flow | `cdk.flow.when` + `cdk.condition.fieldEquals`/`signal` (`packages/cdk/src/condition.ts`); already branching on `needsOwnerSignal` in `implement/source/variant/basic/index.ts:19` | Verdict-routed branches inside a variant |
| Review verdict spine | `reviewVerdictSchema` / `blockerSchema` (`packages/toolkit/src/schema.ts`), `cdk.intent.TasteOnlyBlocking` already in the smart agent's avoid list (`implement/source/shared/agent.ts`) | A place to make "nits don't loop" enforceable |
| Dispatch preflight | `modules/io/src/dispatch_preflight.rs` (opt-in `--task-dispatch` binding, task resolved via `TaskProvider`) | The hook point if selection ever moves to dispatch time |

The pain is equally visible in the tree: `implement/source/variant/basic/index.ts:24` and `complex/index.ts:14` both carry the deliberate comment *"No round ceiling: the loop ends when the reviewer approves… the run's own frame/time budgets are the outer stop."* For a simple UI tweak, that outer stop is `total-seconds = 10800` — three hours of sanctioned grinding. Both reviewer seats resolve to smart models, and the review prompt itself says "take as many rounds as it needs."

## Recommended approach: post-draft complexity triage inside `implement-basic`, routing review depth and reviewer seat

A staged plan, cheapest first:

**Stage 0 — bounded `implement-quick` experiment.** Extend `implement-quick` with the capped `reviewer-light` loop described below, then dispatch selected UI cuts through it. Do not assume a model yet: give `reviewer-light` its own seat and choose the model as an explicit experiment variable. Only light-review approval is success; if the final permitted review still returns blocking work, report unresolved cap exhaustion rather than treating it as approval. This tests whether light review keeps finding actionable work and how often it reaches the cap without giving up review entirely.

**Stage 1 (the recommendation) — a `light` review path selected by an in-run triage gate.** Evolve `implement-basic`:

1. After `shared.step.draft.compose`, add a 1-frame **size triage** step, shaped exactly like `feasibilityGate`: the smart drafter classifies the *drafted* change against evidence (files it will touch, novel logic vs. cosmetic adjustment, blast radius beyond the rendering layer) into a typed verdict slot — `tier: light | standard`, with reasons. Post-draft triage is deliberately richer than dispatch-time classification: the draft already names the files and approach, so the classifier judges a plan, not a prose guess.
2. `cdk.flow.when(tier == "standard")` → today's loop, verbatim, no-ceiling ruling untouched.
3. `cdk.flow.when(tier == "light")` → a capped loop (`iterations: 3`, the diminishing-returns consensus): implement → capture diff → **one** review round per iteration by a new trait-declared reviewer role (`reviewer-light`), whose runtime.toml seat is separate but whose model remains an experiment choice. Its doctrine hardens the existing `TasteOnlyBlocking` avoid into the verdict contract: only contract-breaking findings justify `revise`; taste and polish findings are *recorded* in the verdict but do not block. Approval exits immediately. Escalation happens only when the final permitted light review returns `revise` with unresolved blocking work; that exit routes into the standard path. The cap itself is not an escalation trigger, and it never turns an unapproved result into approval.
4. Dynamic model assignment falls out of role indirection: two declared reviewer roles with separately configurable seats is the mechanism. No mid-run re-seating machinery is needed or proposed; seats resolve once per run per role, and branching between roles *is* the dynamic assignment. The `reviewer-light` model is intentionally undecided until the quick experiment supplies evidence.

Everything in Stage 1 is CDK/trait authoring plus runtime.toml seat mapping — the trait-package edit landing recipe (ts-build → `ctx traits build` → export) covers it; no Rust changes.

**Expected effect:** a UI tweak run becomes draft + triage + (implement → light review) × ≤3 + commit tail — roughly 6–8 frames before any necessary escalation — while anything the triage classifies as standard, or the final light review still finds blocking work in, enters exactly today's behavior.

## Alternatives and tradeoffs

**B — Dispatch-time variant selection (classifier or task facet).** Either extend the `[tasks] dispatch-trait` flow so a cheap classifier frame reads the task file and picks the variant, or add an owner-set effort facet to task files that dispatch maps to a variant. Tradeoffs: selection happens before any draft exists, so it judges prose rather than a plan; a facet pushes calibration burden onto task authoring; a classifier frame adds Rust work in `dispatch_preflight.rs` territory. Worth revisiting once Stage 1 data shows how often triage disagrees with what dispatch would have guessed. (Stage 0 is the degenerate manual form of this.)

**C — Cascade (start light, escalate on miss).** Skip triage; every run starts on the light path and escalates when the light reviewer reports contract-breaking blockers or the captured diff exceeds size bounds. Closest to FrugalGPT; saves the triage frame. Tradeoffs: complex tasks pay a wasted light round (and the light reviewer is the *detector* of complexity — the component assigned the deliberately narrower review); worst-case wall clock grows. Stage 1's exhaustion-escalation already gives a bounded version of this safety net without betting the routing on the narrower review.

**D — True mid-run dynamic re-seating.** Let a run swap a role's model between frames based on verdict signals. Requires runtime changes to seat resolution (`flatten_agent_defaults` and session records assume one resolution per run) for marginal benefit over role-branching. Park it; only revisit if role-count proliferation becomes a real smell.

## Concrete touchpoints

- `.ctx/traits/authored/implement/source/shared/agent.ts` — declare `reviewerLight` (reviewer template, separately configurable seat; experiment model TBD).
- `.ctx/traits/authored/implement/source/shared/data.ts` — new `sizeTier` slot (typed verdict: tier + reasons + evidence).
- `.ctx/traits/authored/implement/source/shared/step/review.ts` — `light` review step with the nits-don't-block verdict doctrine.
- `.ctx/traits/authored/implement/source/variant/basic/index.ts` — triage step + `when`-routed light/standard loops with escalation wiring.
- `.ctx/traits/authored/implement/source/variant/quick/index.ts` — the initial capped light-review experiment, where reaching the cap with unresolved blockers is not approval.
- `packages/toolkit/src/sequence/` — optionally extract the triage gate as `sizeGate` beside `feasibility.ts` if a second trait wants it (single-consumer rule says: keep it in the trait until then).
- `.ctx/traits/runtime.toml` (this repo) — `[agent.role.reviewer-light]` seat, with its experiment model still to be selected.
- `trait.toml` — version bump.

## Owner decisions applied

1. **Evolve `basic`.** Do not add a `calibrated` variant.
2. **Escalate only when light review still finds work.** Approval exits; a final `revise` verdict with unresolved blockers enters the uncapped standard loop.
3. **Keep severity in doctrine for now.** A typed `severity` field would be used to mechanically decide whether a finding blocks, drives another iteration, or triggers escalation, and to audit those decisions. Stage 1 already gets that control from the existing approve/revise verdict plus the light-review doctrine, so a toolkit-wide schema change has no current consumer.
4. **Do not add a task facet now.** Triage uses the drafted change and repository evidence only.
5. **Extend `implement-quick` with the capped light-review loop as the initial experiment.** The `reviewer-light` model remains undecided and should be an explicit experiment variable.
