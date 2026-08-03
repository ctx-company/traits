# 0058 — Command-step timeouts should watch for silence, and live in project config

**Status:** designed with the owner 2026-08-03, ready to implement · **Raised:** 2026-08-03

## Why

A command/check step gets one fixed wall-clock ceiling, declared in the TRAIT
(`timeout-ms` on the step, read as `command.timeout_ms`). That number has been retuned three
times — undeclared (120s default) killed every gate and parked approved runs; 30 min proved too
short once main grew and two dispatches ran at once, parking three runs at round 1; now 90 min.
It will drift again, because how long a gate takes depends on the project, the machine and the
repo's size — none of which the trait knows. It is also wrong by construction for a portable
trait: the same recipe runs against a TypeScript project and a Rust workspace.

What generalises is not duration but LIVENESS: a command still printing output is working,
however long it takes; a command silent for a long stretch is stuck.

## Shape

Two optional keys in the repo's `.ctx/traits/runtime.toml`, flattened into `[run]` beside the
existing budget keys, mirroring the `[merge] gate-seconds` precedent (which already does
repo-configured per-command ceilings with `DEFAULT_MERGE_GATE_SECONDS` as its fallback):

```toml
[run]
command-idle-seconds = 600     # kill only after this long with no output
command-seconds = 14400        # absolute backstop, however chatty the command is
```

Both optional; built-in defaults apply when absent, so a repo that configures nothing still
behaves sanely. Naming decision left open with the owner: `command-*` matches the model's own
term for these steps, `gate-*` reads better but is inaccurate for the non-gate command steps
(the git commit tail, `capture-diff`).

Owner-agreed behaviour: the trait's own `timeoutMs` is DROPPED from
`implement`'s `repo-gates` step — the recipe says what proves the work done, the project says how
long that may take.

## Implementation notes

- `modules/io/src/command.rs::run_raw` — the wait loop currently only compares
  `started.elapsed()` against one timeout. The stdout/stderr reader threads must record a
  last-output timestamp (shared atomic) so the loop can kill on silence.
- `RunRequest` gains an idle field; every construction site
  (`worktree.rs`, `git_process.rs`, `publish.rs`, `run.rs`, tests) needs updating.
- `harness_config.rs` — add the keys to the run section, expose them on `EffectiveRunPolicy`,
  and give them `doctor --config` provenance rows like every other key.
- `run.rs`'s command path resolves the policy (`resolve_runtime_config`) and passes both bounds
  into the request.
- Distinguish the two kills in the recorded result: "silent for N" and "exceeded N" are
  different repo conditions, and the reviewer doctrine should not read either as the worker's
  defect.

## Watch

- **Silence is not always death.** Cargo prints `Compiling <crate>` and can then be quiet for
  minutes on one large crate or a long link; the same is true of a slow `tsc` or bundler. The
  idle default must exceed the quietest legitimate stretch — start at 600s, not 300.
- Keep the backstop: idle alone lets a command that prints forever (a retry loop, an accidental
  watch mode) run without end.
- The related terminal-suspension bug is FIXED (`ea6d4c37`): command steps now run in their own
  session, so a frozen-by-the-terminal gate can no longer masquerade as a slow one. That fix is
  what makes idle detection trustworthy — before it, a suspended gate was silent but not stuck.
- Task 0056 (cheap per-round gate, full suite at merge) and 0057 (warm build cache) are the
  other two halves of "gates are slow"; this task only fixes how they are BOUNDED.

## Done when

A gate that prints output for 40 minutes is never killed for taking long; a gate that goes quiet
is killed within the configured idle window and recorded as a repo condition rather than a work
defect; both bounds are set in the repo's config with `doctor --config` provenance; and
`implement`'s trait no longer declares a `timeout-ms` for its gate step.
