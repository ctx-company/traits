# Rethink sessions & runs: do we need both, or is the session enough?

## The idea

ctx.traits currently carries two identity namespaces for one execution: a
`session_id` and a `run_id`. The question is what the split actually buys us
today, whether the session alone is enough, and what a collapse would touch.

## What the split is today (observed in this repo)

**The native start path mints both IDs from the same seed, 1:1.**
`modules/io/src/run.rs:956-988` mints `session-<hash>` and `run-<hash>` from
one `fresh_identity_seed(trait_id)` (`run.rs:83`: nanos | pid | counter |
trait-id), differing only by the `"session|"` vs `"run|"` prefix fed to the
digest. No session ever has a second run; no run outlives its session. The
owner decision recorded there (Group 42.5 F2) is that every native start mints
fresh identity — deterministic derivation was removed after a smoke run
silently clobbered a paid mid-flight run. The WASM adapter is the exception:
`run_start_json` (`modules/wasm-core/src/abi.rs:546-575`) still calls
`SessionId::deterministic` and `deterministic_run_id`; its session seed includes
inputs while its run seed does not, so different-input starts can even share a
run id. `RunStartRequest` has no identity field, and wasm-core cannot mint a
host-fresh identity because it deliberately has no process or persistence
access (`abi.rs:947-965`).

**The session envelope already owns the execution ledger.** `Session`
(`modules/core/src/procedure/session.rs:1435`) holds `session_id`, `run_id`,
`status`, frames, provenance, and `ledger: State` — the pure run ledger from
`modules/core/src/procedure/run/` (planned sequence items, slot states,
producer edges). The module docs describe the session as "a stable
session/frame/call envelope" *wrapping* the run ledger. The layering is real
and useful; the dual public ID namespace is a separate choice layered on top
of it.

**What the dual namespace costs, concretely:**

- `find_session_by_run_id` (`modules/io/src/run_session.rs:280`) linearly
  scans every ledger in the store deserializing each one to map a run-id back
  to its session. `ctx traits merge <run-id>` depends on it
  (`modules/cli/src/app/merge.rs:523`), with a `find_by_run_id` fallback
  against the center index and a `validate_selected_run_id` guard after.
- CLI verbs accept four spellings of the same thing: "Run-id, full session
  ID, unambiguous session-ID prefix, or ledger path"
  (`modules/cli/src/app/surface/cli.rs:876-879`, resume; same for
  `sessions story`). Prefix resolution (`resolve_session_path`, P421) and
  short-display logic (P506 §3.4) exist only for session IDs, so run-ids get
  a parallel, less-capable resolution path.
- Every `CallSubmission` must carry *both* IDs plus a `state_digest`;
  `submit_run_call` rejects on session-id mismatch, run-id mismatch, and
  missing run-id separately (`session.rs:2747`, `session.rs:2849-2860`). The
  run-id check is redundant given session-id + state-digest.
- The center projection carries both (`{"session_id","run_id","trait_id",
  "status"}`). `run_id` is embedded in the `RunSummary` JSON stored in
  `center_rows.summary`, then searched linearly in the center's in-memory map
  by `FindByRunId`; there is no sqlite run-id column or SQL index
  (`modules/io/src/center.rs:1883-1891,1958-2002,4297-4311`).
- Vocabulary has already fused in self-defense: the code says "run-session"
  nearly everywhere (`run_session.rs`, "run-session ledger", "run-session
  store"), the active *runs root*
  `~/.config/ctx/traits/runs/<repo-key>/` stores files named
  `<session-id>.json`, worktrees derive from the session id
  (`derive_worktree_id`, `modules/io/src/worktree.rs:190`) while branches
  live under `ctx/run/<id>`. Two names for one thing, distributed at random.

## Researched grounding

- **Temporal — Workflow Id vs Run Id**
  (https://docs.temporal.io/workflow-execution/workflowid-runid): the
  canonical two-ID design. Workflow Id is the stable business identity; Run
  Id names one execution attempt. The split earns its keep because retries
  and continue-as-new mint a *new* Run Id under the *same* Workflow Id
  (https://docs.temporal.io/workflow-execution/continue-as-new), and Temporal
  explicitly warns against using Run Id in logic because it is mutable.
  ctx has the mechanism without the motive: native IDs change together, while
  WASM's second deterministic ID can be reused across different inputs.
- **GitHub Actions — re-run attempts**
  (https://docs.github.com/en/actions/how-tos/manage-workflow-runs/re-run-workflows-and-jobs):
  the opposite resolution of the same need. Re-runs keep the *same* run id
  and add an `attempt` ordinal (`github.run_attempt`) — multi-attempt
  history without a second global namespace.
- **LangSmith — run/trace/thread hierarchy**
  (https://docs.langchain.com/langsmith/observability-concepts): multiple
  levels are justified there because the cardinality is real — many runs per
  trace, many traces per thread. A 1:1 level is not a level.
- **Claude Agent SDK — sessions**
  (https://code.claude.com/docs/en/agent-sdk/sessions): the closest peer
  system uses a single session id as the only execution identity; forking
  mints a fresh session id rather than a sub-identity. This is the shape ctx
  actually has, hidden under two names.

The pattern across prior art: a second identity is warranted only when its
cardinality differs from the first (many runs per workflow, many attempts per
run). At 1:1 it is pure overhead.

## Recommended approach: collapse public identity to the session

Keep the layering — a pure execution ledger wrapped by the session envelope —
but place it under session vocabulary. Delete the identity split: the session
id becomes the only name for an execution, in memory and on disk.

This is one coordinated cut. The steps below are implementation order, not
independently shippable compatibility phases:

1. **Mint or require one session id.** Native starts keep minting a fresh
   `SessionId`. WASM `RunStartRequest` gains a required caller-supplied
   `session-id`; the host must mint it freshly before calling pure wasm-core.
   Remove `SessionId::deterministic`, `deterministic_run_id`, and every WASM
   `run-id` request/response field in the same ABI change. Caller-supplied is
   the correct boundary because wasm-core owns neither entropy nor
   persistence, while core `StartRequest` already accepts identity from its
   adapter.
2. **Migrate persisted state, then remove `run-id` immediately.** Add a
   bounded one-shot migration that reads the old typed ledger shape, makes
   `session-id` authoritative, removes the outer, nested-state, frame-template,
   and frame `run-id` fields, and recomputes the state digest before writing
   atomically. Migrate persisted liveness, holder, sidecar, lock, debug, board,
   and provenance records from run identity keys to session identity keys in
   the same pass. After a successful migration there is no serde alias,
   mirrored write, dual-read path, or `run_id` left in the runtime schema.
3. **Rename storage and identity vocabulary in the same migration.** Rename
   `~/.config/ctx/traits/runs/` to `sessions/`, the corresponding state helpers
   and `CTX_CENTER_RUNS_ROOT`, `run-session` APIs/files/types to `session`, and
   `ctx/run/<id>` refs to `ctx/session/<id>` while updating stored worktree
   provenance. The existing `plan_state_migration` only moves same-named
   families from the old parent, so this needs an explicit `runs` → `sessions`
   migration with collision refusal and atomic per-artifact writes. Keep
   `ctx traits run` as an action verb; move the pure ledger types under the
   session namespace and rename execution-position nouns rather than leaving
   public or persisted run identity vocabulary behind.
4. **Make every consumer session-only.** `merge`, `resume`, and `sessions
   story` accept and print session refs through the existing P421 prefix +
   P506 short-display machinery. Remove `find_session_by_run_id`,
   `FindByRunId`, `validate_selected_run_id`, and the redundant submission
   check. Migrate the center, CLI dashboard/tasks, desktop rows/actions,
   liveness, task-board joins, and standing-wall evidence to `session_id`.
   The center needs no SQL column migration: bump its projection version and
   rebuild disposable `center_rows.summary` from migrated ledgers. The CLI and
   desktop are the only in-repo center consumers found; the private socket
   protocol and `publish = false` crates provide no evidence of an external
   center consumer.

**Future-proofing without building it now:** if re-attempt semantics ever
land (retry a failed sequence under the same identity), follow GitHub
Actions, not Temporal — an `attempt: usize` ordinal *inside* the session
ledger, session-scoped, needing no global namespace, no store scan, no new
resolution grammar. That door stays open at zero cost today.

## Alternatives and tradeoffs

- **B. Make the split real (Temporal-style).** Session id becomes a stable
  identity across re-runs; each start mints a run id under it; merge/story
  address runs, resume addresses sessions. Honest version of the current
  shape, and it would give re-run lineage. Rejected: nothing in the repo
  needs multi-attempt today — the one incident in this area (deterministic
  ids clobbering a paid run) was *caused* by identity reuse and fixed by
  fresh-per-start. Building attempt lineage now is generality ahead of
  demonstrated need, and it makes the resolution grammar richer, not
  simpler.
- **C. Status quo.** Zero migration cost. Ongoing cost: an in-memory center
  scan for default merge-by-run-id and an O(store) deserialize scan with an
  explicit session store, doubled validation in every call submission, four
  spellings per CLI verb, and every new surface (center, dashboard, MCP,
  WASM) forced to thread both IDs forever. The "run-session" compound noun is
  the codebase telling us the concepts already merged.
- **Naming-only variant of A.** Keep two fields, define `run_id ==
  session_id`. Cheapest diff, but preserves both namespaces' plumbing and
  the confusion. Rejected even as a temporary state: the owner requires the
  persisted field to be removed in this change.

## Concrete touchpoints

- `modules/io/src/run.rs:83,956-988` — single fresh `SessionId` minting.
- `modules/core/src/procedure/session.rs` — remove `Session.run_id`,
  `StartRequest.run_id`, `CallSubmission.run_id`, checks at 2747/2849-2860,
  and `deterministic_run_id`.
- `modules/core/src/procedure/run/` — move the pure ledger implementation under
  the session namespace; remove its identity `Id` and use the envelope's
  `SessionId` where identity is required.
- `modules/core/src/procedure/runtime/state.rs` and `frames.rs` — remove
  persisted nested/frame `run-id` copies and rename execution-position nouns.
- `modules/io/src/run_session.rs:280` — rename the store API and delete
  `find_session_by_run_id` in the same cut.
- `modules/cli/src/app/merge.rs:253-581`, `story.rs` — target becomes a
  session ref; `validate_selected_run_id` removed.
- `modules/cli/src/app/surface/cli.rs` — arg docs for resume/story/merge.
- `modules/io/src/center.rs` and `run_summary.rs` — session-only projection,
  projection-version rebuild, and migration of all in-repo consumers; there
  is no sqlite `run_id` column.
- `modules/wasm-core/src/abi.rs:261-280,546-575` — require a host-supplied
  `session-id`; remove deterministic and run-id ABI paths now.
- `modules/io/src/state.rs:323-437` — dedicated `runs` → `sessions` family
  migration; the current same-name family migrator is insufficient.
- `modules/io/src/worktree.rs` — migrate `ctx/run/<id>` refs and provenance to
  `ctx/session/<id>`.

## Owner decisions applied

1. **WASM:** require a host-supplied fresh `session-id`. Deterministic identity
   is unsafe for a persisting host and wasm-core cannot create host freshness.
2. **Persisted `run-id`:** drop it in this change, with a one-shot typed state
   migration rather than a runtime serde alias.
3. **Vocabulary and paths:** rename run identity/storage nouns to session,
   including the active `~/.config/ctx/traits/runs/` root and `ctx/run/*` refs.
4. **Downstream rows:** the center projection feeds merge, liveness,
   task-board/standing-wall logic, CLI dashboard/tasks, and desktop surfaces.
   Migrate all of them to `session_id`, bump and rebuild the disposable center
   projection, and leave no `run_id` field or lookup behind.
