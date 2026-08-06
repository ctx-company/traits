# Brief — 0065: authoring assist writes CDK source, not canonical (slice 1: IO + layout)

## What was built

The first landable slice of task 0065 (`.internal/tasks/0065-authoring-assist-writes-cdk-source-not-canonical.toml`): the safe writer's authoring-source mode and its layout helper. The diff is confined to two files, purely additive, with no legacy call sites touched:

- `modules/io/src/write.rs` — three new `CandidateWriteMode` variants (`NewCandidateSource`, `RefineApplySource`, `ManagedImportSource`) mirroring the legacy canonical-targeting modes; two private helpers on the enum (`targets_authoring_source`, `leaf_semantics`) so leaf overwrite rules (new-must-not-exist, refine-must-exist) are shared rather than duplicated; `validate_candidate_path` gains a second accepted target shape, `<traits-root>/<package>/source/index.ts`, via a new `canonical_source_package_id` that mirrors the existing `canonical_generated_package_id` guards (component normalization, single-`.ctx` anchoring, exact five-segment tail); a `Debug` derive on `CandidateWriteResult`; and a three-test `authoring_source_write_tests` module (source write lands under the traits root; a source-mode write aimed at a `generated/index.toml` path is rejected; `RefineApplySource` requires an existing file and reports `overwritten`).
- `modules/io/src/layout/mod.rs` — `package_source_write_path(package_root) -> <root>/source/index.ts`, the assist-facing sibling of `package_manifest_write_path`.

## Why

Today every assist path (`generate`, `refine --apply`, `import --llm-assisted`) writes the build output `generated/index.toml` directly, so the next `ctx traits build` clobbers assist edits and "build is the only writer of canonical" cannot hold. The safe writer hard-required the generated shape, so no assist path *could* target source — this slice gives the writer that capability, unblocking the handler rewrites (slices 4–6) without changing any current behavior.

## What was not done, and why

Slices 2–7 of the approved plan (trait-spec `cdk-authoring.md` resource, the three meta-trait schema/prompt flips, the generate/refine/import handler rewrites, candidate-replay TS handling) were not attempted. The reviewer verified the reason: `.ctx/node_modules/@ctx-traits/cdk` is absent in this environment and the package 404s on the public registry, so the landing recipe and every remaining Done-when check are unrunnable here. Stopping at the self-contained, gate-green IO/layout slice was accepted as reasonable divergence in the verdict's forgiveness-reason.

## Validation (implementation-run)

- `cargo fmt --check` at workspace root and `-p ctx-traits-io`: exit 0, zero `Diff in` lines — this closes the prior round's sole blocker (`rustfmt-gate-authoring-tests`, a 3-line vs 2-line wrap of the `scratch_dir` test helper at write.rs:835-838).
- `just test`: full workspace suite green (`EXIT:0`), including all three `authoring_source_write_tests`. First attempt hit a known cross-worktree shared-build-slot cache contention (`SIGKILL`/rmeta mismatch on `winnow`/`serde_json`), unrelated to the change; the rerun completed clean.

## Review findings (reviewer-run evidence, per the final verdict)

Status: **approved**, blockers empty, escalation none. The reviewer established every finding with its own commands (`cargo fmt --check`, `cargo test -p ctx-traits-io --lib authoring_source_write_tests` → 3 passed, `git status`/`diff --stat`, grep of `NewCandidate` call sites, `ls .ctx`) rather than trusting the work summary, and reported the summary matched on every point checked. Carried non-blocking notes for follow-up:

1. The doc comment on `CandidateWriteMode::NewCandidate` (write.rs:191, now "(compose, refine --out)") describes a post-slice-4 state while generate.rs:174 still uses `NewCandidate` — restore "generate" to the comment or annotate as pre-0065 until slice 4 lands.
2. Owner decisions needed before slices 2–7: provision the CDK toolchain in run worktrees (link `packages/cdk` locally; the npm package is unpublished); rule on `compose.rs:256` — a reviewer-verified fourth canonical writer via `CandidateWriteMode::NewCandidate` — as in-scope for 0065's "no assist code path writes canonical" Done-when or a deliberate exclusion; and the plan's four open annotation questions (build-gate rung shape, staging vs in-place scaffold for generate, legacy-package-without-source error for refine, deletion of the legacy generated-shaped validator arm).
3. Two untracked scratch files (`.ctx/package.json`, `.ctx/traits/runtime.example.toml`) persist; the sandbox denies the worker's `rm`, so they need an owner-side `git clean` on `.ctx/` before landing. They are outside the diff.

Pre-commit verification for this record: `git diff --stat` confirms exactly the two files above (185 insertions, 15 deletions), `cargo fmt --check` re-run exits 0, and the untracked `.ctx/` files remain outside the staged change.