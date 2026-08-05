# 0126 — Checklist coverage under `for-each`, or a stated refusal

**Status:** ready to implement · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P404)

Typed checklists check coverage — every declared item answered exactly once — only
for a whole-list `replace` write, and REFUSE `append` for checklist verdicts (a
single appended element cannot be judged against the whole list). A `for-each`
driven over checklist items has no coverage story at all: it is neither the
replace shape coverage validates nor the append shape that is refused. Fine while
nothing uses that shape; a silent hole the moment something does.

Where: `modules/core/src/procedure/runtime/schema_validation.rs`
(`checklist_coverage_validation`) + the for-each control path in
`control_flow.rs`. Decide whether a for-each accumulating verdicts gets coverage
checked at loop close, or whether that wiring is statically refused like `append`.

## Done when

A for-each over a checklist either has its coverage verified when the loop
completes, or fails validation at authoring time with a message pointing at the
replace shape — no third silent option.

Full original contract: `archived/board/execution-plan.md` (Group 101, P404).
