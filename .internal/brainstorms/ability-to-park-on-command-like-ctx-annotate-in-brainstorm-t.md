# Park on command: callback-completed brainstorm gate

## The idea

The brainstorm trait's owner-acceptance gate is a `cdk.step.command` that runs
`ctx-annotate --stdin` synchronously (`.ctx/traits/authored/brainstorm/source/shared/step/gate.ts`).
While the owner reads the proposal, the drive process blocks inside
`ctx_traits_io::run::advance_commands`, holding the terminal, the worktree, and
the run's wall-clock. The command engine treats owner silence as a hang: a
command step that produces no output is killed after `DEFAULT_COMMAND_IDLE_MS`
(10 minutes) and unconditionally after `DEFAULT_COMMAND_WALL_MS` (4 hours)
(`modules/io/src/command.rs:29,36`). The gate script's fallback then emits
"annotation gate returned no decision; retrying next iteration", burning a
round of the bounded acceptance loop. So today the run either times out or
just sits there — exactly the two failure modes the topic names.

The proposal: let a command opt into **callback completion**. The Rust runtime
persists a pending-command record, launches a tiny detached command worker
bound to that exact frame/session/argv, and exits the drive cleanly. The worker
runs the command and reports its exit evidence to the center; the center
atomically completes a successful frame and resumes the run, or keeps a failed
frame parked for explicit retry/cancel. The
brainstorm gate keeps being a command — it no longer needs an Ask-specific
handoff abstraction.

## Researched grounding

Every mature workflow system converged on the same shape for human waits:

- **AWS Step Functions callback pattern (`waitForTaskToken`)** — the workflow
  emits a durable task token and pauses; an external actor returns the token
  via `SendTaskSuccess` to resume. Lesson: bind each worker callback to a
  single parked frame with an opaque, one-use token. Step Functions permits an
  optional bound via `TimeoutSeconds`; a parked command here has no callback
  TTL unless its declaration explicitly sets one.
  https://docs.aws.amazon.com/step-functions/latest/dg/connect-to-resource.html
- **Temporal human-in-the-loop signals** — `wait_condition()` returns the task
  to the server; the worker goes idle and consumes no compute; a signal is
  durably recorded and replayed even across worker crashes. Lesson: parked
  identity lives in the ledger, not only in worker memory. The proposed local
  callback worker still remains live, so host loss or reboot orphans it rather
  than inheriting Temporal's stronger durability. https://temporal.io/blog/human-in-the-loop-approvals and
  https://learn.temporal.io/tutorials/ai/building-durable-ai-applications/human-in-the-loop/
- **LangGraph `interrupt()` / `Command(resume=value)`** — pause surfaces a
  value to the caller; resume injects the human's answer back into the paused
  point; a checkpointer is mandatory. Caveat worth copying into our contract:
  on resume the interrupted node re-runs from its start, so recovery must never
  silently launch an orphaned command a second time; retry is an explicit owner
  action. https://www.abstractalgorithms.dev/langgraph-human-in-the-loop
- **Airflow deferrable operators** — a waiting operator suspends itself and
  frees its worker slot; a tiny async trigger owns the wait and signals
  resumption. Lesson: separate the *wait* (cheap, declarative) from the *work*
  (the drive loop), or idle waits starve the fleet — the remote-runtime version
  of our problem.
  https://airflow.apache.org/docs/apache-airflow/stable/authoring-and-scheduling/deferring.html

## What this repo already has

Most pieces needed by a callback command already exist, but not their binding:

- `Status::WaitingOnHuman` (`modules/core/src/procedure/session.rs:100`): the
  drive arm at `modules/cli/src/app/drive.rs:2466` exits cleanly with
  `"awaiting-owner"`; the session ledger persists in the session store
  (`modules/io/src/run_session.rs`).
- Command frames already carry resolved argv, cwd, success exit codes, output
  slot, call template, run/sequence position, and state digest
  (`modules/core/src/procedure/runtime/frames.rs`; consumed in
  `modules/io/src/run.rs:4721`). That is the callback identity; do not invent a
  second command model.
- `advance_command_frames` already has one success/failure convergence point:
  it turns `RunOutput` into `CommandExecutionEvidence` and submits through
  `submit_run_call` (`modules/io/src/run.rs:4814-5052`). Extract and reuse that
  landing logic for synchronous execution and callback completion.
- The command runner already owns cwd/env, timeout, capture, executable-digest,
  and success-exit-code policy (`modules/io/src/command.rs`). The worker must
  call it rather than spawn independently and drift.
- The center daemon (`modules/io/src/center.rs`) tracks every session as a
  public row, accepts correlated local requests over its Unix socket, and has
  maintenance-lock reconciliation. Add one idempotent command-finished request
  there; the ledger remains authoritative, not the center's SQLite cache.
- The CDK command surface already carries execution policy beside argv
  (`timeoutMs`, `idleTimeoutMs`, `successExitCode` in
  `packages/cdk/src/sequence.ts:216-229`), so callback completion belongs on
  `CommandSequenceFields`, not on Ask.

The gap: command execution and frame submission are one blocking Rust call.
There is no durable pending-command identity and no trusted callback endpoint,
so `ctx-annotate` must keep the drive alive while the owner thinks.

## Recommended approach: callback-completed commands

Add one explicit command mode, spelled `completion: "callback"` in the CDK.
"Async" is avoided because it conventionally means fire-and-forget; this mode
still gates the sequence on the command result. The Rust runtime wraps the
command in an internal worker, parks, and accepts exactly one authenticated
completion callback.

1. **CDK and core declaration**: add `completion?: "blocking" | "callback"` to
   `CommandSequenceFields`; omit/`blocking` preserves every current command.
   Lower it to `completion = "callback"` on `CommandPlan`/`CommandDeclaration`
   and carry it unchanged into the resolved command frame. No annotate-specific
   sugar or handoff descriptor.
2. **Park record**: before launching, persist `PendingCommand { callback_id,
   token_hash, wrapper_pid: None, frame identity/state digest, logical argv,
   started_at }` and a distinct `WaitingOnCommand` status. After spawn, bind the
   wrapper's own PID to that same record before releasing its random,
   one-use callback token over a private descriptor; neither the wrapped command
   nor its child PID participates in callback identity. Persist-before-spawn
   makes every accepted callback address an already parked frame. The wrapper
   PID supports best-effort liveness checks and diagnostics, but is not
   authentication because PIDs can be reused.
3. **Tiny worker**: re-exec the current binary as
   `ctx traits internal command-worker`, detached from the drive. It receives
   the session locator and resolved execution context, waits for the parent to
   bind its PID and release the token, then invokes the existing
   `command::run_with_env`, retains stdout/stderr/exit/timeout/truncation
   evidence, and forwards that immutable result to the center. On transport or
   center unavailability it retries delivery, not command execution, until it
   receives a terminal acknowledgement, an explicit cancellation, or an
   explicitly configured callback TTL expires. This is a thin owner of the
   existing runner, not a second runner. Expiry is reported as a timeout failure,
   not treated as successful completion.
4. **Callback landing**: add `CommandFinished` to the center's correlated
   request protocol. Under the ledger maintenance lock, validate callback id,
   token, bound wrapper PID, state digest, and current frame; then feed the
   worker's `RunOutput` through the same extracted command-submission function
   used by blocking commands. Core, not the worker, decides success from
   `success_exit_code`, validates output, and either completes the frame or
   records its recoverable failure in the same ledger write. Its correlated
   response definitively acknowledges
   accepted/already-applied callbacks and refuses stale or cancelled ones, so
   the wrapper can stop retrying without ambiguity.
5. **Resume and failure**: after an accepted successful callback, the center
   launches the normal detached drive for the next frame. Any failure — worker
   spawn, callback TTL, command execution timeout, non-success exit, invalid
   output, or a confirmed-absent wrapper — leaves a recoverable failed command
   and offers explicit retry/cancel instead of driving or replaying argv. Its
   projection includes a typed reason plus the available exit status,
   stdout/stderr, timeout, and truncation evidence. Retry deliberately starts a
   replacement command and resumes normal callback handling; cancel terminates
   the parked attempt. Neither action occurs without the user's decision because
   the failed or missing command may already have produced side effects.
6. **Brainstorm trait**: keep `GATE_SCRIPT` and add only
   `completion: "callback"` to `acceptanceGate`. Its stdout still writes
   `ownerAnswer`; its existing empty-annotations projection and 500-round
   acceptance backstop stay byte-for-byte conceptually unchanged. Callback
   parking spends no loop round because the command frame has not completed.
   Callback mode changes completion and process lifetime only: the declared
   command executes through the same `cdk.step.command` path, with no
   command-specific notification hook. It has no callback TTL by default,
   though the declaration may set one; the normal 10-minute idle bound is
   disabled for this human-owned worker.
7. **Remote runtime (future, unlocked not built)**: the same callback envelope
   can cross a transport later, but MVP is machine-local: one center socket,
   one ledger, one detached worker. Do not build a remote queue or Ask bridge
   until a remote executor exists.

## Alternatives and tradeoffs

- **A. Ask + handoff summons** — attach argv to an Ask and have an owner
  surface launch it. This durably survives a host reboot because no process is
  expected while parked, but it special-cases external human tools and splits
  command execution between Ask/answer and the command runtime. Rejected in
  favor of the owner's wrapper: callback completion reuses one command model
  and also serves long-running non-human commands.
- **B. Interactive-exempt command steps** — mark the gate `interactive: true`,
  disable idle/wall bounds, keep blocking. Smallest diff, fixes only the
  timeout half: the run still "just sits there" holding the drive process,
  terminal, and worktree, and dies with a crash or sleep. Rejected as the
  primary; callback completion retains the command but releases the driver.
- **C. Poll-and-requeue (status quo hack)** — let the gate time out and retry
  next iteration. Burns bounded-loop rounds to model a wait, couples patience
  to `maxIterations`, and re-opens the diagram every round. This is what the
  fallback string in `gate.ts:25` already does by accident; it is the
  anti-goal.
- **D. Poll a decision file** (Airflow triggerer shape) — the center polls for
  command output and re-drives. Adds polling and loses the exact exit evidence
  the worker already has; callback delivery is direct and idempotent.

## Concrete touchpoints

- `packages/cdk/src/sequence.ts` — `CommandSequenceFields.completion`; canonical
  lowering and generated types/schema.
- `modules/core/src/trait/procedure/model.rs` and runtime frame builders — carry
  callback completion policy on the existing command plan/frame.
- `modules/core/src/procedure/session.rs` — `WaitingOnCommand` and the durable,
  additive `PendingCommand` record.
- `modules/io/src/run.rs` — split command launch from shared result landing;
  persist pending state and validate callback identity.
- `modules/io/src/command.rs` — reuse `run_with_env` in the internal worker; no
  second subprocess implementation.
- `modules/io/src/center.rs` — correlated `CommandFinished` request, locked
  idempotent landing, resume dispatch, orphan projection.
- `modules/cli/src/app/drive.rs` and internal command routing — launch the
  worker, return `awaiting-command`, and expose worker failure/retry/cancel.
- `.ctx/traits/authored/brainstorm/source/shared/step/gate.ts` — add
  `completion: "callback"`; rebuild the authored trait package normally.

## Owner decisions

1. A parked command has no callback TTL by default. Its declaration may set an
   optional TTL when a bounded wait is desired. Reaching that TTL is a
   recoverable timeout failure, not completion; the run remains parked for an
   explicit retry or cancel decision.
2. After a valid successful callback completes the parked command frame, the
   center starts the normal detached drive for the next frame. Any failed
   callback or orphaned worker instead leaves the run parked and exposes only
   retry/cancel; it never automatically re-executes argv. The failure projection
   distinguishes timeout, spawn/orphan, non-success exit, and invalid output and
   includes the available exit status, stdout/stderr, timeout, and truncation
   evidence. This prevents an infinite replay loop and lets the user decide
   whether possible command side effects make retry safe.
3. Parking pauses the run without cleaning it up. The worktree remains in place
   and the worker continues the existing command in its original cwd.
4. Callback completion adds no command-specific notification behavior.
   `ctx-annotate`, or any other declared command, executes exactly as the same
   `cdk.step.command` would execute in blocking mode.
