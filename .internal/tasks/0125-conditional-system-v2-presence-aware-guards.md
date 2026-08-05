# 0125 — Conditional system v2: presence-aware guards, designed properly

**Status:** ready to implement (DESIGN-FIRST — a design doc before code) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P432, owner call 2026-07-21: build the proper system, not the narrow cap hack)

Guards are static predicates over always-present fields. They cannot express "this
cap binds only when the caller supplied it", "this field may be absent", or "these
two list counts must match". That wall forces trait forks: auto-research needed a
2-condition and a 6-condition keep-guard as two traits, and deep-research's
cardinality guard cannot compare two counts.

Candidate scope: presence conditions over optional ports and optional fields
(present/absent with defined truth semantics per operator); conditional binding
with FAIL-CLOSED semantics (a cap that is supplied but unmeasurable in the result
DISCARDS rather than passes); count-to-count comparison; composition with existing
all/any/not; static validation that every referenced optional field/port is
declared optional.

Explicitly OUT: arithmetic between arbitrary fields, ordered folds/replay logic
(stays in pinned scripts by design), and any general canonical expression language
(standing house refusal).

RIDER (2026-07-23, CDK audit): loop `until:`/`stop-if:` gain the same
check-polymorphism P458 gave branch `check:` — a `sequence.check` gate step
directly as the loop guard, killing the boolean-slot + `condition.equals` two-step;
same desugaring, no extra schema surface.

## Done when

The design doc enumerates the vocabulary with absent/present truth semantics for
every operator; implementation passes a fixture matrix covering absent-port,
absent-field, supplied-but-unmeasurable (fail-closed), and count-vs-count; the
auto-research unification's single keep-guard (landed P434 shape) is expressible.

Full original contract: `archived/board/execution-plan.md` (Group 104, P432).
