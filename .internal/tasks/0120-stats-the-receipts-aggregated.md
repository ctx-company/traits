# 0120 — `ctx traits stats`: the receipts, aggregated

**Status:** ready to implement · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P442)

Run ledgers already record tokens, frames, outcomes, and trait digests per run —
richer telemetry than competitors' adoption widgets — but nothing aggregates it;
the "loops with receipts" pitch needs a surface.

`stats [--since] [--trait] [--json]` over the run store: runs per trait, tokens per
trait, outcome split (completed / exhausted-unapproved / blocked / killed), average
refinement rounds to approval. Repo-scope now; machine-wide automatically once the
global-tier resolution (0121) is in play. Strictly read-only — no new telemetry is
written.

## Done when

Stats over an existing day's ledgers reproduces hand-counted numbers; `--json` is
stable; an empty store prints a clean zero state.

Full original contract: `archived/board/execution-plan.md` (Group 109, P442).
