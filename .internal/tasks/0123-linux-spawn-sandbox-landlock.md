# 0123 — Linux spawn sandbox: landlock via `pre_exec`, bubblewrap fallback

**Status:** ready to implement (needs a Linux machine in the loop — cannot be authored and self-verified on Darwin) · **Raised:** 2026-08-05 (0032 migration from the retired `.plans/` board — was P480a; verified still absent 2026-08-04: `generate_spawn_sandbox` in `modules/io/src/confinement.rs` is sandbox-exec-only)

P480 shipped the macOS half of the OS-level spawn sandbox (`sandbox-exec` SBPL
profile generated per worktree, applied at both spawn seams). Linux runs today get
a loud `spawn_sandbox_unsupported_capability` report instead of enforcement —
honest, but a gap.

In `modules/io/src/confinement.rs`, parallel to `render_macos_sandbox_profile` /
`SpawnSandbox`: a `landlock` crate 0.4.5 (kernel-maintainer-published) ruleset
built from the same `ConfinementPlan.additional_directories`, applied via
`std::os::unix::process::CommandExt::pre_exec` before `exec`, requiring ABI ≥ 2
(rename/link governed) so the sandbox doesn't silently under-enforce on an older
kernel. On ABI < 2 or missing landlock syscall (older kernel, container without
the feature): downgrade to a bubblewrap (`bwrap`) argv-prefix fallback mirroring
`sandbox-exec`'s delivery, or to the named capability report if neither is
available — never silent.

## Watch

- `pre_exec` runs in the forked child between `fork` and `exec` —
  async-signal-safety rules apply; keep it to the landlock syscalls only.
- The `landlock` dependency needs adding to `modules/io/Cargo.toml` (deliberately
  deferred out of P480).
- Network stays open in this profile (exfiltration is a separate decision — do not
  claim otherwise); the mutation tripwire (P479, landed) stays active under the
  sandbox.

## Done when

On a Linux host with landlock ABI ≥ 2, the same live-boundary proof P480's macOS
test performs (worktree write succeeds, out-of-worktree write fails, carve-out
write succeeds) passes under `pre_exec`; on an unsupported kernel/container the
bubblewrap fallback or the named capability report fires — never a silent
unconfined spawn.

Full original contract: `archived/board/execution-plan.md` (Group 115, P480a).
