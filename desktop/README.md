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
`CenterDelta` events off the same bounded-wait `recv_timeout()` loop (see
below), over the same single unbounded `async-channel`, to the same single
`cx.spawn` consumer in `Shell`. `SnapshotAssembler::accept` now returns a `Vec<LinkUpdate>` (usually
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

## Recovering from center loss (0256.5)

The link thread (`src/center_link.rs`) is a supervisor: connect, pump events,
and on any stream end — a connect failure, a mid-stream disconnect, or a
server-side backpressure eviction — announce a `LinkUpdate::Down(reason)`
and retry after a fixed 500ms backoff (parity with the CLI dashboard
worker's `wait_for_retry`), forever, until the receiver is dropped. A fresh
`SnapshotAssembler` is built per attempt, so a stream that dies mid-snapshot
can never leak staged rows or buffered deltas into the next subscription.
Repeated identical outages are de-duplicated (one `Down` per distinct
reason, cleared once a snapshot lands again) so a permanently absent center
does not repaint the window every backoff tick for the life of the process.

A center exit, a plain disconnect, and eviction are indistinguishable on
this wire by design: `modules/io/src/center.rs`'s backpressure eviction is
*defined* to end in a bare `shutdown(Both)`, exactly the wire shape of any
other stream end, and the client must not try to tell them apart — all
three recover through the same reconnect-and-resnapshot path. One
corollary of that: the link drains its socket into an *unbounded*
`async-channel`, so it can never be evicted for its own slowness (a slow
gpui UI thread would grow that channel unboundedly instead) — accepted at
this walking-skeleton's scale, not fixed here.

`src/shell.rs`'s `CenterFace` is where the link's one fact ("the
subscription is down, with a reason") becomes a presentation state. It adds
a fourth `CenterState` alongside the three 0256.1–.4 already established:

- `Connecting` / `Unavailable { reason }` — never held rows; first contact
  with no matching center serving.
- `Connected { dashboard }` — unchanged.
- `Stale { dashboard, reason, since }` — a prior `Connected` dashboard that
  lost its subscription. Rows stay visible, labelled: the header always
  reads "center unreachable — showing state as of HH:MM:SS (reason);
  retrying — N runs", never a bare "N runs", so stale data is never
  presented as current.

The two rules that made recovery possible from 0256.4 onward needed no
rework, only a fourth state to transition into: a `Snapshot` always
installs a coherent `Connected` dashboard, wholesale-replacing whatever
came before it (including a `Stale` one) — never a merge of an uncertain
old stream with a new snapshot — and a `Delta` is folded in only while
`Connected`, so a delta arriving while `Stale` changes nothing.

The desktop **never spawns a center** to recover, in any of these cases:
`center::subscribe` (spawn-on-need) resolves its own executable, which for
`ctx-desktop` would fork a second copy of the GUI, not a center. Locating a
version-matching `ctx` binary on `PATH` is launcher work, out of scope for
0256. A visibly-stale or -unavailable view plus indefinite retry is the
sanctioned alternative.

`desktop/tests/support/mod.rs` holds shared scratch/env helpers plus a
`FakePeer` — a hand-driven `UnixListener` speaking the io crate's wire
protocol — so the failure paths (`absent_center.rs`, `center_disconnect.rs`,
`subscriber_eviction.rs`, `idle_consumer_link_leak.rs`) can be reproduced
deterministically: there is no way to stop an in-process `run_server()` on
cue, and a live subscriber keeps it alive past its idle timeout.
`subscriber_eviction.rs` reproduces the wire *shape* eviction is defined to
produce (a delta backlog then a bare shutdown), not a full-stack
backpressure trip against a real bounded channel — see the file's own
comment for why that trade was made.

The pump inside `center_link.rs` waits on `CenterSubscription::recv_timeout`
on a fixed `CONSUMER_POLL_INTERVAL` (200ms) rather than a blocking `recv()`,
and re-checks whether the UI receiver has been dropped between waits. A
plain `tx.is_closed()` check made only before subscribing (the 0256.5 first
pass) misses exactly one case: the UI is gone but the subscription is
otherwise idle, with no event arriving to wake a blocking `recv()` and
notice the closed channel — that leaks the link thread, the socket, and the
server-side subscriber until the center or process exits.
`idle_consumer_link_leak.rs` reproduces that case directly: it drains a
served snapshot, drops the receiver while the peer stays silent, and asserts
the peer observes client EOF within a bounded deadline.

Control verbs and virtualization remain out of scope for the dashboard
entirely, for now. Row selection and run detail are covered next.

## Run detail (0257.1)

Clicking a compact row now seeds **detail state**: one background,
authoritative full-session read of that run's ledger, through
`ctx_traits_io::run_session::read_run_session` — the same IO store boundary
the CLI's own attach path uses — never a hand-rolled `std::fs::read_to_string`
+ `serde_json` in this crate.

**Resolution.** A selection resolves to `RunRow::ledger_path`, the center's
own primary key for a run, keyed alongside `repo_key` so two same-named runs
in different repositories stay distinct at this layer too (matching
`run_row.rs`'s `repository_separation_keeps_same_named_runs_distinct`
guarantee for the list layer). This is deliberately **not** re-derived from
`ctx_traits_io::state::global_runs_root(&row.repo_key)` plus
`run_session::session_store_path(..)`: the center's runs root is
env-overridable (`CTX_CENTER_RUNS_ROOT`), so that derivation diverges from
the row's real `ledger_path` whenever the center is not serving out of the
global runs root — including in this crate's own integration tests — and it
would re-perform, from a second source, a resolution the center already
shipped on the wire. Nothing in this slice reaches `current_repo_key()`,
`current_global_runs_root()`, or `default_session_store()` (the one call in
`run_session` that reads cwd); resolution is always center-supplied.

**State machine** (`src/detail.rs`, gpui-free — unit-testable without an
`App`, the same pattern `dashboard.rs` and `run_row.rs` document):
`RunDetail::select` returns a `LoadRequest` the caller must run on a
background executor, or `None` when no read is needed. Re-selecting the
already-current selection is a no-op in every state (`Loading`, `Loaded`,
`Failed`) — the "exactly once" rule; there is no retry-on-reclick, since a
silent second full-ledger read is exactly what this task's contract forbids.
`RunDetail::apply` ignores an outcome tagged with a superseded generation (a
stale selection's result landing late) and otherwise transitions
`Loading -> Loaded | Failed`, returning whether the visible state actually
changed — the same discipline `Dashboard::apply` established in 0256.4, so
`Shell` calls `cx.notify()` only on real change. `DetailLoad::Loaded` boxes
`DetailBaseline` (the session plus its activity sidecar read, see 0257.2
below) to keep `clippy::large_enum_variant` quiet.

`RunDetail` is a **sibling** of `CenterFace`/`Dashboard`, not a member of
either: a center reconnect installs a fresh `Dashboard` snapshot, and
`select`'s "exactly once" rule still holds across reconnects — a selection
is never re-read merely because the center reconnected. What changed in
0257.3: every `LinkUpdate` now also reaches detail via `RunDetail::follow`,
which is how a selection advances live and recovers from an outage without
polling or a second ledger read per delta — see "Live follow (0257.3)"
below.

**gpui wiring** (`src/shell.rs`) is deliberately thin: `Shell::select_ledger`
looks the row up by ledger path, calls `RunDetail::select`, and — only when
that returns a request — runs `detail::load` on gpui's **background**
executor (`cx.background_spawn`, inside a foreground `cx.spawn` that does
nothing but await it and hand the result back), so the filesystem read never
blocks the UI thread. The generation carried on the request, not `Task`
drop, is the correctness guard against a stale read overwriting a newer
selection — storing the `Task` in `Shell::detail_task` (instead of
`.detach()`) is a courtesy cancellation, not the rule. Each row's `ElementId`
is its `ledger_path`, unique by construction (it is the center's primary
key), which is exactly why it — not a shifting list index — is the click
target's identity.

**Presentation** is the native frame tree — see "Native frame tree (0257.2)"
below.

Reconstruction fixtures (`tests/detail_reconstruction.rs`) cover a live and a
finished session baseline written with `write_run_session` (the same writer
production uses), walk the real row -> selection -> load path with no
center, and prove the same baseline reappears from a freshly constructed
`RunDetail` — a simulated process restart. `tests/detail_repository_identity.rs`
points `HOME` at an empty scratch directory and the process cwd outside any
ctx repository, then proves two rows sharing a `run_id`/`session_id` but
carrying different `repo_key`s each resolve to their own ledger — it is
env-mutating and stays the only `#[test]` in its target, per the convention
`tests/support/mod.rs` documents.

A selected run disappearing from the list (an `Ended` delta) or the
subscription going `Stale` while detail is open are reconciled by
`RunDetail::follow` — see "Live follow (0257.3)" below.

## Native frame tree (0257.2)

The selected run's loaded ledger is now projected into a **hierarchical**
detail model and rendered as native gpui elements, replacing 0257.1's
`summary_line` placeholder strip. No terminal lines, no `tui::Line`, no
import of the CLI-private ratatui renderer or `ctx-traits-cli` — this crate
still has no dependency on it at all.

**Projection is gpui-free** (`src/detail_tree.rs`, unit-testable without an
`App`, the same pattern `dashboard.rs`/`run_row.rs`/`detail.rs` document).
`detail_tree::project(session, activity, live)` builds a `DetailTree` from
the session's own durable evidence — never by resolving a trait/plan, which
is repository-relative work this crate does not do:

- `session.ledger.sequence_statuses` (one entry per reached frame position,
  in execution order) is grouped by path prefix into nested `DetailNode`s: a
  loop's body items land under one synthesized iteration group per
  `#itN`, a branch arm lands under its own group inside the right
  iteration, and a top-level item's empty `position_path` is normalized to
  a synthetic single-segment path so one algorithm covers both cases.
  Grouping keys drop a segment's `index` only where it is a changing child
  cursor — an intermediate loop/branch/for-each/parallel control segment —
  and retain it for a path's terminal segment (the one that actually lands
  a `SequenceStatus`) and for the root `procedure` seat, so distinct
  anonymous leaves at different declaration positions don't collapse into
  one node. A landed leaf never inherits a structural group's ordinal
  either, even when its terminal segment carries an enclosing
  iteration/item index. See the module doc comment for the full rule, and
  for the explicit caveat that `parallel`/`for-each` are covered by
  fixture, not by observation against a real ledger.
- A **structural** group node (an iteration group, a branch-arm group, an
  orphan container placeholder) carries no `SequenceStatus` of its own and
  never fabricates one: a `pending` loop container's own status renders
  verbatim even while its children read `accepted`. Every other node's
  state is the durable `SequenceStatusKind` mapped 1:1 (`Accepted -> Done`,
  etc.) — except the **current** frame (the node whose normalized path
  equals `next_frame.position_path`, falling back to `active_path`), whose
  state word instead comes from `SessionState::derive`, the exact function
  `run_row::row_state` already calls, so `Running`/`WaitingOnAgent`/etc. are
  decided in one shared place. `live` is kept refreshed from every
  `RowChanged`/`Appeared`/`Ended` delta while a selection is following the
  center stream (`detail.rs`'s `Selection::live`, driven by `RunDetail::
  follow` — see "Live follow (0257.3)" below) rather than the point-in-time
  capture from `RunRow::live` at selection time it started as.
- The header reuses `ctx_traits_io::run_summary::RunSummary::from_session`
  for every fact (title, task value, elapsed, tokens, landing, stop reason,
  next frame kind) and `run_row.rs`'s `elapsed_text`/`token_value`/
  `tokens_text` (promoted to `pub(crate)`) for formatting — nothing is
  re-derived.
- Activity is **strictly embellishment**: `detail::load` now also runs
  `ctx_traits_io::activity_sidecar::read_activity` (tolerant — an absent
  file or an unparseable/truncated trailing line degrades only the sidecar
  read, never the caller) and carries the result in `DetailBaseline`
  alongside the authoritative `Session`. `ActivityOverlay` folds those
  records and is applied only *after* the durable tree is fully built, so
  it has no path that can add, remove, reorder, or restate a node. Only the
  latest `Activity`/`Narration` record for the **current** frame's
  `frame_id` is attached (an event's `frame_id` is iteration-blind, so
  attaching it to every historical iteration of a looped item would
  fabricate evidence); `StepSummary` records are read but never attached —
  matching them to a node needs the CLI's private `structural_step_key`
  encoder, which is a follow-up, not this task's scope.

**Native rendering** (`src/detail_view.rs`) is a **free function**,
`detail_view::detail_element(tree: &DetailTree) -> AnyElement` — no
`Context`, no `cx.listener`, since this slice has no controls (no
expand/collapse, no scroll-follow, no click targets) — so it is directly
callable from a test with no gpui `App`. It emits nested `div()`s, one per
node with children nested inside their parent, indentation via `.pl(px(..))`
and state as its own child element; depth, hierarchy and state live in the
element structure, not in a pre-formatted string. `Shell::render` picks
`detail_view::loading_element()` / `detail_view::failed_element(reason)` /
`detail_view::detail_element(&tree)` for the three `DetailLoad` states.

An unvirtualized nested column is accepted at this walking-skeleton scale
(a 7-iteration x 3-item loop is ~24 nodes; a 20-iteration run is ~60+); if a
later task needs it, `uniform_list` is the escalation path. Likewise no
`gpui` `test-support` feature is pulled in — element construction is pure,
so no `TestAppContext` is needed here; it would be the escalation path for a
future laid-out/painted assertion.

`desktop/tests/detail_frame_tree.rs` walks the real `RunRow -> select ->
load -> project` path against a nested-loop ledger (`support::
write_nested_session_ledger`) and a sidecar with a deliberately truncated
trailing line (`support::write_activity_sidecar`), asserting the hierarchy,
the un-fabricated loop-container state, the current-frame activity/
narration attachment, `skipped_activity_lines == 1`, and that the durable
tree is unchanged once the sidecar file is deleted entirely — proof that
the sidecar can only ever embellish. It also builds the real element tree
via `detail_view::detail_element`, and the `Loading`/`Failed` states, with
no `App`.

## Live follow (0257.3)

An open detail view now stays current as frames land, without polling and
without re-reading the ledger for every update.

**The two-lane rule.** `CenterPublicRow.summary` (`RunSummary`) does not
carry `ledger.sequence_statuses`, so the frame tree's node set can never be
advanced from a delta's payload alone — only a ledger re-read can grow or
reshape it. Activity, by contrast, arrives on the wire in full
(`CenterDelta::ActivityLine`'s `ActivityRecord`) and needs no read at all.
So: **activity advances from deltas with zero IO; frame structure advances
from a coalesced, evidence-gated re-read of the ledger**, gated on
center-supplied evidence that the ledger actually changed, at most once per
landed frame, never once per delta. `RunDetail::follow(&LinkUpdate)` is the
one entry point this drives through, mirroring `CenterFace::apply`'s shape
and taking the update by reference so `Shell` can feed detail first (it
reads nothing from the face) and hand the owned update to the face
unchanged — preserving stream order exactly for both consumers.

**The fingerprint.** `(row.modified_epoch_secs, row.summary with title
cleared)`. `RunSummary` derives `PartialEq`, so this catches every
ledger-derived fact the center projects — status, `current_sequence_title`,
elapsed, tokens, landing, stop reason, and so on — while ignoring `title`,
which the center rewrites from a sidecar `SessionTitle` line the overlay
already carries, immediately followed by a `RowChanged` that must not
itself cost a read. A `RowChanged`/`Appeared` whose fingerprint is
unchanged from the selection's current one issues nothing; one whose
fingerprint moved bumps the generation and returns exactly one
`LoadRequest`, coalescing for free through the existing generation guard —
the newest trigger always supersedes an in-flight read, so there is no
extra in-flight bookkeeping and no way to starve.

Accepted, documented bound: `modified_epoch_secs` is **1-second
resolution** — this is a property of the wire itself
(`CenterPublicRow`), not just this crate's own comparison, and is easy to
observe directly: two ledger writes landed inside the same wall-clock
second produce identical `CenterPublicRow`s and the center emits no
`RowChanged` between them at all (`detail_live_follow.rs` has to space its
frame-landing writes past a full second for exactly this reason). Bounded
and self-healing — the tree converges on the next write that crosses a
second boundary, and always on the terminal transition (a status change is
a fingerprint change). The rigorous fix, carrying the session's
`state_digest` on `CenterPublicRow`, is an `io`-crate change outside this
slice's scope.

The CLI's `refresh_attached_view`
(`modules/cli/src/app/dashboard.rs:3518`) solves the same "did the ledger
change" question with a `state_digest` compare on a poll tick. The desktop
does the same digest-vs-sidecar split but on a push stream instead of a
tick — parity of design, not shared code (the desktop must not depend on
`ctx-traits-cli`, and a tick is exactly what this task forbids).

**Activity replay at the seed/resync boundary.** An `ActivityLine` for the
selected run, while `Following`, folds straight into the *displayed*
`ActivityOverlay` (`ActivityOverlay::apply_live_record`) whenever `load` is
already `Loaded` — for immediate feedback. Independently, `Selection`
tracks `in_flight: Option<u64>` — the generation of a load (seed or resync)
that has not yet settled — separately from `load`, precisely because a
resync deliberately *keeps* `load` at `Loaded` with the old baseline so the
display never flashes back to `Loading`; `load`'s own state can therefore
not be used to tell "is a read in flight". Whenever `in_flight` is set, the
same record is *also* buffered in a capped `pending` queue (256, oldest
dropped: the overlay is last-write-wins per `frame_id`, so the oldest
buffered record is precisely the one whose loss is invisible) and replayed
once that read lands — this is what stops a wholesale baseline replacement
from silently dropping a record that arrived mid-resync. `ActivityOverlay`
carries a `watermark_ms` (the latest `at_epoch_ms` folded in) so a replayed
record from before the seed's own sidecar read can never roll a newer,
already-folded line back to a superseded one — equal timestamps are kept,
since millisecond-resolution collisions are idempotent under the
last-write-wins fold.

**Failure and recovery.** `Stale` is absorbing: `follow_delta` checks
`selection.follow == Following` once, ahead of the whole `CenterDelta`
match, so every delta variant — activity, `RowChanged`/`Appeared`, and
`Ended` alike — is a no-op while stale, not just the two that had their own
copy of the check historically. Only a recovery `Snapshot` closes it.

- `Down(reason)` marks the selection `Stale { reason }`; the last
  authoritative tree stays visible and labelled (`detail_view::
  follow_element`, worded to mirror `CenterFace::header`'s existing
  staleness shape so the two faces never word the same condition two ways).
- A selection made from a row served by an already-`Stale` `CenterFace`
  (`RunDetail::select_stale`, used by `Shell::select_ledger` whenever
  `CenterFace::stale_reason()` is `Some`) starts `Stale` too, rather than
  looking current: without this, a run picked from a retained stale list
  would ignore the eventual recovery snapshot and could never resync a
  change made during the outage.
- `Ended` for the selected run — reserved by the center for a ledger that
  has actually disappeared — sets `live = false` and `Stale` with a
  distinct reason, and issues **no** re-read: reading a deleted ledger
  would only turn a good tree into `Failed`.
- A recovery `Snapshot` while `Stale` returns to `Following` and issues
  **exactly one** resync `LoadRequest`, whether or not the snapshot still
  carries a matching row — the ledger is authoritative either way. An
  already-`Following` selection (the first snapshot, or a duplicate) issues
  nothing.
- A **resync**'s failure keeps the previous `Loaded` baseline on screen and
  reports `Stale` instead of clobbering it with `Failed` — the rule is
  state-based (`Loaded` at the time of the error means "this was a
  resync"), so it needs no extra flag on the request. A *first* load's
  failure still lands as `Failed`, since a fresh selection is never
  `Loaded` when its read is issued. The currently displayed baseline is
  never flashed back to `Loading` during a resync either, for the same
  reason.

**Reuse.** `Shell::spawn_load` is the one background-executor /
generation-guard / notify-on-real-change path, extracted so both the
initial selection and every follow-driven resync share it; `detail_task`
staying a single slot means assigning a new task cancels a superseded
in-flight read as a courtesy on top of the generation guard (reads are
read-only, so nothing is lost), not the correctness guard itself — the
generation carried on the request already is.

**Restart identity.** A closed-and-reopened process discards all derived
in-memory detail (`RunDetail`, `ActivityOverlay`, `pending`, `follow`
state) and reconstructs a fresh `DetailTree` from the ledger and its
sidecar alone, exactly as the very first `select`/`load` does — there is no
persisted live-follow state to go stale. `detail_live_follow.rs` proves
this directly, and proves it is not merely a coincidence of the two paths
happening to agree on the parts it checks: a freshly constructed
`RunDetail` selecting the same row produces a `DetailTree` that is
`assert_eq!`-equal, whole-struct, to the live-followed tree — and reading
the ledger to build it leaves the ledger's bytes on disk unchanged.

`tests/detail_live_follow.rs` drives a real in-process center (the
`live_deltas.rs` pattern): seeds a one-frame ledger, lands two more frames
by rewriting the ledger directly (the honest stand-in for a driver's
frame-boundary write — no driver, no desktop-owned write path), and drains
updates in stages — asserting the tree is exactly two nodes after the
first landing and exactly three after the second, each transition costing
**exactly one** re-read, never one per delta the scan-driven center happens
to emit for the same write. It then appends sidecar narration for the last
frame, rewrites the ledger to a genuinely terminal (`completed`) state, and
asserts that transition too costs exactly one re-read and lands a terminal
header. `tests/detail_stale_recovery.rs` drives `support::FakePeer` by hand
to prove the disconnect/stale/delta-dropped/resync-once cycle without a
real center in the loop. Both are env-mutating and stay the sole `#[test]`
in their target, per the convention `tests/support/mod.rs` documents.

## Spawn (0258.1)

Submitting a trait and arguments from the desktop creates a detached run
through the **same shared center capability** every face uses — never a
GUI-private write path.

**The structural problem this slice solves.** `ctx_traits_io::center::
start_trait` resolves `center_executable()` as `std::env::current_exe()`
and spawns that executable when no center is serving. For `ctx-desktop`
that would fork a second copy of the GUI, not a center — exactly the
hazard `subscribe_existing` was introduced for on the read path (see
"Center link (0256.2)" above; `subscribe_existing` never spawns either).
`ctx_traits_io::center::start_trait_existing` is the symmetric write-path
entry: it connects to an already-serving center only, reporting connection
failure rather than retrying, and is the one spawn entry this crate calls.
`modules/cli/tests/proof_center.rs` proves both halves of this at the
process level: `start_trait_existing_never_spawns_a_center_when_none_is_
serving` (the regression guard for the fork hazard) and
`start_trait_existing_reaches_two_subscribers_and_the_run_outlives_the_
requester` (the desktop-shaped path end to end — dropping the requesting
subscription immediately after the start is accepted, then asserting only
the second, untouched subscriber observes the run appear and complete).

**Repository identity comes from center state, never the process's cwd.**
`Dashboard::repositories()` derives the spawn picker's choices from the
*unfiltered* keyed row map (the current `RepoScope` is a view filter and
must not shrink the set a spawn can target), deduped by `repo_key` and
filtered to rows carrying an absolute `repo_path` (`camino::Utf8Path::
is_absolute`) — `run_start` rejects an empty or relative path, and
`repo_path` is genuinely empty whenever the center could not resolve one
(see "Run rows (0256.3)" above). **Ceiling,
stated explicitly:** a repository the center has never seen a run in
cannot be spawned into from the desktop yet. The rigorous version — a
`Repos` request serving the center's own repo index — is a protocol
addition with its own proof obligations, deliberately deferred rather than
smuggled into this slice.

**`spawn_form.rs` is gpui-free**, the same pattern `dashboard.rs`/
`detail.rs` document: `SpawnForm` holds the submitted text, cursor,
offered repositories, selection, and a `SpawnStatus` (`Idle` /
`Invalid(reason)` / `Requesting` / `Requested { session_id }` /
`Rejected(reason)`). `submit()` validates through
`ctx_traits_io::spawn_request::parse_spawn_args` — the face-independent
half of the TUI's spawn validation (one argument per line, `#` comments
and blanks dropped, a small forbidden-flag list rejected by name), shared
so the desktop does not copy a security-shaped denylist that can drift —
and refuses to produce a request while empty, forbidden, unselected, or
already `Requesting` (one in-flight request per form). `settle` never
touches `Dashboard`: `SpawnForm` has no field or method that could reach
one, so optimistic row insertion is impossible by construction, not by
convention — a spawned run becomes visible only through the same
subscription-delta path every other row does. `Requested { session_id }`
is worded as *requested*, not *running*, for exactly this reason, and
clears itself (`clear_requested_for`) once a delta for that session
arrives; if it never arrives the message honestly stays.

**`Shell` wiring stays off the UI thread.** `ACTION_TIMEOUT` is 600s:
`start_trait_existing` can block the calling thread for up to ten minutes
waiting for the driver to register, so `Shell::submit_spawn` mirrors
`Shell::spawn_load`'s shape exactly — `cx.spawn` awaiting
`cx.background_spawn`, landing the result through `SpawnForm::settle`
behind the same generation guard `RunDetail::apply` uses. The window
closing (dropping `spawn_task`) is correct and load-bearing, not a leak:
the center's `run_start` has already spawned the driver detached by the
time a `Started` response would arrive, so the child keeps running
regardless — proven at the process level by
`proof_center::dropping_the_requester_mid_start_leaves_the_detached_
driver_running`.

**What is deliberately not built.** No `CenterDelta` variant, no new
`Request` variant, no GUI-specific spawn verb, no local row insertion or
"pending run" placeholder, no polling timer or `center::list()` call after
a successful start, no filesystem read anywhere in the spawn path. No
`control_existing`/`start_session_existing` scaffolding — those belong to
0258.2/0258.3.

**Text entry.** gpui 0.2.2 has no `TextInput` component and no built-in
editor element (that lives in Zed's private `ui` crate), so `Shell`
implements the minimum on a focusable `div`: `on_key_down` appends
`keystroke.key_char` when present and handles `backspace` / `enter`
(newline) / `cmd-enter` (submit) / `escape` (close) by `keystroke.key` and
`modifiers`. No `EntityInputHandler`, no IME, no selection, no clipboard —
non-ASCII input, dead keys, and paste are imperfect. Accepted ceiling for
this walking-skeleton form, recorded here as a decision rather than an
oversight.

**Rendering.** `spawn_view.rs` mirrors `detail_view.rs`: free functions,
no `Context`, directly callable from a test with no gpui `App`.
`spawn_element` renders the text/status/hint; the repository picker's
row (with its click listener) is built in `Shell::render` itself — the
same "the interactive row lives in `Shell::render`, not in a pure view
module" split the run list already uses — since it needs a `Context` to
wire the listener with and `spawn_view.rs` has none.

`tests/spawn_through_center.rs` drives `support::FakePeer` by hand across
two connections (the snapshot/delta subscription never pipelines a second
request on the same connection — see `support::FakePeer`'s own doc
comment): serves a snapshot with a resolvable and an unresolvable-`repo_
path` row, asserts only the resolvable one is offered, submits a request
with the process cwd set outside every repository (so a cwd-derived path
is structurally impossible to confuse with a pass), asserts the argv
reaching the center is exactly the user's lines with the center-supplied
`repo_path`, asserts the row does not exist until an `Appeared` delta
carries it, and then proves a disconnect/recovery cycle neither loses nor
duplicates the spawned row. Env-mutating and the sole `#[test]` in its
target, per the same convention.

## Interrupt (0258.2)

Requesting a stop for a live run from the desktop invokes the **same
shared center control capability** every face uses — never a driver-facing
verb, and never a completion inferred from the request's own
acknowledgement.

**`control_existing` is `control`'s existing-only sibling.**
`ctx_traits_io::center::control` resolves `center_executable()` as
`std::env::current_exe()` and spawns that executable when no center is
serving — for `ctx-desktop` that forks a second copy of the GUI, exactly
the hazard `start_trait_existing` (0258.1) and `subscribe_existing`
(0256.2) exist for on the write and read paths respectively.
`control_existing` is the write-path entry this crate calls: it connects
to an already-serving center only, reporting connection failure rather
than retrying. `control`/`control_existing` share one request builder and
one decode (`control_request`/`decode_control`), the same
`trait_start_request`/`decode_start` shape 0258.1 established, so the two
entry points differ only in which transport helper they call.
`ControlResult::message` — the six-case wording the TUI footer renders —
moved from `ctx-traits-cli`'s private `control_message` into
`ctx-traits-io` alongside `ControlResult` itself, so the desktop's status
line and the TUI's footer share exactly one wording source rather than a
hand-copied duplicate (the crate boundary forbids the desktop depending on
`ctx-traits-cli`). `modules/cli/tests/proof_center.rs` proves both halves
of the transport at the process level:
`control_existing_never_spawns_a_center_when_none_is_serving` (the
regression guard for the fork hazard) and
`control_existing_interrupt_reaches_two_subscribers_and_the_run_outlives_
the_requester` (the desktop-shaped path end to end — dropping the
requesting subscription immediately after the request is acknowledged,
then asserting only the second, untouched subscriber observes the
interrupted effect).

**`interrupt.rs` is gpui-free**, the same discipline `spawn_form.rs`
documents: no field, method, or dependency here can reach a `Dashboard`,
so the type system — not a convention — forbids optimistic row mutation.
`Interrupts` is a `HashMap<ledger_path, _>`, not a single slot like
`SpawnForm`: several rows can be interrupted independently. `request`
refuses (returning `None`, touching nothing) when the row is not
`RowState::Live` or already `Requesting` for that `ledger_path` — the
client-side guard, `SpawnForm::submit`'s `Invalid`/`Requesting` analogues.
`settle` is generation-guarded exactly as `SpawnForm::settle` is:
`ControlResult::Acknowledged` moves to `Requested { session_id }`, worded
*"stop requested — waiting for the center"* and never *"stopped"* — the
same reasoning `SpawnStatus::Requested`'s wording documents, since
acknowledgement means the driver accepted the request, not that the run
stopped. Every other `ControlResult` variant, and a transport `Failed`,
move to `Refused(message)`. `observe` is the **only** path that clears a
`Requested` entry: the row is gone from the model (an `Ended` delta
removed it) or is present and no longer live — acknowledgement never
clears it, only an observed delta does.

**Reconciliation reads the unfiltered row model, not the scoped
projection.** `Dashboard::row_liveness`/`CenterFace::row_liveness` sit
beside `contains_session` and reuse its unfiltered-map doctrine: a run
whose repository the current `RepoScope` hides must still resolve a
pending interrupt (the exact blocker class 0258.1 had to reopen for
spawn). `CenterFace::row_liveness` returns `Option<Option<bool>>` rather
than collapsing "no model yet" (`Connecting`/`Unavailable`) and "the row
ended" (present in a `Connected`/`Stale` model but absent from the keyed
map) into the same bare `None` — the two must be treated oppositely, and
`reconcile_interrupts` early-returns on the former so an outage can never
be mistaken for an observed stop. `reconcile_interrupts` is a free
function beside `reconcile_spawn_status`, walking every `Requested` entry
`Interrupts` currently tracks: the row's own delta and the control
response are scheduled on independent connections and can land in either
order, so both the update loop and `Shell::interrupt_row`'s settle
callback run this one shared implementation.

**`Shell` wiring stays off the UI thread.** `control_existing` inherits
`ACTION_TIMEOUT` (600s), so `Shell::interrupt_row` mirrors
`Shell::submit_spawn`'s shape exactly — `cx.spawn` awaiting
`cx.background_spawn`, landing the result through `Interrupts::settle`
behind the generation guard, then running `reconcile_interrupts` again
(the response may have lost the race to the row's own delta), and
notifying only on real change. `interrupt_tasks` is a
`HashMap<ledger_path, gpui::Task<()>>`, not a single `Option` slot like
`spawn_task`: a single slot would cancel an unrelated in-flight interrupt
for a different row.

**What is deliberately not built.** No new `CenterDelta` variant, no new
`Request` variant, no GUI-specific control verb, no local row mutation on
acknowledgement, no ledger write, no direct driver contact. Pause/resume
(0258.3), detail-pane rendering, and summons are out of scope here.

`tests/interrupt_through_center.rs` drives `support::FakePeer` by hand
across two connections (the snapshot/delta subscription and the control
request/response round trip never share one connection), asserting the
control request's wire shape (`kind: "control"`, `command: "interrupt"`,
`session_id`/`repo_key` exactly as the center supplied them, with the
process cwd moved outside any repository first), that the row stays live
after `Acknowledged` alone, and that a `RowChanged { live: false }` delta
resolves it — covering both the ack-then-delta and delta-then-ack
orderings, plus one refusal path. Env-mutating and the sole `#[test]` in
its target, per the same convention.

## Dark token set and bundled faces (0265.1)

`src/tokens.rs` is the one entry point for `.internal/docs/design/tokens.md`'s
dark column: every named colour, the three literal diff washes, the type
scale, the used weight, both font-family names and every layout constant,
each defined exactly once. `tokens.md` stays the only source of the values —
there is no second palette, no theme trait, and the light column is recorded
there and unimplemented here. `Shell::render` is the only consumer so far,
repainting the run list; every other surface still renders in gpui's
defaults until its own cut lands.

`tokens.md`'s five layout entries that are ranges rather than single values
(list-row pad, rows gap, dot-text gap, bordered-box pad, rail-divider pad)
are named as endpoint pairs — e.g. `LIST_ROW_PAD_Y_COMPACT`/`_OPEN` — rather
than silently collapsed to one number. This needs an owner ruling before
later cuts inherit the convention.

**Fonts are bundled, not fetched.** IBM Plex Sans and Mono (Regular only)
are vendored into `assets/fonts/` from a pinned upstream commit — see
`assets/fonts/README.md` for the exact source and per-file hashes — with the
SIL OFL 1.1 text beside the bytes. `src/fonts.rs` is the one registration
path (`include_bytes!` straight into `TextSystem::add_fonts`, not an
`AssetSource`: `0256.1` established no asset mechanism, and introducing one
to hand two static blobs to a byte-taking API would be the extra mechanism,
not the reuse — `0260` can revisit when the app bundle needs a resource
path) and the one verification (`verify_bundled`), both called from
`main.rs` before the window opens. A registration or resolution failure is
an `.expect()` panic at startup — loud, not a quiet fallback render.

`TestAppContext`/`#[gpui::test]` cannot prove font resolution: gpui's test
platform installs a no-op text system that resolves and "adds" fonts
without touching real data, so a `TestAppContext` version of this check
would pass with nothing vendored at all. The proof runs against the real
platform text system instead, via `gpui::Application::headless()` in the
standalone `ctx-desktop-font-proof` binary (`src/bin/font_proof.rs`), which
`tests/bundled_fonts.rs` drives as a subprocess in two modes: default
(register, then the bundled families must resolve to a bundled `FontId`,
not gpui's fallback stack) and `--skip-registration` (registration
skipped, and the resolution check must itself fail — the forced-negative
control proving the positive check is not vacuous). Driving it as a
subprocess test keeps the proof inside plain `cargo test` / `just
desktop-test` without any `Justfile` change.

## The rail (0265.8)

Rule 10's rail — one 260-wide full-height column on the Sessions screen —
splits the same way every other surface here does: `src/rail.rs` is a
gpui-free model (grouping, ordering, the dot rule, the brightness
invariant), `src/rail_view.rs` is the free-function element that paints
exactly that model. Rows are the distinct repositories of the accepted
center row model (`Dashboard`'s unfiltered keyed map — the same doctrine
`repositories`/`contains_session`/`row_liveness` already document, so a
`RepoScope` that hides a repository's rows from the run list never hides it
from the rail), one per `repo_key`, ordered by `(name, repo_key)` — total
and insertion-independent. The active repository is the selected run's
repository (`RunDetail::repo_key()`); it drives both the active row and the
footer's space line through one projection, `rail::repo_display_name`
(final two path segments, falling back to `repo_key`) — deliberately not
`run_row::repo_label_for`, the compact row's different, one-segment label.
The active identity is only active if it still matches a repository in the
current list, so when that repository's last row ends, the active row and
the footer's space line disappear together — no fallback promotion.

The rail's dot reuses `frame_list`'s one dot primitive rather than growing a
second: `DotTone::Bright` is a new variant for `text-bright`, rule 10's
instruction for the rail's active row specifically and not part of rule 4's
palette (the same extension class `0265.5` used for `Danger`), and
`dot_color`/`dot_element` are promoted `pub(crate)` so `rail_view.rs` calls
the identical 5px `rounded_full` element the frame list paints. The owner
handle resolves through one new `placeholders::OWNER_HANDLE` entry, not an
inline literal.

The rail participates in the same accepted snapshot/delta transaction the
run rows do — `Dashboard::rail`/`CenterFace::rail` are pure per-frame
projections off the existing model, with no second subscription, reducer,
cache, timer, or poll; a `Down` retains the last complete accepted rail
behind the stale marker (dimmed via `.opacity(0.6)` on the rows container
only, matching the run list's own stale treatment) and only a fresh
snapshot replaces it wholesale.
