# Live-run missing-input modal

## Why

Interactive runs previously returned immediately when a required port placed the session in `Status::AwaitingInput`. The live TUI now collects that value without ending the run, while unattended drives retain the existing fail-fast behavior.

## Implementation

- `modules/cli/src/app/drive.rs` selects the next unresolved port, builds a prompt from best-effort trait metadata, and opens it only when a live `RunPanel` exists.
- Submitted text is persisted through `ctx_traits_io::run::set` using `CallerProvenance::cli()` and guarded input evidence, then the drive loop continues so session status is re-derived.
- Cancellation, panel loss, missing prompt data, and headless execution return the existing `awaiting-input` result rather than waiting indefinitely.
- Rejected values are reported with `panel.note(...)` and the next loop iteration reopens the prompt.
- `modules/cli/src/app/run_view.rs` tracks the pending reply channel, renders a `Modal::TextInput`, and resolves submit and cancel outcomes without blocking the panel input thread.
- Six helper tests cover input selection, metadata fallback, and prompt formatting. Two panel tests cover modal submission and cancellation.

## Validation

Implementation evidence reports `cargo build --workspace --lib`, six focused `awaiting_input_prompt_tests`, two focused `request_input` tests, and the complete `just test` gate passing without warnings or errors.

## Reviewer Evidence

The reviewer independently inspected the change and reported that the diff is confined to `modules/cli/src/app/drive.rs` and `modules/cli/src/app/run_view.rs`. The review confirmed interactive-panel gating, fail-fast headless behavior, channel resolution on submit, cancel, and panel loss, lock release before the drive waits, string coercion matching the CLI default, and reuse of the established evidence-label pattern. The reviewer reran `CARGO_TARGET_DIR=target just test` plus the focused tests successfully and approved with no blockers.

Non-blocking review notes remain: validation errors appear as a panel note before re-prompting rather than inside the modal; the awaiting-input status is presented through `panel.note(...)`; modal submissions are JSON strings, so non-string schemas require the existing CLI `--value-json` path; and interactive TUI answer, invalid-answer, and cancellation checks remain owner-run. No `just test-full` execution is evidenced in the supplied validation record.