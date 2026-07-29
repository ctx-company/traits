# 0038 — A failing gate must say WHY, and its argv must have an owner

**Status:** DONE for v1 — part 1 LANDED 2026-07-29 (verdict record carries exit-code, timed-out, and a failure tail; three call sites converge on one CheckEvidence type; reviewer/worker prompts read the tail); part 2 DEFERRED post-v1 by owner ruling 2026-07-29 · **Raised:** 2026-07-29

## What happened

`run-3201b6ff` (ctx-gate, DEMO-CRITICAL-4) burned six rounds without moving. The
`repo-gates-passed` digest was **byte-identical every round** — `c2b420dd…`,
`{"argv":["just","test"],"ok":false}` — because:

```
$ just test        # in ctx-gate
error: Justfile does not contain recipe `test`
```

ctx-gate has no `test` recipe. The argv lives in the trait
(`.ctx/traits/packages/implement/generated/quick/index.toml:307-308`) and is
executed by ctx, not by the agent, so **no worker could ever have fixed it.**
The reviewer raised `required-validation-gate-failed` in all five verdicts, and
by round 5 it had invented a cause:

> "The supplied repository gate ran the explicitly prohibited `just test`
> command and reports ok=false."

It blamed the worker for a command the worker never chose. Meanwhile
`implement:quick` runs `rounds: 10, onExhausted: "continue"` — four more rounds
and it would have committed unapproved work with both blockers standing.

This is the *second* instance of one class. `check_output_value` grew its `argv`
field precisely because "documentation and the declared check disagree, every
round re-validates the wrong thing and the loop cannot converge"
(`modules/core/src/procedure/session.rs:2400-2404`). That fix made the command
visible. It did not make the **failure** visible, so the reviewer filled the gap
with a guess.

## Part 1 — carry the reason (cheap, ships alone)

`check_output_value` (`session.rs:2410`) emits exactly two fields:

```json
{ "ok": false, "argv": ["just", "test"] }
```

Add the one field that would have ended this in round 1:

```json
{ "ok": false, "argv": ["just", "test"], "exit-code": 1,
  "tail": "error: Justfile does not contain recipe `test`" }
```

Output is already captured — `COMMAND_CAPTURE_LIMIT = 327_680`
(`modules/io/src/run.rs:37`) — and simply never reaches the slot. A short tail
(last ~2 KB, stderr preferred) is enough; the frame budget cannot take the full
capture, and the ledger already holds it for anyone who wants more.

Three call sites must agree, as before: `modules/io/src/run.rs:1983` (submits),
`session.rs:2374` (accepts), `ledger_contract.rs:1348` (replays). The slot schema
in `.ctx/traits/packages/implement/source/sequence/family.ts` gains the two
fields, and both should be **optional** so a gate that could not be spawned at
all still produces a decodable value.

Then say so in the produce prompt: a gate that failed because the command does
not exist is a **misconfigured repository**, not a worker defect, and the
reviewer must report it as such rather than as a blocker against the work.

## Part 2 — DESIGN: `config()` in the CDK

**Owner ruling 2026-07-29: deferred post-v1.** v1 ships the convention — the
gate is `just test`, and the dispatch preflight (0041) refuses at dispatch with
"add a `test:` recipe" when it is absent. Two facts recorded for the pickup:
`argvFrom: PortHandle<string[]>` already exists in the CDK, so a configurable
gate may need no new primitive — but an `argvFrom` gate step is invisible to
the static preflight, so configurability and dispatch-time refusal are in
tension and the choice must be made deliberately, with real user feedback
rather than guessed now.

The gate argv is baked into a package meant to be installed elsewhere. Every
consuming repo must own a `test` recipe meaning "my gate", or edit and rebuild
the trait — which moves its digest and drops its trust. That is 0012's problem
with a different value in it.

**It is not a port.** The distinguishing test is *does it vary per run?*

| value | varies per run? | primitive |
|---|---|---|
| `phase=P535` | yes, every run | **port** |
| gate argv | never, within a project | **config** |
| `.plans/EXECUTION_PLAN.md` | never, within a project | **config** |

Proposed shape — the trait author declares a setting with a default:

```ts
const repoGate = config.argv("repo-gate", {
  default: ["just", "test"],
  description: "The command whose exit status decides repository done-ness.",
});

export const repoGatesStep = sequence.check("repo-gates", {
  argv: repoGate,
  output: repoGatesPassed,
});
```

and the installing project answers it once, in the committed tier (0037):

```toml
# .ctx/traits/config.toml
[trait.implement.config]
repo-gate = ["just", "verify"]
```

### This forces 0010's decision

Gate argv is the first value where 0010's `default` / `requirement` / `additive`
split has teeth. It must be a **requirement**: `config.toml` (committed, the
project's decision) may set it; machine-local `runtime.toml` must **not**, or a
person can silently green their own gate on their own machine and nothing in the
receipt would show it. Precedence "nearest layer wins" is actively wrong here,
which is exactly the case 0010 was raised to settle.

### Relationship to 0012

0012 proposed `[defaults.port] execution-plan = "docs/roadmap.md"` — shape agreed
2026-07-28. Under the table above that value never varies per run, so it is
config wearing a port's clothes. **Decide whether `config()` subsumes
port-defaults** (0012 folds into this, one primitive) **or whether both exist**
with a stated rule for choosing. Do not ship both without the rule; two ways to
default the same kind of value is the ambiguity 0037 avoided by refusing to
partition keys by kind.

## Watch

- **Pinning stays the default; `config()` is the only opt-out** (owner ruling
  2026-07-29). Every value in a package remains covered by its digest exactly as
  today. Declaring `config()` is what makes one value overridable, and the
  declaration itself — id, type, **default**, description — is source, so it is
  inside the digest too.

  The useful consequence: the digest pins the default *and the exact,
  enumerable set of knobs a project may turn*. Approving a package is approving
  that list, and a package cannot grow a new knob without moving its digest and
  asking again. What the digest no longer pins is only the effective value of an
  already-declared setting — and Part 1's `argv` field carries that into the
  receipt, so extend it with the winning layer rather than adding a second
  record. State the rule this way round in the product doc: not "the digest
  stopped pinning the gate", but "these three values are configurable, and that
  fact is itself pinned."
- **Config is not a general escape hatch.** Only values a *consumer* legitimately
  owns become settings. A trait's own rubric, its seat contracts, and its output
  schemas are not consumer property — same boundary 0012 draws for resources.
- **A configured argv is arbitrary command execution** from a committed file. It
  already is (`just test` runs whatever the Justfile says), but making it
  configurable widens who can change it without moving a digest. Worth one
  explicit sentence in the product doc rather than discovering the question later.
- Part 1 is worth landing **on its own**, before any of Part 2 is decided. It
  converts a silent six-round deadlock into a first-round diagnosis, and nothing
  in it depends on where the argv comes from.

## Done when

A check step that fails reports its exit code and enough output to diagnose it,
and a missing command reads as a misconfigured repo rather than a worker defect;
a trait author can declare a project-owned setting with a default; a project sets
it once in `config.toml` without editing or rebuilding the package; a
machine-local tier cannot weaken it; every value NOT declared through `config()`
stays digest-pinned exactly as today; and the effective value appears in the
ledger with the layer that supplied it.

## Interim

ctx-gate got a `test` recipe on 2026-07-29 (`cargo fmt --check`, `cargo check`,
`git diff --check` — phase-independent, no test targets, since the DEMO-CRITICAL
phases forbid them). That unblocks the loop; it does not remove the coupling.
