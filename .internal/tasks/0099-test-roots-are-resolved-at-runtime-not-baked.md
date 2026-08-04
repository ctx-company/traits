# 0099 — Test roots are resolved at runtime, never baked at compile time

**Status:** ready to implement · **Depends on:** nothing · **Raised:** 2026-08-04 (owner call after
this failure class killed two runs' gates in one day; decisions in this file are the contract —
they were settled deliberately, do not re-open them without a concrete contradiction)

Gate builds share build-slot target dirs (`CARGO_TARGET_DIR=…/build-slots/slot-N`) across run
worktrees. Any test code that bakes a filesystem path at compile time — `env!("CARGO_MANIFEST_DIR")`
— can therefore execute as a cached binary whose baked path names a *different worktree than the one
under test*, including one that no longer exists. This is not hypothetical; it happened twice on
2026-08-04:

- run-4de40dda (0082) parked three times because slot-3's cached `proof_branch_literals` binary
  carried `repo_root()` = the pruned 0085 worktree (`wt-e8af0ba4971f`): the source walk found 0
  `.rs` files and the allowlist check ENOENT'd, failing `just test-full` deterministically on every
  merge retry. Verified at the artifact level: the dead path is baked into
  `libsupport-*.rlib` and the test binary the gate ran.
- run-414acdf0 (0078) went red the same way one layer over: its per-round gate's sdk-check ran a
  stale shared-slot `ctx` binary that predated the new config schema.

The house already knows the rule: `ctx_bin()` in `modules/cli/tests/support/src/lib.rs` reads
`CARGO_BIN_EXE_ctx` **at runtime** and its comment explains precisely why (P477). `repo_root()`,
directly below it, violates the same rule with `env!`.

## Decisions

- **Runtime derivation only.** Every path a test resolves must come from the running process's own
  environment or working directory — `std::env::var("CARGO_MANIFEST_DIR")` (cargo sets it as a real
  process env var for test binaries), the test binary's own location, or cwd. Never `env!`/
  `option_env!` for anything that names a directory.
- **Mind whose manifest it is.** The runtime `CARGO_MANIFEST_DIR` for an integration-test binary is
  the manifest dir of the package under test (e.g. `modules/cli`), not of a helper crate like
  `support` — the parent-hop arithmetic changes when switching from the baked form. Verify each
  derived root by probing for a landmark (e.g. `Cargo.toml` + `modules/`) rather than trusting hop
  counts blindly.
- **Sweep the class, not the instance.** All four current bakes go:
  - `modules/cli/tests/support/src/lib.rs:38` (`repo_root()` — today's kill),
  - `modules/wasm-core/tests/native_wasm_parity.rs:40`,
  - `modules/core/src/model_view/compile.rs:1328` (test code),
  - `modules/io/src/confinement.rs:1458` (test code).
- **Guard the invariant the house way.** Extend the grep-gate pattern (`proof_branch_literals` is
  the precedent) with a small proof that fails on any new `env!("CARGO_MANIFEST_DIR")` in
  `modules/**` outside an explicit, justified allowlist — so the class stays dead instead of
  regrowing one helper at a time.

## Watch

- The proof guarding the invariant must itself resolve its root at runtime, or it re-imports the
  disease it polices.
- `CARGO_MANIFEST_DIR` as a runtime var is set by `cargo test`/`cargo run` but NOT for a binary
  executed directly outside cargo; helpers that also serve non-cargo contexts need the
  landmark-probe fallback, not an `expect`.
- Do not "fix" this by cleaning build slots in the gate — that treats the symptom, costs a full
  rebuild per gate, and leaves every other baked path waiting.

## Done when

No `env!("CARGO_MANIFEST_DIR")` remains in the four listed sites; each derives its root at runtime
and verifies it by landmark; a proof fails on new compile-time path bakes outside its allowlist;
`just test-full` passes when the test binaries were compiled under a *different* worktree's checkout
of identical sources (the shared-slot scenario that killed run-4de40dda); and the full per-round
gate is green.
