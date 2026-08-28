# The board: one door — decision record

Source document for a future plan run (`--task` can reference this file
by path). Raised 2026-08-25 from the owner brainstorm; rulings R1–R8 are
owner decisions of 2026-08-28 (three chat passes). Companion diagram:
`.hidden/2026-08-25-the-board-one-door.html` (note 06 of the lifecycle
series, gitignored — carries the full history: positions, calibrations,
and the dissolved §07 write-locus question).

## Context

Task management moves behind one interface. Today only the CLI and
dashboard hold the write capability (`TaskProviderMut`); runs go around
it with prompts and shell — the renumber script, the cksum snapshot
guards, and the commit tails in plan/architect are all compensation for
that lockout. The carrying decision: the board stops being repo files at
all. Tasks are entities in the system's own store, reached by every
writer through the interface; traits touch them only through CDK toolkit
calls; external trackers link as origins; the center adds liveness,
never correctness.

## Rulings (owner, 2026-08-28)

- **R1 — store, not repo.** Tasks live like ledgers in the system's own
  store: no commits, no worktree copies, no clone-carried board. Kills
  the compensation layer whole — symbolic-key renumbering, commit tails,
  cksum guards, the 8-slice cap, the board path baked into trait
  canonicals — and dissolves the write-locus question outright. The
  fallback (files kept in-repo, agents sandboxed away from the
  directory) was considered and set aside.
- **R2 — toolkit access.** The model seat returns typed drafts into a
  slot; the authored trait submits them:
  `toolkit.tasks.create("creating tasks", slot.draft.tasks)` —
  batch-shaped by construction, driver-executed, ledger-evidenced. Reads
  arrive the same way: a task behaves as if it sat in a slot. Joins the
  toolkit namespace 0253.5 introduces.
- **R3 — in-process.** Every process talks to the store directly; board
  operations work with nothing else running. The center only adds
  `watch` (live pushes to faces) and, later, custody of origin
  credentials.
- **R4 — origins, not backends.** Tasks stay internal entities, always.
  A Linear ticket or GitHub issue is a story an agent picks up and
  architects internal tasks from — linked as an origin, never the task
  itself, never a second implementor. Linear itself: unscheduled.
- **R5 — claims refuse, never queue.** Grabbing an occupied task fails
  immediately and names the holder. No waiting-to-obtain exists anywhere
  in the design.
- **R6 — park = signal.Abort.** The parks claims care about are
  trait-declared aborts carrying signal handles and verdict evidence;
  `signal.Park` (the P264 branch policy) and the P479 tripwire are the
  two edge producers — all three emit the same release-with-report
  record. Timeouts fail unless routed; budget ceilings pause;
  interrupted and killed are owner stops — none are parks. Claim
  mapping: parks release and attach the report; pauses and
  awaiting-owner keep the claim held.
- **R7 — close-on-outcome.** A landed run settles its claim through
  close(evidence) — mostly built already as `update` + `set_closure`
  (0144) promoted to a named verb. The auto-close policy
  (confirm/checked/merge) survives and keeps deciding close-vs-propose;
  only the dashboard lane as the sole path dies.
- **R8 — editing stays, mediated.** An edit action in ctx opens $EDITOR
  over the raw task; the save returns through the interface. Safer
  shapes later.

## Build order

1. Interface + claims + close-on-outcome — pure Rust in driver and
   dispatch, zero CDK or trait changes.
2. Plan/architect converted to toolkit calls after 0253.2/.3 land; the
   compensation layer is deleted.
3. Center hosting + `watch`.
4. Origins / Linear, sometime.

## Open before filing tasks

- Stop-record shape: one record and one dashboard list for summons-stop
  (claim held, question attached) vs park-stop (claim released, report
  attached) — offered as builder-level, awaiting disposition.
- Plan creates, draft vs sweep: a plan run's tasks exist in the store
  before anyone reviews them — draft-then-accept, or live with
  rejection-sweep.
- Migration: 329 files (102 live + 227 archived) move into the store
  once; the task-board directory resource and `port:task` path plumbing
  retire with them.

## Accepted consequences to keep in view

- No clone-carried board until a sync story exists (center-hosted or
  origin-backed).
- Task history needs store-side journaling once git history stops
  covering it (digest CAS wants revisions regardless).

## Sources

core `task/provider.rs` (TaskProvider/TaskProviderMut, `set_closure`,
`Closure`, `AutoClosePolicy`); io `dispatch_preflight.rs`
(`find_standing_wall` — walls become a claim phase); toolkit
`sequence/feasibility.ts` and `sequence/park-report.ts` (P414 derived
park reports); cdk `signal-verb.ts` (the 0209 verb set);
`proof_tripwire.rs` (P479); tasks 0144, 0253.2–.5.
