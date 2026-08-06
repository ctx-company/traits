# Unstick session-title generation

## What was built

- The drive-end flush now closes an unanswered title claim through the existing failure ladder, producing `Retryable` or `Terminal` instead of leaving permanent `InFlight` state.
- Attach observers derive `title_generation_live` from the existing driver-lock probe and refresh it with the ledger.
- `title_row_line` shows “(Generating session title…)” only while a driver is live. Dead-driver `None`, `InFlight`, and `Retryable` states render the blank-title form; existing stale ledgers therefore require no migration.
- Narrator dispatch, prompts, retry policy, and frame-boundary persistence were not changed. No `run_session.rs` API addition was required.

## Implementation evidence

Current-source inspection confirmed the final-flush close-out in `modules/cli/src/app/drive.rs`, shared liveness derivation in `modules/cli/src/app/dashboard.rs`, observer state propagation in `modules/cli/src/app/run_view.rs`, and display gating plus behavioral tests in `modules/cli/src/app/run_view/render.rs`. Working-tree status and diff were not independently collected because this step explicitly prohibited git commands.

## Why

Interrupted or completed drives could leave title claims in `InFlight` or `Retryable` indefinitely. The UI treated those orphaned states as active generation even though no process could advance them. The change prevents new orphans during orderly drive shutdown and presents existing stale claims truthfully without reader-side ledger writes.

## Validation

Implementation-reported validation passed `cargo build --workspace`, `cargo fmt --check`, workspace-wide clippy with warnings denied, 452 CLI library tests with one ignored, and all non-CLI workspace tests. Three CLI integration proof files still fail with the existing CDK/Node `TypeError: input.prompt is not a function`; the implementation reported an identical clean-baseline failure. New tests cover retryable and terminal close-out, successful/no-claim no-ops, a missing driver lock, and live-versus-dead rendering for every unresolved title state.

## Review evidence

The reviewer approved the implementation. Review confirmed the close-out uses claim-owner checks, the reader path reuses the existing liveness contract, and the display policy resolves dead `Retryable` state to blank. The reviewer accepted the integration failures as unrelated from their shared signature and the untouched harness boundary, but did not independently rerun the baseline comparison.

Reviewer advisories: the baseline comparison used `git stash` despite the repository rule against shared stash state; the reviewer found only an unrelated one-line `projection.rs` stash entry and no residue from this work. The blank rendering for dead `Retryable` state follows the owner-approved plan’s default, although it received no separate owner decision. A late worker success can still arrive after the final pending-value check and be superseded by close-out; this is no worse than the previous process-exit behavior and remains within the approved scope.

## Owner evidence

The owner approved the plan unchanged through Plannotator with no annotations. The adjacent one-shot retry-policy gap and persistent migration of old ledgers remain intentionally out of scope.