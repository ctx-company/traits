# 0050 — Root-cause the park-report ledger-contract failure, then restore the output port

**Status:** code-verified, two clauses await owner-run evidence · **Raised:** 2026-08-01 (after this
failure killed runs repeatedly across a full day of runs) · **Verified:** 2026-08-04

## Verification (2026-08-04)

The fix, proofs, and port restore described below were already landed in commit `52a14830` ("Fix
stale output_ports in quick's guarded review loop, restore park-report port") before this run
started; `git merge-base --is-ancestor 52a14830 HEAD` confirms it is an ancestor of the current
branch. This run's work was verifying that landing against every done-when clause, not
re-implementing it — the status line above was stale relative to the code.

Evidence gathered this run:

- **Root-cause fix present**: `modules/core/src/procedure/runtime/control_flow.rs:60-63` — inside
  `refresh_runtime_status`, the early-return path for a guard-terminal run (`FinalState::Blocked |
  Failed | Rejected`) now recomputes `state.output_ports = finalize_outputs(trait_ref, state)?;`
  before returning, closing exactly the gap the surviving hypothesis names.
- **Regression proofs pass**: `CARGO_TARGET_DIR=target cargo test --test proof_park_honesty` — all
  8 tests green, including the two guarded-shape regressions
  (`guarded_exhaustion_refreshes_project_written_park_report_output`,
  `guarded_timed_out_stop_refreshes_project_written_park_report_output`).
- **Proofs bite (reverse-proof)**: reverted the 4-line `control_flow.rs` hunk in the working tree
  (`git diff 52a14830^ 52a14830 -- modules/core/src/procedure/runtime/control_flow.rs | git apply
  -R`), reran the two guarded tests — both FAILED. Reapplied the hunk (`git apply`, no `-R`), reran
  — both green again. Tree confirmed clean (`git status --porcelain`) before and after.
- **Port restored and rebuilt clean**: `port.parkReportPort` is declared in `quick.ts:141` and
  `quick/port.ts:20`. `ctx traits check implement:quick --verbose` reports `cdk-drift: 5 family
  leaves and source maps byte-match rebuilt ./.ctx/traits/packages/implement/source/index.ts (ok)`
  and `machine-trust: trust=verified` — no drift between source and the generated canonical despite
  `1884eaae` touching `quick.ts` after the fix commit.
- **Sibling variants covered by construction**: `smart.ts:192`, `strict.ts:153`, and
  `quick/port.ts:20` all still declare `parkReportPort`; the core fix lives in the
  variant-agnostic `refresh_runtime_status`, so every variant is covered without further code
  changes.
- **No stale containment comment**: grep for the `2026-08-01` park-report removal comment across
  `.ctx/traits/packages/implement/source/` finds none — the surviving `DISABLED 2026-08-01`
  comments are the unrelated feasibility-gate parking, not this task.
- **Standard gate green**: `CARGO_TARGET_DIR=target just test` (sdk-check, ts-format-check, cargo
  fmt --check, cargo clippy, `cargo test --workspace --lib`) passes with no new failures.

**Still open, and cannot be closed from inside this run** (per the standing reviewer-role ruling —
the owner runs live paid dispatches, this agent audits transcripts):

- Done-when clause "a parked run again surfaces its park report in structured final outputs" — needs
  a real dispatch that parks and an inspection of its structured output.
- Done-when clause "a full implement run in this repository completes without a ledger-contract
  error" — needs a real end-to-end dispatch.
- Re-approval of the rebuilt trait on the owner's machine (trust is a per-machine, per-digest
  decision; this run's `trust=verified` reflects this machine's prior approval of the current
  digest, not a fresh approval action).

This task stays open until the owner runs those dispatches and confirms the two live-run clauses.

## What was removed, and why this task exists

`port:park-report` was removed from `implement:quick`'s procedure output on 2026-08-01 as
CONTAINMENT, not as a fix. Runs were dying mid-flight with:

```
invalid manifest at run-session.ledger: run ledger contract invalid:
output_ports row port:park-report contradicts recomputed semantic output evidence
```

Removing the port stopped the bleeding immediately and cost nothing observable: `slot:park-report`
and both projections stay, so the park record is still written to the ledger every round, and the
standing-wall preflight still reads it (`dispatch_preflight.rs` reads `slot:park-report` from
`accepted_slot_values`, never the port — verified). What the port bought, and what is currently
missing, is park-report appearing in a parked run's STRUCTURED FINAL OUTPUTS, where an owner or a
tool reads it without opening the ledger. That is worth restoring once the defect is understood.

SCOPE: this task is ctx-trait ONLY. A run is confined to its own worktree in one repository, so
anything involving a sibling checkout (syncing a vendored copy, approving trust there, dispatching
a run there) is not in-run work and must never appear as a blocker here — a run that tries it
spins until it parks, which is exactly what happened on 2026-08-03 (run-f2799f78, blocker
`ctx-gate-restoration-missing`, three steps no in-run effort could ever close). Propagating the
restored port to any vendored copy elsewhere is an owner step, tracked separately.

## What is established (do not re-derive)

- The failure is the ledger contract check in `validate_output_port_contract`
  (`modules/core/src/procedure/runtime/ledger_contract_outputs.rs`): `expected` comes from
  `finalize_outputs(trait_ref, ledger)`, `actual` from the stored `ledger.output_ports` rows; a
  difference in any of value-slot-ref / required / status / value-digest reports "contradicts".
- `build_session` (`modules/core/src/procedure/session.rs`) runs this on EVERY state transition,
  so it kills live runs, not only resumes.
- `port:park-report` is the ONLY output port whose backing slot is written by `project` steps
  (`deriveParkReportStep`: clear to `[]`, then append inside the revise branch) rather than through
  a prompt/command step's declared output. Every other port's slot is written once through the
  normal acceptance path, which refreshes `output_ports` in the same transition. That is why the
  error never named any other port.
- All 641 ledgers on this machine are internally consistent — nothing on disk is corrupt, and the
  failure state is transient (a failing transition errors out without persisting).
- The known-good fixture `modules/cli/tests/proof_park_honesty.rs` declares the IDENTICAL shape
  (park-report as an optional output port with `value = "slot:park-report"`, clear/append project
  steps inside a branch inside a loop) and PASSES. So the shape alone does not reproduce it.

## Ruled out (each checked against every ledger in the run stores on this machine)

- Trait source/canonical digest drift — that has its own clean guard and a different message
  (`stale run-session source: loaded source digest does not match session ledger`).
- First-vs-last accepted value in `accepted_slot_values` (`upsert_runtime_value` keys on ref-text;
  no ledger holds two entries for one slot).
- The empty-array omission rule (every `[]` park-report row is consistent: status `accepted`,
  row digest == slot digest).
- The merge path (no merge frame ever records it).

## The surviving hypothesis to test first

A projection lands after the last `output_ports` refresh, leaving a stale row for the next
validation. `execute_project_item` → `record_accepted_slot_value` mutates accepted slot values
during control-flow advancement; `refresh_runtime_status`
(`modules/core/src/procedure/runtime/control_flow.rs`) recomputes `output_ports` at entry and again
after its progress loop. Find the path that reaches `build_session` with a projection applied after
the last recompute. Prime suspects, because the passing fixture does NOT exercise them: the guarded
loop's exit paths — `alsoRequire` (`repo-gates-passed.ok`), the timed-out gate `stop-if`, and
`on-exhausted: "block"` — where the loop may terminate between a projection and a refresh.

## Reproduction path

Extend the `proof_park_honesty.rs` fixture (or a scratch package) until it matches quick's build
loop, not just its park-report shape: add an `alsoRequire` guard, a `stop-if`, and `on-exhausted`,
then drive frames MANUALLY with `--no-drive` + `ctx traits call` (no model spend) through a revise
round so the clear/append projections run, and validate after each transition. If that reproduces,
the failing transition is identified exactly; if it does not, instrument `build_session` to dump
both fact maps on mismatch and run one real dispatch.

## The fix, then the restore

1. Repair the defect at its layer — either refresh `output_ports` after any projection that writes
   a port-backed slot, or make the contract check recompute project-written ports instead of
   comparing them. Prefer the former: the invariant "rows always equal a recompute from current
   state" is what makes the check meaningful, and weakening it hides real drift.
2. Add a regression proof at the exact shape that failed (quick's full guarded loop with a
   project-written output port, driven through a revise round).
3. Restore `port:park-report` to `implement:quick`'s procedure output in THIS repository,
   rebuild, relock, re-approve. The removal sites carry a `2026-08-01` comment naming this task.
   Note that procedure I/O is inferred from declared ports since 0046's rule-4 work landed, so
   restoring means re-declaring the port (`quick/port.ts` and quick's `port:` list) — which is
   also why merely deleting the containment comment is not enough.
4. Check the sibling variants (default/smart/strict/phase) — they declare park-report ports too and
   were never exercised as hard as quick; whatever fix lands must cover them.

## Watch

- Do not restore the port before the regression proof exists — this failure cost a full day of
  runs, and it is silent until a run dies mid-flight.
- Related: 0049 (a session should execute the bytes it started under, and ledger errors should name
  their cause in one readable sentence rather than an internal invariant).
- OUT OF SCOPE, explicitly: any change, check, approval or dispatch in a sibling repository. Runs
  are single-repo by construction; cross-repo propagation is an owner step and belongs on the
  board as its own task, not as a blocker inside this one.

## Done when

The exact transition that leaves a stale `output_ports` row is identified and fixed at its layer; a
regression proof fails without the fix and passes with it; `port:park-report` is back in
`implement:quick`'s procedure output in this repository, rebuilt, relocked and re-approved; a parked run
again surfaces its park report in structured final outputs; and a full implement run in this
repository completes without a ledger-contract error.
