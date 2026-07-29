# Rust gate conventions

The validation gates a Rust change in this repo must pass, and how a worker
or reviewer runs and reads them. These are the same gates this repo's own
Rust core is held to.

## The three gates, in order

1. `cargo fmt --check` — formatting only; run `cargo fmt` (without
   `--check`) to fix, then re-run `--check` to confirm.
2. `cargo check` — the change compiles. Run this before clippy: a
   compile error will otherwise surface as a confusing clippy failure.
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings` —
   lint-clean across every target (bins, tests, examples, benches) with every
   declared feature enabled simultaneously. This does not check every
   individual feature combination in isolation, only the all-features-on
   build; `-D warnings` means any clippy warning in that build is a hard
   failure, not advisory.

Run all three from the workspace root so `--workspace` picks up every crate
the change touches, including ones the diff did not directly edit but whose
public API it affects.

## Reading a failure

- A `cargo fmt --check` diff names the exact files reformatted; apply
  `cargo fmt` and re-check rather than hand-editing whitespace to match.
- A `cargo check` error names the crate and file; fix compile errors before
  attempting clippy — do not try to interpret clippy output against code
  that does not yet build.
- A `cargo clippy` warning names the lint (e.g. `clippy::needless_clone`);
  fix the underlying pattern the lint flags rather than suppressing it with
  `#[allow(...)]`, unless the lint is a genuine false positive for this call
  site — in which case scope the `#[allow]` to the single item, not the
  module, and state why in the work summary.

## Reporting gate results

A work summary or review verdict that claims these gates pass must have
actually run them against the current working tree in this same pass — never
report a gate as passing from memory of an earlier run, and never approve a
review verdict when the summary does not show gate output for a change that
touches Rust source.

## Scope

These three commands are the whole-workspace gate; a phase's own Definition
of Done may narrow the gate to a single crate (e.g. `cargo clippy -p
<crate>`) when the phase is scoped that way — follow the phase contract over
this default when the two conflict, and say in the work summary which scope
was run.
