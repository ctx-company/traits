# 0019 — Render the frame's declared context: `<information>` with intent and behavior

**Status:** ready to implement · **Raised:** 2026-07-29 · **Rewritten:** 2026-07-29
**Related:** 0020 (flatten schemas) · 0021 (derive `<response>`) · 0035 (one source per fact)

## The defect this fixes is a lie, not a missing nicety

Every implement variant declares intent and behavior:

```toml
[[intent.require]] id = "leanness"
[[intent.avoid]]   id = "over-engineering"
[[behavior.tone]]  id = "direct"
```

`ctx traits explain` reads them. Activation scoring uses them. `compile.rs`
resolves each id to a directive through `intent_builtin`/`behavior_builtin`, and
a test (`directive_resolves_for_every_guidance_id_used_by_repo_traits`) asserts
every id a repo trait uses resolves to one.

**None of it reaches the model.** A live worker frame was dumped and checked:
zero occurrences of `tone`, `over-engineering`, or `leanness`; no `<intent>` or
`<behavior>` element; the frame's entire structure is `<input>{<prompt>,<data>}`
and `<output>{<format>,<schema>,<response>}`.

So the product advertises a behavior contract it does not deliver. That is the
reason to do this, independent of whether it improves output.

## Target shape

Everything contextual moves into one frame-level `<information>`; `<input>`
collapses to the operands it actually holds.

```
<information>
  <identity>You are agent:worker. Do only this step's work.</identity>
  <title>Building Produce</title>
  <description>Implements the draft and applies reviewer fixes.</description>
  <prompt>…the step's authored prompt…</prompt>

  <intent>
    <require leanness>Add only what the requested outcome needs; treat anything beyond that as scope creep.</require>
    <avoid over-engineering>Avoid speculative generality; build only what the demonstrated need requires.</avoid>
  </intent>

  <behavior>
    <tone direct>State conclusions and blockers plainly; do not bury them behind hedging or praise.</tone>
    <verbosity brief>Keep output compact unless risk explanation needs detail.</verbosity>
  </behavior>
</information>

<input>
  <data><draft>…</draft><repo-gates-passed>{"ok":false,"argv":["just","test"]}</repo-gates-passed></data>
</input>

<output>
  <budget>Your entire response must fit in N bytes.</budget>
  <format>…</format>
  <schema>…</schema>
  <response>…derived from the contract; see 0021…</response>
</output>
```

Reading order is **who I am and what I am doing → what I operate on → what I
must return.**

- **Id as attribute, directive as content.** The name stays greppable in a
  captured frame; the meaning is present without a lookup.
- **The identity line moves inside.** Today it floats above `<input>` as the one
  piece of framing not in a tag, with the step title welded into the sentence.
  Splitting identity from title makes both addressable.
- **`<budget>`** is a calculated, slightly conservative byte figure derived from
  the SAME constant the runtime enforces (`COMMAND_CAPTURE_LIMIT`). State the
  number; do not explain the mechanism. A P535 worker frame truncated against
  that ceiling and was re-dispatched as a fresh conversation, losing the turn —
  the model had no way to know a ceiling existed.
- **All seats see all intents** for now. Scoping intents per seat is deliberate
  later work.

## Delete the hand-written duplicates in the SAME change

The produce prompt currently says:

> "Prefer the minimal, correct, robust implementation: add no validation,
> abstraction, or defensive code beyond what the phase requires."

That is `require leanness` + `avoid over-engineering` written longhand. Once the
declarations render, that sentence is a second source for one fact — the drift
0035 exists to prevent. **The declaration wins; the prose goes.**

Sweep the other prompts for the same: anything an existing intent or behavior id
already says belongs in the vocabulary, not in authored prose.

## Watch

- **Dilution is the real risk.** The authored prompt is extremely specific
  ("re-run its argv verbatim — that exact command is what decides done-ness").
  The directives are general dispositions. Specific and general instructions
  coexisting can AVERAGE rather than add, blunting the tuned guidance. This is
  the main reason the duplicate deletion above is not optional.
- **Placement versus effect.** `<behavior>` governs the response but sits ~14 KB
  before the output contract, with the whole draft between them. If behavior
  demonstrably fails to land, moving `<behavior>` into `<output>` beside
  `<budget>` is the first thing to try — a one-line change, not a redesign.
  Treat placement as tunable, not settled.
- **The budget figure must derive from the enforced constant.** A hand-copied
  number drifts silently the day the limit changes, and the symptom is a
  truncated frame — exactly what this prevents.
- **`<information>` is context; `<data>` is operands.** Do not let slot values
  leak upward, or the two stop meaning anything distinct.

## Measure it, and do not overclaim

Cost is ~325 tokens (~7.5% of a measured 4,350-token produce frame): 7 intent
directives ~195, 4 behavior directives ~130. Cheap against a frame that runs 20+
minutes.

Its BEHAVIORAL benefit is unmeasured. Nothing in the artifact tells us whether
it changes what a model does. P503 (adherence evals: prove a trait changes
behavior) is the instrument for this and is still unlanded; running one phase
twice, with and without the block, and diffing the verdicts would answer it.

**Land this as "we stopped lying about the contract", not as "we improved the
prompt."** The first is verifiable today; the second is a claim we cannot yet
support.

## Measured (implementation round)

Real `ctx traits preview implement --step building-produce --json` frame, byte length
of the `prompt` field, before vs. after this change (before = HEAD `c2432f23` in a
scratch worktree, same trait, same step, no session so all data ports render pending):

- Before: 5,192 bytes (~1,298 token estimate at 4 bytes/token)
- After: 7,718 bytes (~1,930 token estimate at 4 bytes/token)
- Delta: +2,526 bytes (~+632 tokens), driven by the 7 intent + 4 behavior
  directive elements added to `<information>`; partially offset by deleting the
  longhand duplicate sentence from the produce prompt.
- `leanness` occurred once before this change too (inside the now-deleted
  longhand prose) and still occurs once after (now solely from the declared
  `<intent group="require" id="leanness">` element) — the fact moved to a
  single source, it did not newly appear. `over-engineering` and `tone`: 0
  occurrences before, 1 and 2 after respectively (both `direct` and `technical`
  behaviors carry `axis="tone"`) — these genuinely reached the model for the
  first time.
- `<budget>Your entire response must fit in 294912 bytes.</budget>` present
  after, computed as `COMMAND_CAPTURE_LIMIT * 9 / 10` (327,680 × 9 ÷ 10 =
  294,912) — not a hand-copied literal.

This confirms the frame carries the declared guidance and the duplicate prose
is gone, on a real frame, not an estimate. Behavioral benefit remains
unmeasured per the note above; P503 is still the instrument for that.

## Done when

A frame carries one `<information>` with identity, title, description, prompt,
intent and behavior sourced from the directive registry; `<input>` contains only
`<data>`; `<output>` states a derived byte budget; every authored prompt
sentence duplicating a declared id is deleted; the token cost is measured
against a real frame rather than estimated.
