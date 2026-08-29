# ctx-traits-desktop

The standalone gpui desktop shell (0256.1). This project lives outside the
root Cargo workspace on purpose: `gpui` resolves ~700 crates, and none of
that cost may reach `just test`, `just test-full`, or a CI lane that
otherwise builds and gates the workspace. `desktop/` has its own
`[workspace]` table, its own committed `Cargo.lock`, and is listed in the
root `Cargo.toml`'s `exclude`, so no cargo command run from the repo root
ever resolves or builds it.

## Commands

Run these from the repo root via `just`, which sets `CARGO_TARGET_DIR`
explicitly so a desktop build never lands in the workspace gates' shared
cache slot:

```
just desktop-build   # cargo build
just desktop-test    # cargo test
just desktop-lint    # cargo fmt --check && cargo clippy -D warnings
just desktop-run     # cargo run — opens the window
```

Equivalent plain-cargo commands work from inside `desktop/` as long as
`CARGO_TARGET_DIR` is not inherited from a workspace gate run; prefer the
`just` recipes to avoid cache cross-contamination.

## Prerequisites (macOS)

`gpui`'s build script needs full Xcode, not just the Xcode Command Line
Tools: it runs `bindgen` (needs libclang), `cbindgen`, and compiles Metal
shaders via `xcrun metal`. Verify with:

```
xcode-select -p        # -> /Applications/Xcode.app/Contents/Developer
xcrun --find metal      # must resolve
```

## Prerequisites (Linux, unvalidated by this project)

`gpui` on Linux needs X11/Wayland/xkbcommon development packages
(`libxkbcommon-dev`, `libx11-dev`, `libxcb-*-dev`, `libwayland-dev`, or your
distribution's equivalents). This has not been built or run on Linux as
part of 0256.1 — treat it as unverified until a later task validates it.

## Toolchain

No second `rust-toolchain.toml`: rustup walks up from `desktop/` and finds
the root pin (1.97.1), so the desktop lane always builds with the same
compiler as the rest of the repo.

## Center link (0256.2)

On startup the shell connects to the machine-local run center through
`ctx_traits_io::center::subscribe_existing` and receives one coherent
`SnapshotStart → SnapshotRow* → SnapshotEnd` snapshot on a background
thread, forwarded to the gpui UI thread over an `async-channel`. It speaks
the same version-scoped socket as the CLI (`/tmp/ctx-{uid}-{version}.sock`,
scoped by `ctx-traits-io`'s own `CARGO_PKG_VERSION`), so a center started
from a different installed `ctx` version is a different socket and the
desktop reports it unavailable rather than connecting to it.

The desktop **never launches a center** — `subscribe_existing` only connects
to one that is already serving, fails fast otherwise, and never retries.
Absent-center startup posture, mid-stream disconnect, and reconnection are
0256.5's; forwarding later deltas after the initial snapshot is 0256.4's (see
below).

For `just desktop-run` to show anything, start a matching-version center
first, e.g. from the repo root:

```
cargo run --bin ctx -- traits internal stats --json
```

then run `just desktop-run` within that center's idle window (300s by
default).

## Run rows (0256.3)

`src/run_row.rs` projects the raw `CenterPublicRow` snapshot into a
presentation-ready `RunRow` list. Each row carries the center-supplied
`repo_key`/`repo_path` as its repository identity, and `ledger_path` (the
center's own primary key) as the row's identity for later selection and
delta application — never `run_id`/`session_id`, which collide across an
unreadable row and across same-named runs in different repositories.

A row shows: a repository label (last path segment of `repo_path`, falling
back to the raw `repo_key` when the center could not resolve a path — this
is a real case, not hypothetical), a title (falling back through
`task_key` → `trait_id` → `run_id` → `session_id`), the run id and trait,
a state (`live`, a resumable/terminal `SessionState`, or `unreadable`), a
detail line (the current sequence title, or the parse error for an
unreadable row), elapsed time, and a compact token count.

`RepoScope` (`All` or `Repo(repo_key)`) is a client-side filter over the
machine-wide snapshot, driven only by `repo_key` values that already
arrived over the wire. It is deliberately never derived from the desktop
process's own working directory — there is no "show my repo" inference
here, only an explicit scope an owner would set. `Shell::set_scope` makes
the scoped view exercisable and testable; no scope control exists in the
window yet; adding one is a later task.

The list is a plain `div().id("run-list").flex_col().overflow_y_scroll()`
column, one row per `RunRow`. As of 0256.4 the render call reads a cached
projection off `Dashboard` rather than re-projecting a raw `Vec` — see below.
The unvirtualized row list is still accepted at this walking-skeleton's
scale; `uniform_list` is the option for virtualization if the list ever
needs it.

`elapsed_text` and `tokens_text` intentionally mirror
`ctx_traits_cli::app::tui::elapsed_text` and
`app::dashboard::dashboard_tokens_text_from_summary` byte-for-byte in
output shape, but are not shared code with the CLI: those functions are
`pub(crate)` inside `ctx-traits-cli`, which the desktop must not depend on
(no CLI-private ratatui presentation as a desktop dependency). See the doc
comment atop `run_row.rs`.

Manual smoke test: with a matching-version center running (see above),
`just desktop-run` should open a window listing every run the center knows
about, grouped implicitly by the order above (live first, then newest
ledger modification, ties broken by `ledger_path`).

## Live deltas (0256.4)

Past the initial snapshot, the background link thread keeps forwarding
`CenterDelta` events off the same blocking `recv()` loop, over the same
single unbounded `async-channel`, to the same single `cx.spawn` consumer in
`Shell`. `SnapshotAssembler::accept` now returns a `Vec<LinkUpdate>` (usually
empty or one element) instead of `Option<Vec<row>>`, so a delta that races a
`SnapshotStart..SnapshotEnd` boundary is buffered and replayed immediately
after the snapshot it raced, rather than dropped or reordered. One thread
feeding one channel feeding one consumer, applied sequentially, is what
keeps subscription order intact end to end — there is deliberately no
second channel, second spawn, or coalescing/batching layer, which is the
only realistic way this ordering guarantee breaks.

`src/dashboard.rs`'s `Dashboard` is the model this stream now drives
exclusively: a `ledger_path`-keyed `HashMap<String, CenterPublicRow>`, plus a
`Vec<RunRow>` projection cache rebuilt through `run_row::project` only when a
delta actually changes the keyed row for its `ledger_path` — `ActivityLine`
never mutates the map, and neither does a no-op fold: an `Appeared`/
`RowChanged` replaying content already present, or an `Ended` for a key not
in the model. `Dashboard::apply` reports whether the *visible* projection
changed, which can be `false` even for a real model change the current
`RepoScope` still hides — that update is retained in the model and
reappears once the scope widens. `Shell::apply` installs a `Snapshot`
as a wholesale replacement (never a merge) and folds a `Delta` in only once
already `Connected` — a delta arriving before any snapshot cannot occur in a
conformant ordered stream and is dropped rather than treated as coherent
state. `Shell::set_scope` re-scopes both `Shell` and the live `Dashboard` so
the cache stays in sync.

The insert/update/remove rule itself —
`ctx_traits_io::center::CenterDelta::apply_to` — is shared with the CLI
dashboard worker (`modules/cli/src/app/dashboard/worker.rs`), which now
delegates its `apply_delta` to it. It is protocol semantics, not
presentation: only `Ended` removes a row; a completed run arrives as
`RowChanged` and is retained. Treating `Ended` as "run finished" would be a
visible correctness bug and a divergence from the TUI's own behaviour — see
the unit tests guarding it in both `modules/io/src/center.rs` and
`dashboard.rs`.

Re-projection is a full `run_row::project` (filter + sort) per changed
delta, applied in place with no clone-and-roll-back — the desktop's
projection is infallible, unlike the CLI worker's, which clones per delta to
support a renderer that can reject a projection. O(n log n) per delta with
an unvirtualized list is accepted deliberately at this scale, same as the
unvirtualized-list note above; the incremental-insert option was considered
and declined for the same one-code-path reason.

Not yet built (0256.5's): recovering from an absent center, a mid-stream
disconnect, bounded-subscriber eviction/backpressure (today's channel is
unbounded — the real backpressure is the center's own bounded subscriber
queue), resubscription, and any stale-state presentation. Row selection, run
detail, control verbs, and virtualization remain out of scope for the
dashboard entirely, for now.
