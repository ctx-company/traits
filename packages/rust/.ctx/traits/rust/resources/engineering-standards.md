# Rust engineering standards

Doctrine a Rust worker or reviewer applies to any change in a Cargo
workspace, independent of the specific crate or phase.

## Formatting and lint baseline

- Format with `cargo fmt` before every commit; `cargo fmt --check` must pass
  clean. Never hand-align code to defeat the formatter.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` must
  pass with zero warnings. A new `#[allow(...)]` is a house-rule change, not a
  routine fix — justify it in the work summary or remove the lint trigger
  instead.
- `cargo check` (or `cargo check --workspace --all-targets` where the repo
  uses a workspace) must succeed before any gate that depends on it runs.

## Module and API shape

- Keep modules single-purpose and cohesive; a `mod.rs` or `lib.rs` may host
  logic, but only logic that belongs to the module tree as a whole (shared
  types, re-exports, glue) — push logic specific to a single submodule down
  into that submodule instead of accreting it at the parent.
- Prefer `pub(crate)` over `pub` unless the item is genuinely part of the
  crate's external API — widen visibility only when a real external caller
  needs it, not defensively.
- New public functions and types carry doc comments (`///`) stating what they
  do and any non-obvious invariant a caller must uphold; skip comments that
  restate the signature.
- Favor composing existing traits/functions over introducing a new
  abstraction for a single call site — the same reuse-before-writing bar this
  repo's review doctrine applies to every language.

## Error handling

- Library code returns `Result<T, E>` with a typed, local error enum (or an
  existing shared error type already used by the crate); it does not `panic!`,
  `unwrap()`, or `expect()` on inputs that can legitimately fail at runtime.
- `unwrap()`/`expect()` are acceptable only where failure is a genuine
  programmer error (an invariant already proven earlier in the same
  function) — and `expect()` with a message that states the invariant is
  preferred over a bare `unwrap()`.
- Propagate errors with `?` rather than manual `match` boilerplate; convert
  between error types with `From`/`map_err` at the boundary, not by stringly
  losing the original cause.
- Never swallow an `Err` silently (`let _ = fallible_call()`) unless the
  call site comments why the failure is genuinely inconsequential.

## Testing

- New behavior gets a unit test colocated in the same module (`#[cfg(test)]
mod tests`) unless the repo's own test-authoring rules say otherwise for
  that crate — check the crate's existing test layout before adding a new
  pattern.
- Tests assert observable behavior (return values, resulting state) rather
  than implementation details that would break on a harmless refactor.
- A bug fix ships with a regression test that fails against the pre-fix code
  and passes after, when the crate already has a test harness in place.

## Dependencies

- Adding a new crate dependency is a house-rule-level decision: prefer the
  standard library or an existing workspace dependency first, and state the
  justification in the work summary when a new dependency is genuinely
  required.
- Pin dependency versions the way the workspace already does (`Cargo.toml`
  convention in this repo) — do not introduce a different versioning style
  for one crate.
