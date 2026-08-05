# 0129 — Run handles: background sessions, steering, cancellation

**Status:** filed, not scheduled (design-first; the original was runtime-family gated — claim-gate keeps this out of launch copy) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P389)

Competitors have spawn/notify/cancel on live agents; we have foreground drive +
park. The typed version: `ctx traits jobs` (list running sessions),
`ctx traits steer <session> "<note>"` (append a typed steering record the next
frame's prompt carries — auditable, unlike an opaque mid-stream message),
`ctx traits cancel <session>` (typed terminal state, ledger records who/why).

SDK direction (owner 2026-07-19): longer-term, expose a programmatic SDK for a
freer coding experience over runs — but STARTING FROM OUR STRICT LIFECYCLE, not
free-form scripting: the SDK's verbs are exactly the lifecycle
(start/drive/steer/cancel/resume over sessions with typed ledgers), so
programmatic orchestration inherits provenance by construction; loosening comes
later, if ever. Design lives with this task.

## Watch

- Steering is a LEDGER EVENT, not a side channel — every steer is provenance.
- Note: parts of the surface have since landed elsewhere (the dashboard lists,
  attaches, spawns and kills sessions; killing has a typed cancelled outcome) —
  the design should absorb what exists rather than duplicate it; the net-new core
  is `steer` and the programmatic lifecycle SDK.

## Done when

The design doc covers jobs/steer/cancel + the SDK lifecycle surface and is
owner-ratified, explicitly mapping what the dashboard already provides.

Full original contract: `archived/board/execution-plan.md` (Group 96, P389).
