# 0114 — Public, honest feature matrix + per-competitor rebuttals

**Status:** filed, not scheduled (claim-gated: every cell must be provable against the shipped product) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P247)

HN fact-checks every comparative claim within the hour; a matrix that marks an
unshipped row ✓ gets torn apart in the comments and poisons the whole launch —
whereas a matrix that openly marks what's gated/planned is itself the credibility
play.

Produce: a public feature matrix — rows = (typed / versioned / lockfile /
transitive-audit / drift / multi-harness / render-to-host), columns = ctx vs
SkillFortify / Skills-Are-Not-Islands / MCP / Ruler / Agent-Skills / AGENTS.md /
Cursor — each cell one of ✓ / partial / **gated** / **planned**; plus a short
per-competitor rebuttal paragraph (already drafted as prose in the launch docs)
under the table.

Good looks like: transitive-audit and multi-harness are marked **planned/gated**
for ctx (NOT ✓ — they are not shipped), and every remaining ✓ survives a hostile
reviewer clicking straight through to a real command or doc; each rebuttal attacks
the gap, not the project.

## Watch

- Every cell must pass the claim gate (no cell may assert what it can't back).
- The transitive-audit row stays **planned** — its shipping mechanism is not yet a
  queued task.
- The competitive dossiers at `.docs/research/COMPETITIVE_SX.md` /
  `COMPETITIVE_SLATE.md` (landed P444) feed the rebuttals — no re-research needed.

## Done when

The matrix + rebuttals exist with every cell claim-gated; hostile click-through on
each ✓ lands on a real command or doc.

Full original contract: `archived/board/execution-plan.md` (Group 59, P247).
