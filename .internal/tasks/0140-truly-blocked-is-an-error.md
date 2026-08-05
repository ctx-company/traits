# 0140 — Truly blocked is an ERROR: no commit, non-zero exit, halted batch, refused siblings

**Status:** ready to implement (first step: audit against landed park machinery — archived 0047 "run honesty" and 0050 "park report contract" plus the typed park reports may already cover parts; the wall-id sibling-refusal is the piece most likely still missing) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P414, owner-ordered 2026-07-21)

Incident basis: five queued phases burned full budgets against ONE known wall and
each landed a "blocked" chore commit — implementing nothing, yet reading as
SUCCESS to the batch's outcome gate, so the chain marched on ×4 after the first
wall-hit. A blocked ending that produces a commit is a lie with a receipt.
Leftovers (landed) cover approved-WITH-gaps; this covers the opposite cell:
"does the work stand alone? NO" — which must never ship anything, including a
chore commit.

Classify run endings: (1) approved → land; (2) approved-with-leftovers → land +
leftovers; (3) **BLOCKED** — final verdict still `revise` carrying a
contract-conflict/needs-owner blocker with no certified work — the tail BRANCHES
AWAY from scribe→stage→commit entirely: zero commits, a typed park report in the
ledger (blocker verbatim, wall source cited), a DISTINCT non-zero exit code,
batch recipes halt the chain on that code, and the park records a **wall id** so
dispatching a SIBLING task citing the same standing wall refuses at preflight
("wall <id> standing since run <x>") — the ×5 cascade becomes ×1.

## Watch

- Never conflate with approved-with-leftovers (that ships).
- Historical blocked-chore commits stay untouched in history.
- The branch guard reads verdict fields — coordinate with the shared reviewer
  verdict surface.

## Done when

A blocked run produces ZERO commits + a park report + the distinct exit code; a
two-task batch fixture halts at the first blocked ending (second never
dispatches); a sibling citing the same recorded wall refuses at dispatch; an
approved-with-leftovers run still lands normally.

Full original contract: `archived/board/execution-plan.md` (Group 102, P414).
