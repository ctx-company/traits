# 0098 — `implement:guarded` — the commit waits for the owner's approval

**Status:** filed, ready (demo-scoped) · **Raised:** 2026-08-04 (owner design session; the decisions here are the contract) · **Touches:** `packages/agents/src/process.ts` (`commitTail`), `implement/source/sequence/family.ts` (`familyProcedure`), new `implement/source/variants/guarded.ts`

The demo: a run does the whole implement procedure, and at the end, instead of committing on its
own, it waits for the owner to approve. `ctx-gate run -- <command>` runs a command only when its
configured policy allows — otherwise it waits, polling until someone approves or refuses. From the
run's side it is just a command that takes a while and then exits 0 or not.

## Decisions

- **The gate is the command, not a new concept.** The commit step's command changes from
  `git commit -m <message>` to `ctx-gate run -- git commit -m <message>`. Nothing else in the
  procedure changes, and the trait system learns nothing new.
- **Three small additive edits.** `commitTail` gets an optional field — words to put in front of
  the commit command, plus a step timeout. `familyProcedure` passes it through. A new
  `variants/guarded.ts` sets it. Every other variant's generated output stays byte-identical.
- **The name is right.** "Guarded" here already means an approval stands between the work and the
  commit — today, two model reviewers. This variant adds one more approver: the owner. Default
  commits after two model approvals; guarded also waits for yours.
- **The timeout is absurdly high, on purpose, and lives in the trait.** Hours, not minutes (owner
  ruling 2026-08-04: demo-only, make it never fire). An undeclared timeout defaults low and would
  kill the hold mid-decision — declare it on this one step, per the standing rule that a step's
  ceiling lives in the trait.
- **Silence is already covered.** ctx-gate may print nothing while it polls; the repo's existing
  600-second idle budget is enough, because no demo waits longer than that on a held commit (owner
  ruling, same day). No heartbeat work on either side.
- **The step says what it is doing.** Title it "Commit the work (awaiting ctx-gate approval)" so
  the live run view narrates the hold by itself.
- **Approve → normal completion; deny → the run parks with the refusal on record.** The approval
  never enters the run — the run sees only the exit code. A nonzero exit fails the step and the run
  parks with ctx-gate's own output as the reason. A denied commit is a recorded outcome, not an
  error to debug.
- **Who approves, and how, is ctx-gate configuration.** The trait only says the commit goes through
  the gate; it never names the approver. Same split as everywhere else — the trait declares the
  need, config binds the person.
- **This is not an ask (0087).** No question is posed and no answer enters the run; the decision is
  recorded on ctx-gate's side, not in the run record. Keep them separate. The eventual bridge is
  0087's split (the run states the effect, the host performs it — a denial then lands in the run
  record first-class) and 0091 (park during the wait, resume on approval). This demo needs neither.

## Scope

The `commitTail` option, the `familyProcedure` passthrough, `variants/guarded.ts`, and a demo
runbook note (a ctx-gate policy example plus the preflight below). Base the variant on whichever
variant the demo dispatches (default or quick) — one line either way, since the option lives in the
shared procedure.

## Watch

- **The commit message is what the approver reads.** The scribe's message sits inside the command
  line ctx-gate shows, so the approval prompt describes itself. Free — do not add wiring to
  "provide context".
- **Exit codes.** Any nonzero parks. If ctx-gate later distinguishes denied / errored /
  gave-up-waiting, the park evidence can say "denied by owner" instead of "command failed" — nice,
  not needed now.
- **A missing `ctx-gate` binary is discovered only after the whole build loop ran.** The runbook
  preflights `ctx-gate --version` before dispatch; no doctor machinery for one demo.
- **Dry-run can never trigger an approval request** — it executes nothing. Stated so nobody
  worries.

## Done when

`implement:guarded` builds (drift/embed + builds + cargo test); dispatched on a small task it
visibly holds at the commit step; approving in ctx-gate lands the commit and the run completes with
the hash in its receipt; denying parks the run with ctx-gate's message in the park evidence; and no
other variant's generated canonical changed.
