# `implement:guarded` demo runbook (0098)

`implement:guarded` runs the same loop as `implement:quick`, except the final commit is wrapped
in `ctx-gate run --`: instead of committing on its own, the run waits for the owner to approve
the commit through ctx-gate.

## Preflight

Confirm `ctx-gate` is on `PATH` before dispatching — there is no `--version` flag, so `--help` is
the check:

```
ctx-gate --help
```

A missing binary is a repository/machine misconfiguration, not something the run can fix; do not
dispatch until this exits 0.

## Dispatch

```
.internal/scripts/implement.sh --trait implement:guarded "<task>"
```

Pick a small task for the demo — the loop runs the full quick procedure (draft, then
worker/reviewer rounds) before it ever reaches the commit step.

## ctx-gate policy

The commit step invokes exactly `ctx-gate run -- git commit -m <message>` — no action name, no
extra flags. Routing `git commit` through approval is a property of the ctx-gate policy
configured for this workspace/session, not of the trait; the trait only wraps the command.

**Owner fill-in:** this repo's ctx-gate policy file/schema was not reachable from inside this run
(the installed `ctx-gate` CLI exposes no `policy`/`config` subcommand to inspect or author a
policy from, and no policy file lives in this tree). Substitute a concrete policy example here
that routes `git commit` (or this session's actions generally) through owner approval, sourced
from ctx-gate's own docs or `ctx-gate guide` at demo time.

## The approval window

The step declares its own `timeout-ms` (8h — see `.ctx/traits/packages/implement/generated/guarded/index.toml`,
`shipping-commit` step) so the command itself is never killed mid-decision. The live silence
bound is this machine's `command-idle-seconds` runtime override (600s / 10 minutes, per owner
config) — ctx-gate prints its checkpoint card once and then polls silently with no give-up of its
own, so ten minutes of *no output* after the card is what the idle bound is actually watching,
not ten minutes to decide.

## Approve / deny

The checkpoint card ctx-gate prints in the step's live output names the checkpoint id and the
exact commands to resolve it:

```
ctx-gate checkpoint allow <id>
ctx-gate checkpoint deny <id>
```

Approve → the command exits 0, the commit lands, and the run completes with the commit hash in
its receipt. Deny → the command exits 1 (owner-verified), the step fails, and the run parks with
ctx-gate's own output as the park evidence.

## Out of scope for this demo

Dry-run executes nothing, so it can never trigger an approval request. Distinguishing a denied
exit from any other nonzero exit, heartbeats, `--json`, and the 0087/0091 ask/park-resume bridges
are all explicitly out of scope for `implement:guarded` (task 0098) — a denial today is just a
parked run, not a first-class recorded decision.
