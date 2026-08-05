# 0130 — Cost/token budgets per run: per-model split + subscription-aware estimation

**Status:** ready to implement · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P390, owner 2026-07-19)

`[budget]` caps frames/seconds only. Add token/cost ceilings per run + per
role-tier, enforced at frame dispatch with the typed pause (the
`paused-provider-credits` precedent → `paused-budget-exhausted`), resumable after
an owner raises the cap.

Cost model: spend splits PER MODEL (ledgers already carry per-frame token counts
where harnesses emit them) against a per-model pricing table — and the estimator
must be SUBSCRIPTION-AWARE: models billed through a flat subscription
(claude/copilot/codex seats) are marginal-cost-zero and must not be priced as API
tokens; the report shows both views (tokens by model, estimated $ by billing
mode), so a "cheap" run through a subscription and a cheap run through API credits
are distinguishable truths.

## Watch

- Enforce at frame dispatch only (never mid-frame — same boundary rule as 0117).
- Composes with 0117's pressure signal and 0120's stats surface; keep the pricing
  table config, not code.

## Done when

A run with a declared token/cost ceiling pauses typed at the boundary when it
would exceed it, resumable after the cap is raised; the ledger shows per-model
token and both-views cost evidence.

Full original contract: `archived/board/execution-plan.md` (Group 96, P390).
