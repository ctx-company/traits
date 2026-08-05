# 0103 — the great rename: abort-if and friends

**Status:** filed, ready · **Depends on:** 0102 · **Raised:** 2026-08-05 · **Touches:** packages/cdk/src (sequence.ts, prompt builders, policy constants), canonical schema + runtime parsing/ledger, every package canonical + lockfile, docs

Breaking renames are sanctioned now — before the functional layer exists — so the new layer is
born with the final vocabulary and never learns the old one. Behavior-preserving end to end:
nothing about what runs changes, only what it is called.

## Decisions

- **The renames.** Canonical `stop-if` → `abort-if`, `on-stop` → `on-abort`, step `emits` →
  `on-complete`, exhaustion policy value `block` → `abort`. TS-side: `prompt.text` →
  `input.prompt` (same tagged template, same lowering; `input.command` already exists), and the
  policy spellings become `flow.Continue` / `flow.Abort` constants evaluating to `"continue"` /
  `"abort"` — `flow.Block` and the old loose exports die.
- **One atomic churn.** Every package regenerates and relocks in this task, once. The runtime
  keeps reporting the same mechanisms in the ledger, receipt, and run-status under the new
  names — recorded evidence, not new semantics.
- **No compatibility shims.** Old spellings are rejected loudly after this lands, never
  aliased: an alias is a second spelling, and trait sources are evaluated rather than
  type-checked, so a silent alias would hide drift indefinitely.

## Scope

CDK field names, types, and constants; the canonical schema; runtime parse and ledger strings;
regeneration of all trait canonicals; the relock; docs and any doctrine resources that name the
old fields.

## Watch

- The whole-repo relock lands when no run is live (never rebuild a trait under a live run) and
  with main HEAD re-verified immediately before committing — other sessions land on main
  concurrently.
- The existing validations carry over verbatim under the new names: `--strict-loops` behavior,
  and on-abort rejecting policy keywords (an abort match always halts).

## Done when

Drift/embed + builds + cargo test green; the regenerated canonical diff contains renamed keys
and values and nothing else — any other hunk is a finding; grep finds no `stop-if`, `on-stop`,
`emits`, or `prompt.text` outside history and archived tasks.
