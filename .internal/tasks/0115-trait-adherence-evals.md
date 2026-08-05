# 0115 — Adherence evals: prove a trait changes behavior, per directive

**Status:** ready to implement · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P503, owner-ordered)

Every skills vendor ships prose with zero evidence it changes anything. We can ship
behavior packages with receipts — and it is the only mechanism that tells us WHICH
directives still earn their tokens as models improve (the calibration feed for the
intent/render seam). Also settles internal format questions (e.g. bullets vs
composed prose) with data instead of taste.

Per trait: a probe set (N openers that should trigger its behavior) + per-directive
checkable predicates keyed by directive id (`one-question` → "reply ends with
exactly one question"; `answer-leakage` → "contains no solution code"); run the same
harness twice — baseline vs injected — score both; emit an adherence report keyed by
directive id with the model-view digest, model id, and date. Deterministic checks
where possible, rubric-judge where not. Composes with the landed eval/scenario
fields and the auto-research loop pattern.

## Watch

- This is a PRODUCT capability and launch evidence, **never a gate on our own
  code** — the 2026-07-24 no-validation-testing-during-exploration ruling stands;
  this must not become a merge gate or a frozen expectation.
- Report numbers with model + date attached — they age with model generations,
  which is the point.

## Done when

One behavior-only trait has a probe set and an adherence report showing
per-directive pass rates injected vs baseline, stamped with digest/model/date;
re-running on a different model produces a comparable report.

Full original contract: `archived/board/execution-plan.md` (Group 121, P503).
