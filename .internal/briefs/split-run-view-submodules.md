# Split `run_view` into cohesive submodules

## Implementation Output

Refactored `modules/cli/src/app/run_view.rs` into six cohesive submodules: `guide`, `model`, `planned`, `projection`, `render`, and `session_text`. The root retains the lock-coupled `RunPanel`, panel state, cadence, handoff, tick, and key-handling logic. Crate-facing `run_view::` paths remain available through root re-exports, avoiding consumer changes.

Redistributed all 101 existing tests with their primary subjects: 37 remain in the root, 8 moved to `guide.rs`, 12 to `projection.rs`, and 44 to `render.rs`. Direct workspace inspection confirmed the module structure, re-export boundary, and 37/8/12/44 test distribution.

## Rationale

The split reduces the largest CLI presentation module into focused boundaries without changing ledger, driver, report, or terminal behavior. Lock-coupled state mutation stays in the root, while pure view models, projection, planned-tree logic, rendering, guide chat, and session text are isolated.

## Validation

Implementation validation reported:

- `CARGO_TARGET_DIR=target cargo build -p ctx-traits-cli --lib --tests`: clean with no warnings.
- `CARGO_TARGET_DIR=target cargo test --workspace --lib`: 266 passed, 0 failed.
- `just test`: all formatting, clippy, SDK, TypeScript, and workspace library gates passed.
- Test motion proof: exactly 101 tests remain.
- Run-view integration proofs: 18/18 passed across `proof_drive_persistent_contract`, `proof_drive_retries`, `proof_drive_role_budget`, and `proof_run_startup_progress`.
- `just test-full` reached an unrelated, pre-existing `proof_cdk_native_family_build` Node/CDK fixture failure: `input.prompt is not a function`, with spawned Node v25.8.1 versus the shell's v24.6.0.

## Reviewer Evidence

The reviewer approved with no blockers and independently confirmed that the final working diff was confined to Rust test code under `modules/cli/src/app/run_view*`, excluding the failing CDK fixture. The reviewer also independently reran the four relevant proof binaries successfully, 18/18.

Reviewer advisories remain: the owner must resolve or explicitly waive the environment-sensitive CDK failure before satisfying the declared `test-full` merge gate; the final test redistribution should be committed separately after verifying HEAD; and the unrelated pre-existing shared stash should be triaged outside this change. The owner approved the completed build loop.
