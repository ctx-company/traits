//! P423 session-manager dashboard: bare interactive `ctx traits` on a TTY.
//!
//! Four polling screens — SESSIONS, TRAITS, MERGES, TRUST — sharing one
//! [`super::tui_ratatui::RatatuiPane`] terminal owner. Every action reuses an
//! existing entity operation (drive registration/probing, trust update, plain
//! merge, trait resolution) instead of shelling out or re-parsing CLI output;
//! see the referenced modules for the canonical logic. No daemon is
//! introduced: `n` detaches a plain `ctx traits run` child that registers
//! itself under the same driver lock every other drive uses, so externally
//! started drives are indistinguishable from dashboard-spawned ones.
//!
//! P469 turns SESSIONS into a master-detail surface with six one-keypress
//! verbs (preview, enter, back, resume, kill, delete), all routed through
//! [`tui_kit::ModalHost`] so no keypress can trigger a side effect without a
//! resolved modal.
//!
//! P471 turns TRAITS into the same master-detail shape: a live preview pane
//! (summary, procedure shape, digest/trust facts, a bounded source excerpt),
//! APPROVE/DENY collected through the kit's text-input modal (the `$EDITOR`
//! temp-file reason path is gone — for TRAITS *and* TRUST, since both
//! screens' reason-collection now routes through [`open_trait_trust_modal`]
//! / [`open_trust_digest_modal`] and [`apply_trait_action`]), and an
//! identity-addressed EDIT SOURCE round-trip that returns to the same row
//! and refreshes the preview in place, degrading inline on a broken rebuild
//! rather than crashing. `SessionAction` generalized to `Action` (`Exit` /
//! `Session(..)` / `Trait(..)`) so both screens share one `ModalHost`.
//! MERGES is untouched by this phase.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::IsTerminal;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line as RLine, Span};
use ratatui::widgets::Paragraph;

use super::frame_prompt::resolved_human_question_body;
use super::lifecycle_reporting::{
    DashboardTraitRow, dashboard_trait_drift, dashboard_trait_editable_source,
    dashboard_trait_inventory,
};
use super::merge::{MergeInputs, merge};
use super::merge_story;
use super::report_check::sequence_kind_label;
use super::run_view;
use super::trust_story;
use super::tui;
use super::tui_kit::{self, MarkSet, Modal, ModalHost, ModalOutcome, ScrollDelta, ScrollList};
use super::tui_panes::{self, FocusRing, PaneId, PaneLayoutResult, PaneScrolls, PaneTree};
use super::tui_ratatui::{self, RatatuiPane, render_line};

use ctx_traits_core::task::TaskStatus as TaskDocStatus;
use ctx_traits_core::task::graph::DerivedStatus;
use ctx_traits_core::task::provider::{
    EffectKind, EffectOutcome, EffectRecord, NewTask, ResolvedTask, SyncReport, TaskProvider,
    TaskProviderMut, TaskSummary, TaskUpdate,
};
use ctx_traits_io::task_board_cache::{self, BoardSnapshotRecord};
use ctx_traits_io::task_files::{self, BoardFingerprint, FilesTaskBoard};

mod worker;

const TICK: Duration = Duration::from_millis(250);
/// Bound on how long a list screen (SESSIONS/TRAITS/MERGES/TRUST) can go
/// without an automatic reload while idle, so externally started, dashboard-
/// spawned, or externally completed runs surface without a keypress. Also
/// the cadence [`refresh_attached_view`] rebuilds a stale SESSIONS
/// preview/attach pane on — never the 250ms draw loop (see its own doc).
const RELOAD_INTERVAL: Duration = Duration::from_secs(2);

/// Cadence of the periodic full liveness sweep, in reload ticks
/// (`RELOAD_INTERVAL` apart). Every 10th tick at the default 2s interval is
/// 20s — bounded cost (still far fewer probes than an every-row,
/// every-tick baseline) against an honest worst case: a row-less held lock
/// (adoption) becomes visible within one sweep interval, never instantly and
/// never never.
const FULL_SWEEP_EVERY_TICKS: u64 = 10;

/// `State::reload()` durations at or above this are surfaced in the
/// SESSIONS pane border title; below it, nothing is shown. Zero during
/// development to read the real number; kept as a named threshold (not
/// inlined) so tightening it later is a one-line change.
const RELOAD_WARN_THRESHOLD: Duration = Duration::from_millis(50);

// Narrow-terminal degradation thresholds (P506 §3.2): below `left_min +
// right_min`, a screen's pane tree collapses to the list alone — `PaneTree`
// itself deliberately has no floor/cap policy (`tui_panes.rs`'s own doc), so
// this is decided here, pre-tree, rather than re-imported.
const SESSIONS_LEFT_MIN: u16 = 60;
const SESSIONS_RIGHT_MIN: u16 = 30;

const TRAITS_LEFT_MIN: u16 = 50;
const TRAITS_RIGHT_MIN: u16 = 40;

const MERGES_LEFT_MIN: u16 = 50;
const MERGES_RIGHT_MIN: u16 = 40;

const TRUST_LEFT_MIN: u16 = 50;
const TRUST_RIGHT_MIN: u16 = 40;

const TASKS_LEFT_MIN: u16 = 50;
const TASKS_RIGHT_MIN: u16 = 40;

const PANE_SESSIONS_LIST: PaneId = "sessions-list";
const PANE_SESSIONS_PROGRESS: PaneId = "sessions-progress";
const PANE_SESSIONS_JOURNEY: PaneId = "sessions-journey";
/// P081: unused sentinels. `render_sessions_preview_body`'s own
/// `PaneData` never populates `history`/`current` (the list-visible preview
/// is progress/journey only — attach now hands the WHOLE terminal to the
/// shared `run_view::RunPanel` observer instead of a second in-dashboard
/// four-pane body), but `run_view::PaneIds` still requires concrete ids for
/// every slot; `run_view::pane_tree` never materializes a leaf for either of
/// these two.
const PANE_SESSIONS_HISTORY: PaneId = "sessions-history";
const PANE_SESSIONS_CURRENT: PaneId = "sessions-current";
/// P552 review `live-run-pane-contract-absent`: the ordinary (list-visible)
/// SESSIONS tree's sole outer placeholder for the whole progress/journey
/// region — never rendered directly (skipped in `draw_screen`'s generic
/// leaf loop) and never a focus target. `run_view::pane_tree` is the only
/// code that ever creates and sizes the REAL `PANE_SESSIONS_PROGRESS`/
/// `PANE_SESSIONS_JOURNEY` leaves, from this placeholder's own resolved
/// rect, inside `render_sessions_preview_body`.
const PANE_SESSIONS_PREVIEW_REGION: PaneId = "sessions-preview-region";
/// P081: renamed from `SESSIONS_ATTACH_PANE_IDS` — attach no longer draws
/// through this module at all (see [`attach_selected`]/`run_with_initial_session`'s
/// attach loop, which hands the terminal to `run_view::RunPanel::new_observer`
/// instead); this backs only the list-visible progress/journey preview now.
const SESSIONS_PREVIEW_PANE_IDS: run_view::PaneIds = run_view::PaneIds {
    progress: PANE_SESSIONS_PROGRESS,
    journey: PANE_SESSIONS_JOURNEY,
    history: PANE_SESSIONS_HISTORY,
    current: PANE_SESSIONS_CURRENT,
};
const PANE_TRAITS_LIST: PaneId = "traits-list";
const PANE_TRAITS_PREVIEW: PaneId = "traits-preview";
const PANE_MERGES_LIST: PaneId = "merges-list";
const PANE_MERGES_PREVIEW: PaneId = "merges-preview";
const PANE_TRUST_LIST: PaneId = "trust-list";
const PANE_TRUST_PREVIEW: PaneId = "trust-preview";
const PANE_TASKS_LIST: PaneId = "tasks-list";
const PANE_TASKS_PREVIEW: PaneId = "tasks-preview";

/// Truly bare interactive `ctx traits`: stdin and the rendering terminal
/// (stderr, matching `--progress tui`'s existing check) are both TTYs and
/// `TERM` is not `dumb`. Anything else — CI, pipes, `TERM=dumb` — keeps the
/// byte-identical line-mode output the caller already prints.
pub(crate) fn interactive_available() -> bool {
    if std::env::var("TERM").is_ok_and(|term| term.eq_ignore_ascii_case("dumb")) {
        return false;
    }
    // Some CI runners allocate a pseudo-terminal for job output, so
    // `is_terminal()` alone is not sufficient: a set `CI` env var (the de
    // facto convention every major CI provider honors) always keeps the
    // exact non-interactive line-mode output.
    if std::env::var_os("CI").is_some() {
        return false;
    }
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    Sessions,
    Traits,
    Merges,
    Trust,
    Tasks,
}

impl Screen {
    fn title(self) -> &'static str {
        match self {
            Screen::Sessions => "SESSIONS",
            Screen::Traits => "TRAITS",
            Screen::Merges => "MERGES",
            Screen::Trust => "TRUST",
            Screen::Tasks => "TASKS",
        }
    }

    fn all() -> [Screen; 5] {
        [
            Screen::Sessions,
            Screen::Traits,
            Screen::Merges,
            Screen::Trust,
            Screen::Tasks,
        ]
    }
}

/// One row's verb eligibility, decided in exactly this one place so every
/// key handler and every rendered hint reads the same classification rather
/// than re-deriving it. `Held` (any status) always wins as `Live`; otherwise
/// a readable ledger's own `Status` decides `Terminal` vs `Resumable`; an
/// unparseable ledger is `Unreadable`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SessionClass {
    Live,
    Resumable,
    Terminal,
    Unreadable,
}

impl SessionClass {
    fn can_resume(self) -> bool {
        self == SessionClass::Resumable
    }

    fn can_attach(self) -> bool {
        self != SessionClass::Unreadable
    }
}

#[derive(Clone)]
struct SessionRow {
    session_id: String,
    ledger_path: camino::Utf8PathBuf,
    /// The ledger's internal run-id — distinct from `session_id` (the ledger
    /// file's basename) — used to look up this run's debug-trace directory
    /// (`ctx_traits_io::debug_trace::tail_latest_attempt_trace` is keyed by
    /// run-id, not session-id). Empty for an unreadable ledger.
    run_id: String,
    /// State column: `live` when a driver holds the lock, else the ledger's
    /// own status word ([`run_view::session_status`]).
    state_text: String,
    /// Phase column: status plus the current step title
    /// ([`run_view::phase_text`]).
    phase: String,
    elapsed_text: String,
    tokens_text: String,
    /// Repository/ad-hoc identity key this row belongs to (P439 ALL mode) —
    /// `None` in the default current-repository-only scope.
    repo_key: Option<String>,
    /// Absolute repository path for this row (P439 ALL mode only); used to
    /// gate cwd-anchored git operations (resume/delete worktree cleanup) to
    /// rows that actually belong to the current repository (§3.6).
    repo_path: Option<String>,
    class: SessionClass,
    /// The ledger's own `Status`, or `None` for an unreadable ledger — feeds
    /// [`session_group`] (§3.1). Not re-derived from `state_text`/`phase`
    /// (both display strings), so grouping stays a pure function of the
    /// typed status rather than a re-parse of what the row already renders.
    status: Option<ctx_traits_core::procedure::session::Status>,
    /// The ledger's last-recorded drive outcome kind, or `None` for an
    /// unreadable ledger or a session that has never driven. Threaded into
    /// [`SessionState::derive`][ctx_traits_core::procedure::activity::SessionState::derive]
    /// alongside `status` so a cancelled parked ask (status still
    /// `WaitingOnHuman`, outcome `Interrupted`) classifies `Cancelled` rather
    /// than staying grouped as an open ask.
    outcome: Option<ctx_traits_core::procedure::session::DriveOutcomeKind>,
    /// P552 persisted narrator session title, when one resolved — the
    /// dashboard row's primary run label; `None` for an unreadable ledger, a
    /// pre-P552 ledger, or a title attempt that never resolved (missing
    /// narrator, failed call).
    title: Option<String>,
    /// The ledger's `provenance.task_key` (0063): which board task, if any,
    /// this run was dispatched against. `None` for an unreadable ledger or a
    /// run never keyed to a task — the TASKS screen's only join key onto
    /// `state.sessions`.
    task_key: Option<String>,
    /// 0063.8: this run's last terminal merge frame's landed sha, when that
    /// frame is `Merged` — [`task_proposals::merged_landed_sha`]'s own
    /// result, carried onto the row so `rebuild_visible_tasks` never
    /// re-parses the ledger to derive a proposal. `None` for an unreadable
    /// ledger or a run that has not landed.
    merged_landed: Option<String>,
}

#[derive(Clone)]
struct TraitRow {
    id: String,
    version: String,
    status: String,
    trust: String,
    canonical_digest: String,
    source_path: String,
    /// The read error for an unreadable package, carried through unchanged
    /// from [`super::lifecycle_reporting::DashboardTraitRow::error`] rather
    /// than re-derived from `status`/`trust` text — the preview's degrade
    /// path (§4.6) checks this directly.
    error: Option<String>,
}

/// The reconstructed live-view pane for one TRAITS row (P471 §4.1),
/// mirroring [`AttachedView`]'s cache discipline: `trait_id` +
/// `canonical_digest` is the cache key (never a list index), so a moved
/// digest — or a source edit that leaves the digest untouched but changes
/// what the preview must say (§4.6) — is what forces a rebuild, never the
/// 2s reload tick alone.
struct TraitPreview {
    trait_id: String,
    canonical_digest: String,
    /// Carries the degraded rendering (§4.6) with the error at the top in
    /// `Tone::Fail` when the trait failed to load/check — never a crash,
    /// never an empty pane. The error text lives in these lines, not a
    /// separate field: there is no other consumer that needs it apart from
    /// what's drawn.
    lines: Vec<RLine<'static>>,
}

/// Pure facts [`trait_preview_lines`] renders from — no IO in this type or
/// in the function that renders it; [`build_trait_preview`] is the only
/// place that does the loading/reading (§4.2).
struct TraitPreviewFacts {
    id: String,
    version: String,
    status: String,
    canonical_digest: String,
    trust_state: String,
    trust_reason: String,
    /// The trust record's own digest differs from `canonical_digest` — the
    /// re-approval-required case (§4.2 point 3), reusing `load_trust`'s
    /// stale notion.
    trust_stale: bool,
    has_trust_record: bool,
    drift: String,
    /// Whether `drift` covers the authored source (`source/index.ts`), not
    /// only the lock/manifest layers `dashboard_trait_drift` compares
    /// (blocker `trait-preview-drift-omits-authored-source`): today this is
    /// always `false`, since `dashboard_trait_drift` unconditionally passes
    /// `skip_cdk_drift: true`. Kept as an explicit fact rather than a
    /// hard-coded render string so the renderer stays correct if that ever
    /// changes.
    source_drift_checked: bool,
    procedure: ProcedureShape,
    source_path: String,
    source_excerpt: Vec<String>,
    error: Option<String>,
}

/// Procedure shape as the preview must present it — three states, never
/// collapsed to a boolean (blocker `preview-mislabels-unloadable-trait-as-
/// guidance-only`): `Sequence` is a trait that loaded with `[procedure]`
/// present; `GuidanceOnly` is a trait that loaded and deliberately declares
/// none (a legitimate classification, PRODUCT.md §Non-Negotiables); `Unknown`
/// is a trait that could not be read/checked at all. The pane must never
/// assert the positive `GuidanceOnly` claim about a trait it failed to load.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProcedureShape {
    /// `(item id, item kind)` per `[[procedure.sequence]]` entry, in order.
    Sequence(Vec<(String, String)>),
    GuidanceOnly,
    Unknown,
}

/// MERGES-screen row classification (P472 §3.3): decided once, in one place,
/// from [`ctx_traits_core::procedure::session::MergeStatus::is_terminal`] and
/// [`super::run::disposition_for_merge_status`] — never re-derived from
/// string status text. `Mergeable` is a completed drive with worktree
/// provenance that has never reached a terminal merge frame; `Parked`/
/// `Failed`/`Landed` classify the last terminal frame, with
/// `PostMergeCleanupFailure`/`RecoveryFailure` always `Failed`, never
/// `Parked` (`session.rs`'s own doc comment on `MergeStatus`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MergeClass {
    Mergeable,
    Parked,
    Failed,
    Landed,
}

impl MergeClass {
    /// `m`/`d` retry is offered for every class except a landed run (nothing
    /// left to retry) — a still-running (non-terminal, no worktree
    /// provenance) row never reaches this screen at all.
    fn can_retry(self) -> bool {
        self != MergeClass::Landed
    }

    /// DROP (`x`) is scoped to terminal rows: a `Mergeable` row may still be
    /// held by a live driver, so dropping it from the queue here would race
    /// an in-progress run — SESSIONS' own DELETE already owns that case.
    fn can_drop(self) -> bool {
        self != MergeClass::Mergeable
    }

    fn label(self) -> &'static str {
        match self {
            MergeClass::Mergeable => "mergeable",
            MergeClass::Parked => "parked",
            MergeClass::Failed => "failed",
            MergeClass::Landed => "landed",
        }
    }
}

#[derive(Clone)]
struct MergeRow {
    session_id: String,
    run_id: String,
    ledger_path: camino::Utf8PathBuf,
    class: MergeClass,
    /// The last terminal merge frame's stage, or `None` for a `Mergeable` row
    /// that has never attempted a merge.
    stage: Option<ctx_traits_core::procedure::session::MergeStage>,
    /// The translated one-line headline ([`merge_story::Explanation::headline`])
    /// for a parked/failed row — empty for `Mergeable`/`Landed`.
    headline: String,
    phase: Option<String>,
    trait_id: String,
    /// The last terminal merge frame, kept for the detail pane's translation
    /// and gate-evidence rendering — `None` for `Mergeable`/rows with no
    /// terminal frame yet.
    last_frame: Option<ctx_traits_core::procedure::session::MergeFrame>,
    worktree: Option<ctx_traits_core::procedure::session::WorktreeProvenance>,
    /// Repository/ad-hoc identity path this row belongs to (P439 ALL mode) —
    /// `None` in the default current-repository-only scope, mirroring
    /// [`SessionRow::repo_path`].
    repo_path: Option<String>,
}

/// The reconstructed live-view pane for one MERGES row (P472 §3.4), mirroring
/// [`TraitPreview`]'s cache discipline: `session_id` + the last frame's
/// identity + the worktree's branch name is the cache key, so a Git-shelling
/// rebuild happens on selection change or when that key moves, never on the
/// 250ms draw tick.
struct MergePreview {
    session_id: String,
    cache_key: (String, String),
    lines: Vec<RLine<'static>>,
}

/// Pure facts [`merge_preview_lines`] renders from — no IO in this type or in
/// the function that renders it; [`build_merge_preview`] is the only place
/// that shells out to Git or reads the ledger.
struct MergePreviewFacts {
    run_id: String,
    phase: Option<String>,
    trait_id: String,
    class: MergeClass,
    stage: Option<ctx_traits_core::procedure::session::MergeStage>,
    /// What the run produced, already classified — `None` when the worktree
    /// is no longer registered (landed/cleaned up) and Git facts could not be
    /// computed.
    produced: Option<MergeProduced>,
    explanation: Option<merge_story::Explanation>,
    gate_rows: Vec<merge_story::GateRow>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
}

enum MergeProduced {
    Nothing,
    DocsOnly { files: usize },
    Commits { commits: usize, files: usize },
}

/// One TRUST-screen row (P473 §4.2): trait-centric, unlike the old
/// digest-centric shape — `trait_id: None` marks exactly the [`Orphaned`]
/// rows appended for a recorded decision that names no visible trait (never
/// synthesized for a resolvable trait). Covers every tier `InventoryContext`
/// sees, including built-ins (unlike [`State::traits`]), so a built-in
/// package still needing a verified digest to auto-activate is visible here.
///
/// [`Orphaned`]: trust_story::TrustClass::Orphaned
#[derive(Clone)]
struct TrustRow {
    trait_id: Option<String>,
    /// Display label for the winning tier: `"repo"` for repo-authored,
    /// otherwise the origin string [`super::lifecycle_reporting::DashboardTraitRow`]
    /// already carries (`"built-in"`, an npm origin, ...). `"orphaned"` for
    /// an orphan row, which names no resolvable trait to report an origin
    /// for.
    origin: String,
    family: Option<String>,
    variant: Option<String>,
    /// The trait's current resolved canonical digest — empty for an orphan
    /// row (no trait resolves to name one).
    current_digest: String,
    recorded_digest: Option<String>,
    class: trust_story::TrustClass,
    updated_at: Option<String>,
    reason: Option<String>,
}

/// One run-ledger row's identity/digest/recency, projected once per reload
/// from the same run inventory scan [`sessions_from_inventory_tagged`]/
/// [`merges_from_inventory`] already consume (§4.4) — cheap owned data so a
/// selection-time [`run_sighting`] lookup never re-scans the ledger store.
#[derive(Clone)]
struct RunSightingRow {
    trait_id: String,
    canonical_digest: String,
    run_id: String,
    session_id: String,
    modified_epoch_secs: u64,
}

/// The reconstructed live-view pane for one TRUST row (P473 §4.5), mirroring
/// [`TraitPreview`]'s cache discipline for resolvable rows: `trait_id` +
/// `current_digest` + the resolved [`trust_story::TrustClass`] is the cache
/// key (never a list index), so a trust write — which never moves the digest,
/// only the class — still forces a rebuild. Orphans rebuild on every selection
/// because append-only records can share a digest while carrying distinct
/// timestamps or reasons.
struct TrustPreview {
    trait_id: Option<String>,
    current_digest: String,
    class: trust_story::TrustClass,
    lines: Vec<RLine<'static>>,
}

/// Pure facts [`trust_preview_lines`] renders from — no IO in this type or
/// in the function that renders it; [`build_trust_preview`] is the only
/// place that resolves a run sighting (§4.4).
struct TrustPreviewFacts {
    trait_id: Option<String>,
    origin: String,
    family: Option<String>,
    variant: Option<String>,
    current_digest: String,
    recorded_digest: Option<String>,
    class: trust_story::TrustClass,
    updated_at: Option<String>,
    reason: Option<String>,
    sighting: Option<trust_story::RunSighting>,
    /// Every member of this row's family (including this row itself), each
    /// with its own class — present only when `family` is `Some` (§4.5:
    /// "for a family row, every member with its own class, so `A`'s blast
    /// radius is visible before any keypress"). Including the selected row
    /// is deliberate: `A`'s write also covers it, so it belongs in its own
    /// blast-radius roster.
    family_members: Vec<(String, trust_story::TrustClass)>,
}

/// TASKS' board cache (0063, freshness automated by 0063.7): the provider's
/// own `list`/`get` results plus its `sync` report, captured on every `s`
/// keypress, tick-detected board change, or provider write through the
/// dashboard. `resolved` is keyed by task key and only ever holds keys
/// `summaries` also names — a `get` that races a concurrent edit and returns
/// `None`/an error simply leaves that key absent, which the detail pane
/// renders as "relations unavailable".
#[derive(Clone)]
struct TasksBoardSnapshot {
    summaries: Vec<TaskSummary>,
    resolved: BTreeMap<String, ResolvedTask>,
    sync_report: SyncReport,
    /// Unix epoch seconds this read completed — wall-clock, not
    /// [`std::time::Instant`], so the "as of" age survives a dashboard
    /// restart via the persisted cache (0063.7).
    captured_at: u64,
    /// The board directory's stat-sweep signature at read time — what the
    /// 2s tick compares against to decide whether to re-read.
    fingerprint: BoardFingerprint,
}

/// The five fixed TASKS groups (0063's own "Done when": "blocked, ready,
/// in-flight, parked, done"). `Done`/`Cancelled` both land in `Done` — the
/// spec names exactly five groups, not six.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum TaskGroup {
    InFlight,
    Parked,
    Blocked,
    Ready,
    Done,
}

impl TaskGroup {
    fn order() -> [TaskGroup; 5] {
        [
            TaskGroup::InFlight,
            TaskGroup::Parked,
            TaskGroup::Blocked,
            TaskGroup::Ready,
            TaskGroup::Done,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            TaskGroup::InFlight => "in-flight",
            TaskGroup::Parked => "parked",
            TaskGroup::Blocked => "blocked",
            TaskGroup::Ready => "ready",
            TaskGroup::Done => "done",
        }
    }
}

/// TASKS' visible-row projection, mirroring [`VisibleRow`]: a group header
/// (toggled by Enter/space, no verb of its own) or a real task, addressed by
/// its key — never an index, since the board is keyed by string and a sync
/// can reorder or drop rows entirely.
enum TaskVisibleRow {
    GroupHeader {
        group: TaskGroup,
        count: usize,
        collapsed: bool,
    },
    Task(String),
}

/// The TASKS detail pane, mirroring [`TraitPreview`]: rebuilt on selection
/// change and on every sync, never in the draw path.
struct TaskPreview {
    key: String,
    lines: Vec<RLine<'static>>,
}

/// The reconstructed live-view pane for one session (P469 §3.2): the same
/// `Vec<RLine>` backs both the SESSIONS master-detail preview and the
/// attached full view — they differ only in the rect they're drawn into and
/// each carrying its own [`ViewportScroll`] offset. Rebuilt by
/// [`refresh_attached_view`] on selection change and on the reload tick
/// (never per-draw — reconstruction re-parses a trait package).
#[derive(Clone)]
struct AttachedView {
    session_id: String,
    ledger_path: camino::Utf8PathBuf,
    run_id: String,
    /// The ledger's `state_digest` as of the last successful reconstruction,
    /// so an unchanged ledger skips the expensive trait+plan rebuild on the
    /// next reload tick.
    state_digest: String,
    /// P552: the ledger's own pane projection — `progress`/`journey` back
    /// both the ordinary two-pane preview and the attached four-pane body;
    /// `history`/`current` (from the P521 activity sidecar) are used only
    /// while attached.
    progress_lines: Vec<tui::Line>,
    journey_lines: Vec<run_view::JourneyRow>,
    post_run: Vec<tui::Line>,
    history: Vec<run_view::EventRow>,
    current: Vec<run_view::EventRow>,
    /// The persisted P552 session title, when the drive resolved one —
    /// `None` for a session whose title attempt never resolved (missing
    /// narrator, failed call) or predates P552.
    title: Option<String>,
    /// Full lifecycle drives the attached title row; `title` remains the
    /// resolved-only dashboard list label.
    title_state: Option<ctx_traits_core::procedure::session::SessionTitleState>,
    /// The resolved trait's own name and the ledger's `started_at_epoch`,
    /// carried so the attached four-pane body can render the same
    /// `<bold title> · <trait name> · Started at <HH:MM:SS>` row a live run
    /// shows — `None` only when trait resolution itself failed (`degraded`
    /// already names that reason).
    trait_name: Option<String>,
    started_at_epoch: Option<u64>,
    /// Set when trait resolution failed, so `progress_lines`/`journey_lines`
    /// are the plain-ledger fallback instead of the reconstructed run view —
    /// an honest reason string, never a crash or fabricated content (§3.2
    /// degrade path). P552 review `dashboard-attach-contract-absent`: kept
    /// separate from [`Self::activity_degraded`] so the two compose for
    /// presentation instead of one silently overwriting the other — a
    /// trait-reconstruction failure and a partially-corrupt sidecar can both
    /// be true of the same ledger at once.
    trait_degraded: Option<String>,
    /// Set when this ledger has no P521 activity sidecar at all, or the
    /// sidecar's tolerant reader had to skip corrupt lines — `history`/
    /// `current` are empty (or partial) and this names why, independent of
    /// whether trait resolution itself also failed. See
    /// [`Self::trait_degraded`].
    activity_degraded: Option<String>,
    /// P552 review `dashboard-attach-contract-absent`: whether this ledger
    /// has a P521 activity sidecar at all. Deliberately independent of
    /// `history`/`current`'s own contents (a
    /// current-only sidecar with no completed step is still available) and
    /// of `trait_degraded`/`activity_degraded` (a trait-reconstruction
    /// fallback with no sidecar read at all must not be conflated with
    /// "sidecar exists but is partially corrupt").
    activity_available: bool,
    /// Durable history evidence is distinct from a current-only sidecar.
    history_available: bool,
    /// P552 review `dashboard-attach-contract-absent`: true once the
    /// ledger's own persisted `Status` reports a finished drive
    /// (`Completed`/`Failed`) — the signal `apply_snapshot` uses to swap an
    /// attached four-pane body for the existing P550 story view instead of
    /// continuing to redraw panes for a run that is no longer advancing.
    terminal: bool,
}

/// Identity the renderer asks the IO worker to refresh. The renderer only
/// creates this from its selected or attached row; all filesystem work remains
/// on the worker thread.
#[derive(Clone)]
struct SessionPreviewRequest {
    session_id: String,
    ledger_path: camino::Utf8PathBuf,
    run_id: String,
}

/// P081: `State::attach_request`'s payload — exactly the identity
/// `run_with_initial_session`'s attach loop needs to build the observer
/// (`load_trait_for_session`/`plan_procedure_run` both key off the session
/// id's ledger, never a list index).
#[derive(Clone)]
struct AttachRequest {
    session_id: String,
    ledger_path: camino::Utf8PathBuf,
}

/// The single-keypress action model's tag (P469 §3.3, generalized by P471
/// §"one shared abstraction"): every tag carries the target's identity,
/// never its list index, so a reload that reorders or removes rows never
/// misapplies an action to the wrong row — the handler re-looks-up by id and
/// re-checks eligibility before acting. One `ModalHost<Action>` backs every
/// screen's actions; `Exit` stays screen-agnostic at the top level.
#[derive(Clone)]
enum Action {
    Exit,
    Session(SessionAction),
    Trait(TraitAction),
    Merge(MergeAction),
    Task(TaskAction),
    /// Acknowledges a failed attach (P081/0145): the observer never took the
    /// terminal, so there is nothing to confirm or roll back — dismissing
    /// this modal is the only outcome, for either key the `Confirm` widget
    /// accepts.
    AttachFailed,
}

/// TASKS' `S`/`a` write tags (0063): the modal itself carries the typed text
/// (a child title for split, `done`/`cancelled` for archive), resolved to
/// `ModalOutcome::Submitted` on confirm. Dispatch (`d`) never opens a
/// `Task` modal — a blocked task refuses inline, and a permitted dispatch
/// reuses `SessionAction::Spawn` unchanged.
#[derive(Clone)]
enum TaskAction {
    Split {
        parent: String,
    },
    Archive {
        key: String,
        digest: String,
    },
    /// `e`: status + relations, the only surface the task authorizes
    /// beyond archive (prose editing stays CLI-only). `digest` is
    /// captured at modal-open — the snapshot the write is validated
    /// against.
    Edit {
        key: String,
        digest: String,
    },
    /// `y`: accept a merge-time done-proposal (0063.8). `digest` is captured
    /// at modal-open, same stale-write discipline as `Archive`/`Edit`;
    /// `evidence` is every merged bound run the proposal cited, carried
    /// through so the accept can fold it into `origin` and the reported
    /// result.
    MarkDone {
        key: String,
        digest: String,
        evidence: Vec<super::task_proposals::MergedRunEvidence>,
    },
    /// `R`: one step of the reconcile review queue (0064). `digest` is
    /// captured at modal-open against the proposal's own task (`task_key`
    /// for `MarkDone`, `from` for `RemoveDependsOn`) — same stale-write
    /// discipline as every other write. Reject (`Cancelled`) and accept
    /// (`Confirmed`) alike advance to the next queued proposal.
    ReconcileStep {
        proposal: super::task_proposals::ReconcileProposal,
        digest: String,
    },
    /// `S` when the selected task's latest bound blocked run carries a park
    /// report or an oversized feasibility verdict: one step of the
    /// split-from-park-report queue. `Cancelled` skips this child and
    /// advances; `Confirmed` creates it under `parent`.
    SplitStep {
        parent: String,
        child: PendingSplitChild,
    },
}

/// One split child proposed from a park report's open blocker (or an
/// oversized feasibility verdict's `missing` entry), pending the owner's
/// individual confirmation (0064).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSplitChild {
    title: String,
    content: String,
    validation: String,
    steps: Vec<ctx_traits_core::task::Step>,
}

/// `a`'s modal grammar: `done` or `cancelled` (`canceled` too), optionally
/// followed by `release` to opt into the dependents sweep (0063.6) —
/// `"done release"`, `"cancelled release"`. Case-insensitive.
fn parse_task_archive_input(text: &str) -> Result<(TaskDocStatus, bool), String> {
    let mut words = text.split_whitespace();
    let status = match words.next().map(str::to_ascii_lowercase).as_deref() {
        Some("done") => TaskDocStatus::Done,
        Some("cancelled") | Some("canceled") => TaskDocStatus::Cancelled,
        _ => return Err("type done or cancelled".to_string()),
    };
    let release_dependents =
        words.next().map(str::to_ascii_lowercase).as_deref() == Some("release");
    Ok((status, release_dependents))
}

/// The three forms `e`'s modal grammar accepts, parsed by
/// [`parse_task_edit_input`]: `status <ready|done|cancelled>`,
/// `dep +<key>` / `dep -<key>`, and `dep <old> <new>` (re-point, one
/// `TaskUpdate` call per the recorded ruling). Anything else is a parse
/// error reported verbatim in `state.message`.
fn parse_task_edit_input(text: &str) -> Result<TaskUpdate, String> {
    let mut parts = text.split_whitespace();
    let verb = parts
        .next()
        .ok_or_else(|| "empty — try `status ready` or `dep +<key>`".to_string())?;
    match verb {
        "status" => {
            let value = parts
                .next()
                .ok_or_else(|| "status needs a value: ready, done, or cancelled".to_string())?;
            let status = match value.to_ascii_lowercase().as_str() {
                "ready" => TaskDocStatus::Ready,
                "done" => TaskDocStatus::Done,
                "cancelled" | "canceled" => TaskDocStatus::Cancelled,
                other => return Err(format!("unknown status {other:?}")),
            };
            Ok(TaskUpdate {
                status: Some(status),
                ..Default::default()
            })
        }
        "dep" => {
            let first = parts
                .next()
                .ok_or_else(|| "dep needs +<key>, -<key>, or <old> <new>".to_string())?;
            if let Some(key) = first.strip_prefix('+') {
                if key.is_empty() {
                    return Err("dep +<key> needs a key".to_string());
                }
                Ok(TaskUpdate {
                    add_depends_on: vec![key.to_string()],
                    ..Default::default()
                })
            } else if let Some(key) = first.strip_prefix('-') {
                if key.is_empty() {
                    return Err("dep -<key> needs a key".to_string());
                }
                Ok(TaskUpdate {
                    remove_depends_on: vec![key.to_string()],
                    ..Default::default()
                })
            } else {
                let new = parts
                    .next()
                    .ok_or_else(|| format!("dep {first} needs a second key to re-point to"))?;
                Ok(TaskUpdate {
                    remove_depends_on: vec![first.to_string()],
                    add_depends_on: vec![new.to_string()],
                    ..Default::default()
                })
            }
        }
        other => Err(format!(
            "unknown form {other:?} — try `status ready`, `dep +<key>`, `dep -<key>`, or `dep <old> <new>`"
        )),
    }
}

#[derive(Clone)]
enum SessionAction {
    Kill(String),
    Resume(String),
    Delete(String, DeletePlan),
    Answer {
        session_id: String,
        state_digest: String,
        target: String,
        schema_ref: Option<String>,
    },
    /// `n`'s spawn request (P506 §3.5): the modal itself carries the typed
    /// argument text, resolved to `ModalOutcome::Submitted` on confirm — no
    /// captured payload needed on the tag itself.
    Spawn,
}

/// TRAITS'/TRUST's one shared identity-bound trust-write tag (P471 §4.4,
/// unified for N members by P473 §4.7): `members` is 1..N `(trait_id,
/// digest-captured-when-the-modal-opened)` pairs — TRAITS' `a`/`b` and
/// TRUST's `a`/`b` each pass exactly one; TRUST's `A` (approve/block the
/// whole `metadata.family`) passes every member. Every member is re-checked
/// against `state.trust` before any write, and the whole set aborts naming
/// the offender if any one member's digest moved since the modal opened —
/// so a reload racing an open modal never gets silently (partially)
/// applied.
#[derive(Clone)]
enum TraitAction {
    Trust {
        /// The trait id (single write) or family name (block write), for the
        /// confirmation message.
        label: String,
        members: Vec<(String, String)>,
        verdict: ctx_traits_io::trust::TrustState,
    },
}

/// MERGES' own actions (P472 §3.5): `Retry` re-runs `merge::merge` for the
/// selected run (standard or `--deep`); `PrintPath` writes the run's
/// worktree path into the footer message; `Drop` reuses SESSIONS' own
/// `DeletePlan`/`plan_delete`/`execute_delete` mechanism unchanged, so there
/// is exactly one destructive artifact-removal path in the dashboard.
#[derive(Clone)]
enum MergeAction {
    Retry {
        run_id: String,
        deep: bool,
    },
    Drop {
        session_id: String,
        plan: DeletePlan,
    },
}

/// The exact artifact list a DELETE confirms before touching anything, and a
/// pure function of already-resolved inputs (§3.4): the caller does the git
/// probing (`plan_delete_for_ledger`) and passes the resolved worktree location
/// in, so this function itself does no IO and is directly unit-testable.
#[derive(Clone)]
struct DeletePlan {
    ledger_path: camino::Utf8PathBuf,
    driver_lock_path: Option<camino::Utf8PathBuf>,
    sidecars_root: Option<camino::Utf8PathBuf>,
    /// `(repo_root, worktree_path, branch)`, present only when provenance
    /// named a worktree AND its registration was verified in the current
    /// repository.
    worktree: Option<(camino::Utf8PathBuf, camino::Utf8PathBuf, String)>,
    /// Explains why a provenance-named worktree/branch is NOT in `worktree`
    /// above (foreign repository, or registration failed) — rendered
    /// verbatim in the confirm modal so it never implies a cleaner sweep
    /// than what will actually happen.
    worktree_note: Option<String>,
}

impl DeletePlan {
    fn artifact_lines(&self) -> Vec<String> {
        let mut lines = vec![format!("ledger: {}", self.ledger_path)];
        if let Some(path) = &self.driver_lock_path {
            lines.push(format!("driver lock: {path}"));
        }
        if let Some(root) = &self.sidecars_root {
            lines.push(format!("sidecars: {root}"));
        }
        if let Some((_, path, branch)) = &self.worktree {
            lines.push(format!("worktree: {path}"));
            lines.push(format!("branch: {branch}"));
        }
        if let Some(note) = &self.worktree_note {
            lines.push(note.clone());
        }
        lines
    }
}

struct State {
    screen: Screen,
    sessions: Vec<SessionRow>,
    /// The SESSIONS list actually rendered/navigated (P506 §3.1): headers
    /// plus session rows, rebuilt by [`rebuild_visible_sessions`] on every
    /// reload and every collapse toggle. [`selected_session`] is the only
    /// accessor any action key reads a row through.
    sessions_visible: Vec<VisibleRow>,
    /// A live-view handoff identity pinned through a possible live-to-terminal
    /// regrouping; resolving by identity survives asynchronous snapshots.
    initial_session_id: Option<String>,
    /// Which [`SessionGroup`]s are collapsed to their count-row form.
    /// Everything except `Live` starts collapsed.
    collapsed_groups: HashSet<SessionGroup>,
    traits: Vec<TraitRow>,
    merges: Vec<MergeRow>,
    /// Complete security-visible trust history. `trust_visible` is only the
    /// list projection; hiding orphans never removes them from this set.
    trust: Vec<TrustRow>,
    /// Backing indices for the TRUST list. Orphans start collapsed but remain
    /// reachable with `o`; every selection consumer goes through
    /// [`selected_trust`] to avoid mixing visible and backing index spaces.
    trust_visible: Vec<usize>,
    show_trust_orphans: bool,
    /// Per-screen list scroll/selection (P506 §3.7): switching screens no
    /// longer resets the OTHER screens' position, so leaving and returning
    /// to a screen restores exactly where the user left it.
    list_sessions: ScrollList,
    list_traits: ScrollList,
    list_merges: ScrollList,
    list_trust: ScrollList,
    list_tasks: ScrollList,
    /// TASKS' board cache (0063; freshness automated by 0063.7):
    /// renderer-owned, never part of [`DashboardSnapshot`]/the worker — the
    /// polled session inventory and the task board are two caches with two
    /// cadences, and keeping them structurally separate is what makes "as
    /// of" mean only the board. Populated at startup from the persisted
    /// snapshot cache (or one synchronous live read if the cache misses),
    /// kept fresh by a stat-sweep on the existing 2s tick, and still
    /// forceable with `s`. `None` only when startup's own read also failed —
    /// see `tasks_refresh_error` for why.
    tasks_board: Option<TasksBoardSnapshot>,
    /// Set whenever a board re-read (tick-triggered or `s`) fails; cleared
    /// on the next successful read. The cached board (if any) stays
    /// rendered regardless — this only adds the failure note to the title.
    tasks_refresh_error: Option<String>,
    /// Override for the persisted snapshot's cache root (0063.7). `None` in
    /// production, resolving to the real per-repository cache root; tests
    /// set this to a scratch directory so `cargo test` never touches
    /// `~/.config/ctx/cache`.
    tasks_cache_root: Option<camino::Utf8PathBuf>,
    /// TASKS' visible list projection (group headers + task rows), mirroring
    /// [`State::sessions_visible`]/[`rebuild_visible_sessions`] — rebuilt on
    /// every sync and every collapse toggle, never in the draw path.
    tasks_visible: Vec<TaskVisibleRow>,
    /// Which [`TaskGroup`]s are collapsed to their count-row form. Nothing
    /// starts collapsed — an empty board cache means an empty list either
    /// way.
    collapsed_task_groups: HashSet<TaskGroup>,
    task_preview: Option<TaskPreview>,
    /// 0063.8: derived done-proposals, keyed by task key — rebuilt inside
    /// `rebuild_visible_tasks` from `task_session_join` + `tasks_board`, same
    /// as `tasks_visible`. Never in the draw path, discarded with `State` on
    /// exit; nothing about a proposal persists between looks.
    task_proposals: HashMap<String, super::task_proposals::DoneProposal>,
    /// 0064: the reconcile pass's remaining proposal queue, one confirm
    /// modal at a time — `R` builds a fresh
    /// [`super::task_proposals::ReconcileReport`] and populates this;
    /// [`open_next_reconcile_step`] pops the front on every resolution
    /// (accept or reject alike) until empty. Never persisted beyond the
    /// pass, same as `task_proposals`.
    reconcile_queue: Vec<super::task_proposals::ReconcileProposal>,
    /// 0064: the ambiguous findings from the last `R` pass, surfaced in the
    /// completion message once the queue drains.
    reconcile_ambiguous: Vec<super::task_proposals::AmbiguousFinding>,
    /// 0064: the split-from-park-report queue — one confirm modal per open
    /// blocker (or oversized feasibility `missing` entry), stepped through
    /// the same way as `reconcile_queue`.
    split_queue: Vec<PendingSplitChild>,
    message: Option<String>,
    quit: bool,
    /// SESSIONS scope (P439): current repository/ad-hoc invocation only
    /// (default, byte-identical to pre-P439 behavior) or every indexed
    /// repository/ad-hoc identity machine-wide, toggled with `v`.
    all_repos: bool,
    /// Pane focus for the CURRENT screen (P506 §3.3): rebuilt with that
    /// screen's own leaf ids on every [`State::switch_screen`], since the
    /// leaf id set differs per screen. `Tab`/`Shift-Tab` stay on the screen
    /// tabs (unlike the `tui-demo` proof); this ring moves on `alt`+arrows
    /// and the retained `Enter`/`Esc` pair (enter -> preview pane, esc ->
    /// list pane) — a deliberate divergence from `tui_demo`'s own binding,
    /// preserving every dashboard muscle-memory key (P506 §3.3 risk 5).
    focus: FocusRing,
    /// The last-resolved pane rects for the current screen, cached so
    /// `alt`+arrow directional movement reads the SAME rects a frame just
    /// drew rather than re-resolving the tree outside a draw pass (mirrors
    /// `tui_demo::DemoState::last_pane_layout`).
    last_pane_layout: PaneLayoutResult,
    /// Per-pane scroll state for every non-list (preview/progress/narration)
    /// pane across all four screens — one map, keyed by [`PaneId`], replacing
    /// the four separate `ViewportScroll` fields the pre-pane-tree preview
    /// structs each carried (P506 §3.7).
    pane_scrolls: PaneScrolls,
    session_preview: Option<AttachedView>,
    /// P081: a live SESSIONS row's Enter/`s`(resume) request, recorded here
    /// and picked up by `run_with_initial_session`'s own loop (never acted on
    /// inside `handle_key` itself — the attach loop tears down and rebuilds
    /// this process's OWN terminal pane, which `draw_screen`'s borrowed
    /// `&mut RatatuiPane` cannot do). `None` on every ordinary draw — attach
    /// is a synchronous handoff to `run_view::RunPanel::new_observer`, never
    /// a persisted "attached" mode this struct's own fields describe.
    attach_request: Option<AttachRequest>,
    /// Attachment explicitly follows live trace updates until the user scrolls
    /// away from the tail; list-focused previews stay top-aligned. P552
    /// review `dashboard-attach-contract-absent`: one bool per attached pane
    /// (not one shared flag) so paging any single pane releases only that
    /// pane's follow state instead of all four together.
    session_progress_follow: bool,
    session_journey_follow: bool,
    session_history_follow: bool,
    session_current_follow: bool,
    trait_preview: Option<TraitPreview>,
    trait_explanation: Option<(String, String, Result<String, String>, std::time::Instant)>,
    merge_preview: Option<MergePreview>,
    trust_preview: Option<TrustPreview>,
    /// Current repository's run inventory, projected to the cheap identity/
    /// digest/recency facts [`run_sighting`] needs (§4.4) — captured once per
    /// reload, before [`merges_from_inventory`] consumes the owning scan by
    /// value, so a TRUST preview rebuild never re-scans the ledger store.
    run_sightings: Vec<RunSightingRow>,
    modal_host: ModalHost<Action>,
    /// TRUST's block-approve mark set (P506 §3.6), keyed by trait id — never
    /// by list index, since TRUST reloads and re-sorts every 2s.
    trust_marks: MarkSet<String>,
    /// Parse cache, held across ticks so an unchanged ledger is a
    /// refcount bump instead of a full re-parse. One-shot callers elsewhere
    /// (`stats`, `dispatch_preflight`, `run_queue`) allocate their own
    /// throwaway cache instead — this is the one long-lived instance.
    inventory_cache: ctx_traits_io::run_session::InventoryCache,
    /// Count of [`State::reload`] calls, used only to pace the periodic full
    /// liveness sweep (§3.3) — never displayed.
    reload_ticks: u64,
    /// Most recent [`State::reload`] wall-clock duration (§5), surfaced in
    /// the SESSIONS border title only above [`RELOAD_WARN_THRESHOLD`].
    reload_duration: Option<Duration>,
    worker: Option<worker::Handle>,
    /// True after the renderer has accepted one complete worker snapshot.
    /// Refreshes after that point retain the existing view while work runs.
    has_snapshot: bool,
    loading: bool,
    /// The most recent refresh failure. It remains visible until a later
    /// complete snapshot arrives, without replacing the prior snapshot.
    refresh_error: Option<String>,
    /// P550 `S`-key story view: set while the SESSIONS row's story is open,
    /// rendered by `draw_screen` as a single full-area lines pane in place of
    /// the screen's tree. Read-only and never advances a run — a keypress
    /// must not spend tokens, so this always resolves the free level.
    story_view: Option<StoryView>,
    /// P552 review `live-run-pane-contract-absent`: while attached, pane-cycle
    /// (`Tab`/`BackTab`) and scroll keys are queued here and drained by the
    /// shared [`run_view::render_pane_body`] itself — the SAME mechanism a
    /// live run's own `RunPanel` uses — rather than dashboard recomputing
    /// scroll geometry from a second, independently maintained content-length
    /// table that can drift from what the shared renderer actually drew.
    /// Always empty while the SESSIONS list is visible; `Tab`/`BackTab`
    /// switch dashboard screens in that state instead.
    pending_keys: Vec<crossterm::event::KeyEvent>,
    /// Process-local guide state received only from a live-view handoff.
    guide_chat: Option<run_view::GuideChatHandle>,
    /// The only attached session allowed to use the process-local handoff.
    /// Retaining this across detach permits an explicit reattach, but never
    /// leaks the originating run's context into the list or another session.
    guide_chat_session_id: Option<String>,
}

/// P550 dashboard `S`-key state: a snapshot of one session's story, built
/// synchronously from local ledger + best-effort plan reads at keypress
/// time (never a live view — a still-running session's story is exactly
/// that: a snapshot, stated in its own disposition line).
struct StoryView {
    session: ctx_traits_core::procedure::session::Session,
    report: ctx_traits_core::procedure::story::StoryReport,
    title: String,
    scroll: tui_kit::ViewportScroll,
}

/// Immutable IO-derived data transferred from the worker to the renderer.
/// UI focus, selection, modal text, marks, and scroll state deliberately stay
/// out of this value.
struct DashboardSnapshot {
    sessions: Vec<SessionRow>,
    traits: Vec<TraitRow>,
    merges: Vec<MergeRow>,
    trust: Vec<TrustRow>,
    run_sightings: Vec<RunSightingRow>,
    reload_duration: Option<Duration>,
    session_preview: Option<AttachedView>,
}

impl DashboardSnapshot {
    fn from_state(state: &State) -> Self {
        Self {
            sessions: state.sessions.clone(),
            traits: state.traits.clone(),
            merges: state.merges.clone(),
            trust: state.trust.clone(),
            run_sightings: state.run_sightings.clone(),
            reload_duration: state.reload_duration,
            session_preview: state.session_preview.clone(),
        }
    }
}

impl State {
    fn new() -> Self {
        let mut state = Self::new_without_worker_for_session(None);
        state.worker = Some(worker::Handle::new());
        load_tasks_board_at_startup(&mut state);
        state
    }

    fn new_for_session_with_guide(
        session_id: String,
        guide_chat: Option<run_view::GuideChatHandle>,
    ) -> Self {
        let mut state =
            Self::new_without_worker_for_session_with_guide(Some(session_id), guide_chat);
        state.worker = Some(worker::Handle::new());
        load_tasks_board_at_startup(&mut state);
        state
    }

    /// The worker owns the only mutable IO model. The render state never uses
    /// this constructor, so it cannot accidentally scan stores while drawing.
    fn new_without_worker() -> Self {
        Self::new_without_worker_for_session(None)
    }

    fn new_without_worker_for_session(initial_session_id: Option<String>) -> Self {
        Self::new_without_worker_for_session_with_guide(initial_session_id, None)
    }

    fn new_without_worker_for_session_with_guide(
        initial_session_id: Option<String>,
        guide_chat: Option<run_view::GuideChatHandle>,
    ) -> Self {
        // Everything except `live` starts collapsed: the one section that
        // means "moving right now" is the one worth opening on arrival.
        let mut collapsed_groups = HashSet::new();
        collapsed_groups.extend([
            SessionGroup::Resumable,
            SessionGroup::Pending,
            SessionGroup::Failed,
            SessionGroup::Completed,
        ]);
        let guide_chat_session_id = guide_chat.as_ref().and_then(|_| initial_session_id.clone());
        Self {
            screen: Screen::Sessions,
            sessions: Vec::new(),
            sessions_visible: Vec::new(),
            initial_session_id,
            collapsed_groups,
            traits: Vec::new(),
            merges: Vec::new(),
            trust: Vec::new(),
            trust_visible: Vec::new(),
            show_trust_orphans: false,
            list_sessions: ScrollList::new(),
            list_traits: ScrollList::new(),
            list_merges: ScrollList::new(),
            list_trust: ScrollList::new(),
            list_tasks: ScrollList::new(),
            tasks_board: None,
            tasks_refresh_error: None,
            tasks_cache_root: None,
            tasks_visible: Vec::new(),
            collapsed_task_groups: HashSet::new(),
            task_preview: None,
            task_proposals: HashMap::new(),
            reconcile_queue: Vec::new(),
            reconcile_ambiguous: Vec::new(),
            split_queue: Vec::new(),
            message: None,
            quit: false,
            all_repos: false,
            focus: FocusRing::new(vec![PANE_SESSIONS_LIST]),
            last_pane_layout: PaneLayoutResult::default(),
            pane_scrolls: PaneScrolls::new(),
            session_preview: None,
            attach_request: None,
            session_progress_follow: false,
            session_journey_follow: false,
            session_history_follow: false,
            session_current_follow: false,
            trait_preview: None,
            trait_explanation: None,
            merge_preview: None,
            trust_preview: None,
            run_sightings: Vec::new(),
            modal_host: ModalHost::new(),
            trust_marks: MarkSet::new(),
            inventory_cache: ctx_traits_io::run_session::InventoryCache::new(),
            reload_ticks: 0,
            reload_duration: None,
            worker: None,
            has_snapshot: false,
            loading: false,
            refresh_error: None,
            story_view: None,
            pending_keys: Vec::new(),
            guide_chat,
            guide_chat_session_id,
        }
    }

    fn current_list(&self) -> &ScrollList {
        match self.screen {
            Screen::Sessions => &self.list_sessions,
            Screen::Traits => &self.list_traits,
            Screen::Merges => &self.list_merges,
            Screen::Trust => &self.list_trust,
            Screen::Tasks => &self.list_tasks,
        }
    }

    fn current_list_mut(&mut self) -> &mut ScrollList {
        match self.screen {
            Screen::Sessions => &mut self.list_sessions,
            Screen::Traits => &mut self.list_traits,
            Screen::Merges => &mut self.list_merges,
            Screen::Trust => &mut self.list_trust,
            Screen::Tasks => &mut self.list_tasks,
        }
    }

    fn selected(&self) -> usize {
        self.current_list().selected()
    }

    /// Sets all four list-visible-preview follow flags together — used only
    /// at selection/reset boundaries. Paging a single pane afterward
    /// releases only that pane's own flag, never the other three.
    fn set_session_follow_all(&mut self, follow: bool) {
        self.session_progress_follow = follow;
        self.session_journey_follow = follow;
        self.session_history_follow = follow;
        self.session_current_follow = follow;
    }

    /// Switches to `screen`, resetting the pane focus ring to that screen's
    /// list pane (P506 §3.3). The ring is reconciled against the real,
    /// width-resolved tree on the very next draw (`draw_screen`), so
    /// starting it here from a hypothetical widest-possible tree is neither
    /// needed nor safe — see `FocusRing::reconcile`. Deliberately does NOT
    /// touch any screen's own `ScrollList` — per-screen scroll/selection
    /// persists across a switch (§3.7).
    fn switch_screen(&mut self, screen: Screen) {
        if self.screen == Screen::Sessions && screen != Screen::Sessions {
            self.set_session_follow_all(false);
        }
        self.screen = screen;
        self.focus = FocusRing::new(vec![list_pane_id(screen)]);
        self.reload();
    }

    fn move_selection(&mut self, delta: i32) {
        // The visible-row window is only known at draw time (the pane
        // area's height varies with the terminal), so key handling only
        // moves the selection; the render pass re-clamps the scroll offset
        // against the real rect height on the next render.
        self.current_list_mut().move_by(delta as i64, usize::MAX);
        // Navigation is an explicit user choice, so later snapshots must not
        // snap the list back to the run-view handoff target.
        if self.screen == Screen::Sessions {
            self.initial_session_id = None;
        }
    }

    fn reload(&mut self) {
        if let Some(worker) = &self.worker {
            worker.refresh(self.all_repos, self.screen, self.session_preview_request());
            self.loading = true;
        }
    }

    /// P081: always the currently-selected row's own request — attach is now
    /// a synchronous handoff (see [`AttachRequest`]) rather than a persisted
    /// dashboard mode, so there is no longer a second "attached" identity to
    /// prefer over the list selection.
    fn session_preview_request(&self) -> Option<SessionPreviewRequest> {
        if self.screen != Screen::Sessions {
            return None;
        }
        let row = selected_session(self)?;
        row.class.can_attach().then(|| SessionPreviewRequest {
            session_id: row.session_id.clone(),
            ledger_path: row.ledger_path.clone(),
            run_id: row.run_id.clone(),
        })
    }

    fn apply_snapshots(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        for result in worker.explanation_results() {
            if self.traits.get(self.selected()).is_some_and(|row| {
                row.id == result.trait_id && row.canonical_digest == result.canonical_digest
            }) {
                if let (Some(preview), Ok(explanation)) = (&mut self.trait_preview, &result.result)
                {
                    preview.lines.push(RLine::from(""));
                    preview.lines.push(RLine::styled(
                        "generated explanation (advisory)",
                        Style::default().add_modifier(Modifier::BOLD),
                    ));
                    preview.lines.extend(
                        explanation
                            .lines()
                            .map(|line| RLine::from(line.to_string())),
                    );
                }
                self.trait_explanation = Some((
                    result.trait_id,
                    result.canonical_digest,
                    result.result,
                    std::time::Instant::now(),
                ));
            }
        }
        self.apply_refresh_results(worker.refresh_results());
    }

    fn apply_refresh_results(&mut self, results: impl IntoIterator<Item = worker::RefreshResult>) {
        let mut latest_snapshot = None;
        let mut trailing_error = None;
        for result in results {
            match result {
                Ok(snapshot) => {
                    latest_snapshot = Some(snapshot);
                    // A newer complete snapshot recovers from older errors.
                    trailing_error = None;
                }
                Err(error) => trailing_error = Some(error),
            }
        }
        if let Some(snapshot) = latest_snapshot {
            self.apply_snapshot(&snapshot);
        }
        if let Some(error) = trailing_error {
            self.loading = false;
            self.refresh_error = Some(error);
        }
    }

    fn apply_snapshot(&mut self, snapshot: &DashboardSnapshot) {
        let selected = selected_visible_row(self);
        self.sessions = snapshot.sessions.clone();
        self.traits = snapshot.traits.clone();
        self.merges = snapshot.merges.clone();
        self.trust = snapshot.trust.clone();
        self.run_sightings = snapshot.run_sightings.clone();
        self.reload_duration = snapshot.reload_duration;
        self.loading = false;
        self.has_snapshot = true;
        self.refresh_error = None;
        rebuild_visible_sessions(self);
        restore_visible_selection(self, selected);
        resolve_initial_session(self);
        rebuild_visible_trust(self);
        let trust_ids: Vec<String> = self
            .trust
            .iter()
            .filter_map(|row| row.trait_id.clone())
            .collect();
        self.trust_marks.retain_existing(&trust_ids);
        self.list_traits.set_len(self.traits.len());
        self.list_merges.set_len(self.merges.len());
        // P081: attach is a synchronous handoff, never a persisted mode this
        // reload path has to keep re-pointing at (see [`AttachRequest`]) — the
        // list-visible preview always tracks the current selection.
        if self.screen == Screen::Sessions {
            if self.session_preview.as_ref().is_some_and(|preview| {
                selected_session(self).is_none_or(|row| row.session_id != preview.session_id)
            }) {
                self.session_preview = None;
            }
            if let Some(preview) = &snapshot.session_preview
                && session_preview_matches_current(self, &preview.session_id)
            {
                follow_session_preview(
                    state_pane_scroll_rows(self, PANE_SESSIONS_PROGRESS),
                    self.pane_scrolls.get_mut(PANE_SESSIONS_PROGRESS),
                    self.session_progress_follow,
                    preview.progress_lines.len(),
                );
                self.session_preview = Some(preview.clone());
            }
        }
        // The session-inventory overlay (in-flight/parked) refreshes on this
        // same 2s cadence against whatever board snapshot is currently
        // captured — automated by 0063.7's own tick sweep, not this one — a
        // `None` cache (startup read also failed) rebuilds to nothing, same
        // as `rebuild_visible_tasks` does on its own.
        if self.tasks_board.is_some() {
            rebuild_visible_tasks(self);
            if self.screen == Screen::Tasks {
                refresh_task_preview_for_selection(self);
            }
        }
    }

    /// Worker-only inventory refresh. It is intentionally separate from
    /// [`State::reload`], whose render-side contract is channel-only.
    fn reload_sync(&mut self) -> crate::Result<()> {
        let reload_started = std::time::Instant::now();
        // Bound this tick's driver-lock probes to the local
        // liveness index's own rows (typically a handful), except on a
        // periodic full sweep, which also probes every other row to catch a
        // row-less held lock (adoption: an externally started driver, or one
        // predating this index). `reload_ticks` is bumped once per call
        // regardless of scope, so the cadence is wall-clock-uniform whether
        // or not `all_repos` is toggled mid-session.
        self.reload_ticks = self.reload_ticks.wrapping_add(1);
        let full_sweep = self.reload_ticks.is_multiple_of(FULL_SWEEP_EVERY_TICKS);
        let indexed_ids = ctx_traits_io::run_liveness::indexed_session_ids(
            &ctx_traits_io::run_control::runtime_root(),
        );
        // `None` means the liveness index itself is unavailable this tick:
        // fail OPEN (probe every row, exactly like a full sweep) rather than
        // fail closed on an empty probe set, which would silently render
        // every live session as not-live (unknown, never
        // a fabricated dead/not-live answer).
        let probe_budget = match &indexed_ids {
            Some(ids) if !full_sweep => ProbeBudget::IndexOnly(ids),
            _ => ProbeBudget::Sweep,
        };
        if self.all_repos {
            let machine_wide = ctx_traits_io::run_session::machine_wide_run_inventory_cached(
                &mut self.inventory_cache,
            )?;
            let mut sessions = Vec::new();
            let mut merges = Vec::new();
            for entry in machine_wide {
                sessions.extend(sessions_from_inventory_tagged(
                    &entry.rows,
                    Some(&entry.repo_key),
                    Some(&entry.repo_path),
                    &probe_budget,
                ));
                merges.extend(merges_from_inventory(entry.rows, Some(&entry.repo_path)));
            }
            // Each block above is already Live-first internally; a stable
            // sort over the concatenation makes "active first" hold
            // machine-wide rather than only within each repo's own block,
            // without disturbing the most-recently-modified order within
            // either class.
            sessions.sort_by_key(|row| {
                if row.class == SessionClass::Live {
                    0
                } else {
                    1
                }
            });
            self.sessions = sessions;
            self.merges = merges;
            // TRUST is machine-local (§5: no `all_repos` semantics), so its
            // run-sighting projection always comes from THIS repository's
            // inventory regardless of the `v` toggle — a second, separate
            // scan only in ALL mode. Shares the same cache: this repository's
            // rows were very likely already touched by the machine-wide scan
            // above, so this is typically all cache hits.
            let sighting_inventory = ctx_traits_io::run_session::current_repo_run_inventory_cached(
                &mut self.inventory_cache,
            )?;
            self.run_sightings = run_sighting_rows(&sighting_inventory);
        } else {
            let inventory = ctx_traits_io::run_session::current_repo_run_inventory_cached(
                &mut self.inventory_cache,
            )?;
            self.run_sightings = run_sighting_rows(&inventory);
            self.sessions = sessions_from_inventory_tagged(&inventory, None, None, &probe_budget);
            self.merges = merges_from_inventory(inventory, None);
        }
        rebuild_visible_sessions(self);
        if matches!(self.screen, Screen::Traits | Screen::Trust) {
            let (traits, trust) = load_traits_and_trust()?;
            self.traits = traits;
            self.trust = trust;
        }
        rebuild_visible_trust(self);
        let trust_ids: Vec<String> = self
            .trust
            .iter()
            .filter_map(|row| row.trait_id.clone())
            .collect();
        self.trust_marks.retain_existing(&trust_ids);
        self.list_traits.set_len(self.traits.len());
        self.list_merges.set_len(self.merges.len());
        if self.screen == Screen::Merges {
            refresh_merge_preview_for_selection(self);
        }
        if self.screen == Screen::Traits {
            refresh_trait_preview_for_selection(self);
        }
        if self.screen == Screen::Trust {
            refresh_trust_preview_for_selection(self);
        }
        // Measurement recorded unconditionally (cheap — one
        // `Instant::now()` diff) but only surfaced in the SESSIONS border
        // title above `RELOAD_WARN_THRESHOLD`, so a healthy steady state
        // shows nothing and a future regression has a permanent, honest
        // signal to page off of.
        self.reload_duration = Some(reload_started.elapsed());
        Ok(())
    }
}

/// Reveals and selects the handoff target only after applying an inventory
/// snapshot, so a run that completes during terminal handoff lands in its new
/// (normally collapsed) terminal group rather than a neighboring list row.
fn resolve_initial_session(state: &mut State) {
    let Some(session_id) = state.initial_session_id.clone() else {
        return;
    };
    let Some(index) = state
        .sessions
        .iter()
        .position(|row| row.session_id == session_id)
    else {
        return;
    };
    let row = &state.sessions[index];
    let group = session_group(row.class, row.status.as_ref(), row.outcome.as_ref());
    let terminal = row.class == SessionClass::Terminal;
    if state.collapsed_groups.remove(&group) {
        rebuild_visible_sessions(state);
    }
    if let Some(visible_index) = state.sessions_visible.iter().position(
        |row| matches!(row, VisibleRow::Session(session_index) if *session_index == index),
    ) {
        state.list_sessions.set_selected(visible_index);
        // A snapshot captured before the run finished can land after the
        // terminal handoff. Keep following that identity until a terminal
        // snapshot observes its regrouping; otherwise a retained numeric row
        // selection can land on a header or neighboring session next reload.
        if terminal {
            state.initial_session_id = None;
        }
    }
}

/// SESSIONS-screen rows, projected from one shared run inventory scan (also
/// consumed by [`merges_from_inventory`]) rather than each screen
/// re-scanning the session stores independently. `repo_key`/`repo_path` tag
/// every produced row with its repository/ad-hoc identity for ALL-mode
/// display and cwd-anchored git gating (P439); both `None` in the default
/// current-repository-only scope. Rows are ordered `Live` first, preserving
/// the inventory's existing most-recently-modified order within each class
/// (a stable sort over an already most-recently-modified-sorted input).
/// Pure `(live, status) -> SessionClass` mapping (§3.1), extracted out of
/// `sessions_from_inventory_tagged`'s inventory scan so the classification
/// table itself is directly unit-testable without an inventory scan, a
/// driver-lock probe, or any other IO. `Held` (any status) always wins as
/// `Live`; otherwise a readable ledger's own terminal/non-terminal `Status`
/// decides `Terminal` vs `Resumable`.
fn classify_session(
    live: bool,
    status: &ctx_traits_core::procedure::session::Status,
    outcome: Option<&ctx_traits_core::procedure::session::DriveOutcomeKind>,
) -> SessionClass {
    match ctx_traits_core::procedure::activity::SessionState::derive(status, outcome, live) {
        ctx_traits_core::procedure::activity::SessionState::Running => SessionClass::Live,
        state if state.is_terminal() => SessionClass::Terminal,
        _ => SessionClass::Resumable,
    }
}

/// Blocker 3 (P509): the dashboard lists sessions machine-wide, but a
/// relative `trait_source.path` resolves against the *dashboard's own* cwd
/// unless overridden. When the row carries an ALL-mode `repo_path`, join it
/// against that path and hand the caller an absolute override; otherwise
/// return `None` so the default-mode resolution (relative to the dashboard's
/// own cwd, which the default scope guarantees IS the session's repo) is
/// left untouched. Shared by every answer-path trait load (modal open, row
/// presentation, the Answer applier's `run::set`) so all three agree.
fn resolve_answer_trait_file(
    session: &ctx_traits_core::procedure::session::Session,
    repo_path: Option<&str>,
) -> Option<String> {
    let source_path = session.provenance.trait_source.as_ref()?.path.as_str();
    resolve_answer_trait_file_path(source_path, repo_path)
}

fn resolve_answer_trait_file_path(source_path: &str, repo_path: Option<&str>) -> Option<String> {
    let path = camino::Utf8Path::new(source_path);
    if path.is_absolute() {
        return None;
    }
    let repo_path = repo_path?;
    Some(camino::Utf8Path::new(repo_path).join(path).to_string())
}

/// Ask-specific row presentation is resolved once at inventory projection and
/// uses the same prompt helper as the answer modal.
fn parked_ask_presentation(
    session: &ctx_traits_core::procedure::session::Session,
    repo_path: Option<&str>,
) -> Option<String> {
    let frame = session.next_frame.as_ref()?;
    if session.status != ctx_traits_core::procedure::session::Status::WaitingOnHuman
        || frame.kind != ctx_traits_core::procedure::runtime::SequenceFrameKind::Ask
    {
        return None;
    }
    if matches!(
        session.last_drive_outcome.as_ref().map(|o| &o.outcome),
        Some(ctx_traits_core::procedure::session::DriveOutcomeKind::Interrupted)
    ) {
        return None;
    }
    let trait_file = resolve_answer_trait_file(session, repo_path);
    let loaded = ctx_traits_io::run::load_trait_for_session(
        trait_file.as_deref(),
        None,
        session,
        "dashboard",
    )
    .ok()?;
    let question = resolved_human_question_body(&loaded, session, frame)
        .ok()?
        .chars()
        .take(80)
        .collect::<String>();
    let wait = session
        .last_drive_outcome
        .as_ref()
        .filter(|outcome| {
            outcome.outcome == ctx_traits_core::procedure::session::DriveOutcomeKind::WaitingOnHuman
        })
        .map(|outcome| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(outcome.recorded_at_epoch)
        })
        .map(|seconds| tui::elapsed_text(Duration::from_secs(seconds)))
        .unwrap_or_else(|| "wait unknown".to_string());
    Some(format!("ask: {question} ({wait})"))
}

/// SESSIONS-screen grouping (P506 §3.1/§1.1): the owner's five buckets, in
/// the fixed order [`SessionGroup::order`] renders them. Sourced today from
/// [`SessionClass`]/`Status` behind the single [`session_group`] seam, so a
/// later P504 typed-session-state landing is a one-function change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum SessionGroup {
    Live,
    Resumable,
    Pending,
    Failed,
    Completed,
}

impl SessionGroup {
    /// The owner's fixed display order.
    fn order() -> [SessionGroup; 5] {
        [
            SessionGroup::Live,
            SessionGroup::Resumable,
            SessionGroup::Pending,
            SessionGroup::Failed,
            SessionGroup::Completed,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            SessionGroup::Live => "live",
            SessionGroup::Resumable => "resumable",
            SessionGroup::Pending => "pending",
            SessionGroup::Failed => "failed",
            SessionGroup::Completed => "completed",
        }
    }
}

/// Pure `(class, status) -> SessionGroup` mapping (P506 §3.1, §1.1), decided
/// in exactly one place. `status` is `None` only for [`SessionClass::Unreadable`]
/// (no ledger was ever parsed) — that case always lands in `Failed`.
///
/// A held driver is live regardless of the ledger's last-recorded status, and
/// that is the ONLY route into `Live`. An unheld `awaiting-agent-output` run
/// remains resumable: its ledger records interrupted in-flight work, not
/// current liveness.
fn session_group(
    class: SessionClass,
    status: Option<&ctx_traits_core::procedure::session::Status>,
    outcome: Option<&ctx_traits_core::procedure::session::DriveOutcomeKind>,
) -> SessionGroup {
    if class == SessionClass::Live {
        return SessionGroup::Live;
    }
    let Some(status) = status else {
        return SessionGroup::Failed;
    };
    if ctx_traits_core::procedure::activity::SessionState::derive(status, outcome, false)
        == ctx_traits_core::procedure::activity::SessionState::Cancelled
    {
        return SessionGroup::Failed;
    }
    match status {
        ctx_traits_core::procedure::session::Status::AwaitingAgentOutput => SessionGroup::Resumable,
        ctx_traits_core::procedure::session::Status::AwaitingInput
        | ctx_traits_core::procedure::session::Status::WaitingOnHuman => SessionGroup::Pending,
        ctx_traits_core::procedure::session::Status::Completed => SessionGroup::Completed,
        ctx_traits_core::procedure::session::Status::BlockedCommandPermissionRequired
        | ctx_traits_core::procedure::session::Status::BlockedAgentUnassigned
        | ctx_traits_core::procedure::session::Status::Rejected
        | ctx_traits_core::procedure::session::Status::Blocked
        | ctx_traits_core::procedure::session::Status::Failed => SessionGroup::Failed,
    }
}

/// One row of the SESSIONS screen's visible list once grouping (P506 §3.1)
/// breaks the "selection is a direct index into `sessions`" invariant: a
/// group header (which has no verb — Enter/space toggles its own collapse)
/// or a real session, addressed by its index into `State::sessions`. Rebuilt
/// on every reload and every collapse toggle by [`rebuild_visible_sessions`];
/// [`selected_session`] is the ONLY accessor every action key routes
/// through, so a second index-into-`sessions` read never regresses (P506
/// risk 1).
enum VisibleRow {
    GroupHeader {
        group: SessionGroup,
        count: usize,
        collapsed: bool,
    },
    Session(usize),
}

/// Stable selection identity across reloads. The rendered row index is not
/// stable because group headers and sessions can appear, disappear, or move.
enum VisibleSelection {
    Group(SessionGroup),
    Session(String),
}

/// Rebuilds [`State::sessions_visible`] from `state.sessions` and
/// `state.collapsed_groups` — called after every reload and every collapse
/// toggle, never in the draw path. Every display group emits a header,
/// including empty and collapsed groups.
fn rebuild_visible_sessions(state: &mut State) {
    let mut buckets: Vec<(SessionGroup, Vec<usize>)> = SessionGroup::order()
        .into_iter()
        .map(|g| (g, Vec::new()))
        .collect();
    for (idx, row) in state.sessions.iter().enumerate() {
        let group = session_group(row.class, row.status.as_ref(), row.outcome.as_ref());
        if let Some((_, indices)) = buckets.iter_mut().find(|(g, _)| *g == group) {
            indices.push(idx);
        }
    }
    let mut visible = Vec::new();
    for (group, indices) in buckets {
        let collapsed = state.collapsed_groups.contains(&group);
        visible.push(VisibleRow::GroupHeader {
            group,
            count: indices.len(),
            collapsed,
        });
        if !collapsed {
            visible.extend(indices.into_iter().map(VisibleRow::Session));
        }
    }
    state.sessions_visible = visible;
    state.list_sessions.set_len(state.sessions_visible.len());
}

/// Captures the logical row selected before replacing the inventory snapshot.
fn selected_visible_row(state: &State) -> Option<VisibleSelection> {
    match state.sessions_visible.get(state.list_sessions.selected())? {
        VisibleRow::GroupHeader { group, .. } => Some(VisibleSelection::Group(*group)),
        VisibleRow::Session(index) => state
            .sessions
            .get(*index)
            .map(|row| VisibleSelection::Session(row.session_id.clone())),
    }
}

/// Restores a snapshot-stable selection. A session that is no longer visible
/// resolves to its group header, never to an unrelated session at its old row.
fn restore_visible_selection(state: &mut State, selected: Option<VisibleSelection>) {
    let Some(selected) = selected else {
        return;
    };
    let target = match selected {
        VisibleSelection::Group(group) => state.sessions_visible.iter().position(
            |row| matches!(row, VisibleRow::GroupHeader { group: candidate, .. } if *candidate == group),
        ),
        VisibleSelection::Session(session_id) => {
            let session = state.sessions.iter().find(|row| row.session_id == session_id);
            state.sessions_visible.iter().position(|row| match (row, session) {
                (VisibleRow::Session(index), Some(session)) => {
                    state.sessions[*index].session_id == session.session_id
                }
                _ => false,
            })
            .or_else(|| {
                session.and_then(|row| {
                    let group = session_group(row.class, row.status.as_ref(), row.outcome.as_ref());
                    state.sessions_visible.iter().position(
                        |visible| matches!(visible, VisibleRow::GroupHeader { group: candidate, .. } if *candidate == group),
                    )
                })
            })
            .or_else(|| {
                state
                    .sessions_visible
                    .iter()
                    .position(|row| matches!(row, VisibleRow::GroupHeader { .. }))
            })
        }
    };
    if let Some(index) = target {
        state.list_sessions.set_selected(index);
    }
}

/// Rebuilds TRUST's list projection while retaining the complete trust history
/// in `State::trust`. Only classified orphan records are collapsed.
fn rebuild_visible_trust(state: &mut State) {
    state.trust_visible = state
        .trust
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            state.show_trust_orphans || row.class != trust_story::TrustClass::Orphaned
        })
        .map(|(index, _)| index)
        .collect();
    state.list_trust.set_len(state.trust_visible.len());
}

/// The only mapping from TRUST's visible selection to its backing record.
fn selected_trust(state: &State) -> Option<&TrustRow> {
    state
        .trust_visible
        .get(state.list_trust.selected())
        .and_then(|index| state.trust.get(*index))
}

fn toggle_trust_orphans(state: &mut State) {
    state.show_trust_orphans = !state.show_trust_orphans;
    rebuild_visible_trust(state);
    refresh_trust_preview_for_selection(state);
}

/// The ONLY accessor SESSIONS' action keys (`x`/`s`/`d`/Enter/attach/preview
/// refresh) route through (P506 §3.1, risk 1) — `None` when the cursor sits
/// on a group header (which has no verb) or the index is out of range.
/// Re-resolving the header/row split from the live selection, rather than
/// caching it, means a reload that changes which index the cursor lands on
/// is automatically safe: the existing "re-resolve by session id, never by
/// index" doctrine still governs every ACTION's own target lookup once
/// confirmed.
fn selected_session(state: &State) -> Option<&SessionRow> {
    match state.sessions_visible.get(state.list_sessions.selected())? {
        VisibleRow::Session(idx) => state.sessions.get(*idx),
        VisibleRow::GroupHeader { .. } => None,
    }
}

/// P550 `S`: opens the story view for the selected SESSIONS row. Loading is
/// synchronous — a ledger read plus a best-effort `load_plan` that degrades
/// to ledger-only enrichment on any failure (both local file reads) — and
/// NEVER makes a model call: the free `StoryLevel::Default` is the only
/// level a keypress can ever resolve to (`draw_screen`'s story branch, not
/// this function, is where that level is fixed).
fn open_story_view(state: &mut State) {
    let Some(row) = selected_session(state) else {
        state.message = Some("no session selected".to_string());
        return;
    };
    if row.run_id.is_empty() {
        state.message = Some("story: no run-id recorded for this session".to_string());
        return;
    }
    match build_story_view_from_ledger(&row.ledger_path.clone()) {
        Ok(view) => state.story_view = Some(view),
        Err(err) => state.message = Some(format!("story: {err}")),
    }
}

/// The `StoryView` builder factored out so [`open_story_view`]'s `S`
/// keypress and the P552 review `terminal-attach-story-identity-lost` fix
/// (an attached run becoming terminal) construct the exact same P550 story
/// instead of the attach path growing its own ending — and, critically,
/// from the SAME authoritative `ledger_path` both callers already hold
/// rather than a fresh `run_id` lookup through the current repository's
/// default session store, which a foreign-repository attachment (or a
/// same-run-id collision within the current store) can resolve to the wrong
/// session entirely.
fn build_story_view_from_ledger(ledger_path: &camino::Utf8Path) -> crate::Result<StoryView> {
    let session = ctx_traits_io::run_session::read_run_session(ledger_path)?;
    let plan = super::story::load_plan(&session);
    let activity = super::story::load_activity(ledger_path);
    let report =
        ctx_traits_core::procedure::story::build(&session, plan.as_ref(), activity.as_ref());
    let title = format!(
        "story · {} · {}",
        session.run_id.as_str(),
        super::story::disposition_sentence(&session, &report)
    );
    Ok(StoryView {
        session,
        report,
        title,
        scroll: tui_kit::ViewportScroll::new(),
    })
}

/// Every key while [`State::story_view`] is set: `q`/`Esc`/`S` closes and
/// returns to SESSIONS with list state intact, anything scroll-shaped
/// scrolls, everything else is a no-op — the view is read-only and never
/// advances a run.
fn handle_story_view_key(state: &mut State, key: &crossterm::event::KeyEvent) -> crate::Result<()> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('S') => {
            state.story_view = None;
        }
        _ => {
            if let Some(delta) = tui_kit::scroll_key(key)
                && let Some(view) = state.story_view.as_mut()
            {
                view.scroll.apply(delta, usize::MAX);
            }
        }
    }
    Ok(())
}

/// Enter/space on a SESSIONS group header (P506 §3.1): flips its collapsed
/// state and rebuilds the visible-row list. A no-op on a session row (no
/// group to toggle) or when nothing is selected.
fn toggle_selected_group(state: &mut State) {
    let Some(VisibleRow::GroupHeader { group, .. }) =
        state.sessions_visible.get(state.list_sessions.selected())
    else {
        return;
    };
    let group = *group;
    if !state.collapsed_groups.remove(&group) {
        state.collapsed_groups.insert(group);
    }
    rebuild_visible_sessions(state);
}

/// Probe budget: which rows this tick's [`sessions_from_inventory_tagged`]
/// call is allowed to spend a `flock` probe on. Bounding the per-tick probe
/// set to the liveness index's own rows (typically a handful) is what gets
/// `State::reload()` from O(every ledger) down to O(live drivers); a
/// row-less held lock (adoption — an externally started driver, or one from
/// before this index existed) is instead caught by [`ProbeBudget::Sweep`] on
/// a slower cadence, never by probing every ledger every tick.
enum ProbeBudget<'a> {
    /// Probe only sessions the local liveness index has a row for.
    IndexOnly(&'a std::collections::HashSet<String>),
    /// Probe every row this call sees (the periodic full sweep).
    Sweep,
}

impl ProbeBudget<'_> {
    fn allows(&self, session_id: &str) -> bool {
        match self {
            ProbeBudget::IndexOnly(ids) => ids.contains(session_id),
            ProbeBudget::Sweep => true,
        }
    }
}

fn sessions_from_inventory_tagged(
    inventory: &[ctx_traits_io::run_session::RunInventoryRow],
    repo_key: Option<&str>,
    repo_path: Option<&str>,
    probe_budget: &ProbeBudget<'_>,
) -> Vec<SessionRow> {
    let mut rows = Vec::new();
    for row in inventory {
        let probe = if probe_budget.allows(&row.session_id) {
            ctx_traits_io::run_control::probe(&row.ledger_path).unwrap_or(
                ctx_traits_io::run_control::DriverProbe::Unheld {
                    stale_metadata: None,
                },
            )
        } else {
            ctx_traits_io::run_control::DriverProbe::Unheld {
                stale_metadata: None,
            }
        };
        let (live, holder_pid) = match &probe {
            ctx_traits_io::run_control::DriverProbe::Held(holder) => {
                (true, holder.as_ref().map(|holder| holder.pid).unwrap_or(0))
            }
            ctx_traits_io::run_control::DriverProbe::Unheld { .. } => (false, 0),
        };
        // A slower full sweep discovers pre-index drivers and publishes the
        // same pointer evidence ordinary machine-wide reporting consumes.
        if live
            && matches!(probe_budget, ProbeBudget::Sweep)
            && let ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } =
                &row.status
        {
            let facts = ctx_traits_io::run_liveness::LiveRunFacts {
                session_id: row.session_id.clone(),
                run_id: session.run_id.as_str().to_string(),
                repo_key: repo_key.unwrap_or_default().to_string(),
                repo_path: repo_path.unwrap_or_default().to_string(),
                ledger_path: row.ledger_path.clone(),
                worktree_path: session
                    .provenance
                    .worktree
                    .as_ref()
                    .and_then(|worktree| worktree.path.clone()),
                branch: session
                    .provenance
                    .worktree
                    .as_ref()
                    .map(|worktree| worktree.branch.clone()),
                log_path: None,
            };
            let _ = ctx_traits_io::run_liveness::upsert_row(
                &ctx_traits_io::run_control::runtime_root(),
                &facts,
                holder_pid,
                session.provenance.started_at_epoch.unwrap_or(0),
            );
        }
        let (
            state_text,
            phase,
            elapsed_text,
            tokens_text,
            run_id,
            class,
            status,
            outcome,
            title,
            task_key,
            merged_landed,
        ) = match &row.status {
            ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } => {
                let outcome = session
                    .last_drive_outcome
                    .as_ref()
                    .map(|outcome| outcome.outcome.clone());
                let class = classify_session(live, &session.status, outcome.as_ref());
                let state_text = if live {
                    "live".to_string()
                } else {
                    run_view::session_status(&session.status).to_string()
                };
                let mut phase = parked_ask_presentation(session, repo_path)
                    .unwrap_or_else(|| run_view::phase_text(session));
                if let Some(warning) = ctx_traits_io::run::trait_source_drift_from(
                    session,
                    repo_path.map(camino::Utf8Path::new),
                )
                .warning()
                {
                    phase.push_str(&format!("; {warning}"));
                }
                let elapsed_text =
                    tui::elapsed_text(Duration::from_secs(session.ledger.elapsed_seconds));
                let token_usage = session
                    .last_drive_outcome
                    .as_ref()
                    .and_then(|outcome| outcome.token_usage.as_ref());
                let tokens_text = dashboard_tokens_text(token_usage);
                (
                    state_text,
                    phase,
                    elapsed_text,
                    tokens_text,
                    session.run_id.as_str().to_string(),
                    class,
                    Some(session.status.clone()),
                    outcome,
                    persisted_session_title(session, &row.ledger_path),
                    session.provenance.task_key.clone(),
                    super::task_proposals::merged_landed_sha(session),
                )
            }
            ctx_traits_io::run_session::InventoryOutcome::Unreadable { error } => (
                "unreadable".to_string(),
                error.clone(),
                "-".to_string(),
                "-".to_string(),
                String::new(),
                SessionClass::Unreadable,
                None,
                None,
                None,
                None,
                None,
            ),
        };
        rows.push(SessionRow {
            session_id: row.session_id.clone(),
            ledger_path: row.ledger_path.clone(),
            run_id,
            state_text,
            phase,
            elapsed_text,
            tokens_text,
            repo_key: repo_key.map(str::to_string),
            repo_path: repo_path.map(str::to_string),
            class,
            status,
            outcome,
            title,
            task_key,
            merged_landed,
        });
    }
    rows.sort_by_key(|row| {
        if row.class == SessionClass::Live {
            0
        } else {
            1
        }
    });
    rows
}

fn dashboard_tokens_text(
    usage: Option<&ctx_traits_core::procedure::session::TokenUsageEvidence>,
) -> String {
    let Some(usage) = usage.filter(|usage| {
        usage.work_tokens.is_some()
            || usage.narrator_tokens.is_some()
            || usage.guide_tokens.is_some()
    }) else {
        return "-".to_string();
    };
    format!(
        "W:{} N:{} G:{}",
        dashboard_token_value(usage.work_tokens),
        dashboard_token_value(usage.narrator_tokens),
        dashboard_token_value(usage.guide_tokens),
    )
}

fn dashboard_token_value(tokens: Option<u64>) -> String {
    tokens
        .map(tui::token_text)
        .map(|text| {
            text.trim_end_matches(" tok")
                .replace(".0k", "k")
                .replace(".0m", "m")
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Builds both TRAITS' and TRUST's rows from the single
/// [`dashboard_trait_inventory`] scan (P473 §4.1): TRAITS filters
/// `origin != Some("built-in")` (byte-identical to pre-P473 rows); TRUST
/// projects the full tier set via [`build_trust_rows`]. No second inventory
/// scan, no second trust-store read.
fn load_traits_and_trust() -> crate::Result<(Vec<TraitRow>, Vec<TrustRow>)> {
    let all = dashboard_trait_inventory()?;
    let traits = all
        .iter()
        .filter(|row| row.origin.as_deref() != Some("built-in"))
        .map(|row| TraitRow {
            id: row.id.clone(),
            version: row.version.clone(),
            status: row.error.clone().unwrap_or_else(|| row.status.clone()),
            trust: if row.error.is_some() {
                "unreadable".to_string()
            } else {
                row.trust.clone()
            },
            canonical_digest: row.canonical_digest.clone(),
            source_path: row.source_path.clone(),
            error: row.error.clone(),
        })
        .collect();
    let trust = build_trust_rows(&all)?;
    Ok((traits, trust))
}

/// Projects a run inventory scan to the cheap owned facts [`run_sighting`]
/// needs (§4.4) — called before [`merges_from_inventory`] consumes the same
/// scan by value.
fn run_sighting_rows(
    inventory: &[ctx_traits_io::run_session::RunInventoryRow],
) -> Vec<RunSightingRow> {
    inventory
        .iter()
        .filter_map(|row| {
            let ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } =
                &row.status
            else {
                return None;
            };
            let canonical_digest = session.canonical_digest.as_ref()?.as_str().to_string();
            Some(RunSightingRow {
                trait_id: session.trait_id.clone(),
                canonical_digest,
                run_id: session.run_id.as_str().to_string(),
                session_id: row.session_id.clone(),
                modified_epoch_secs: row.modified_epoch_secs,
            })
        })
        .collect()
}

/// The most recent readable run-ledger sighting (§4.4) whose `trait_id` and
/// canonical digest both match — pure over the already-in-hand
/// [`RunSightingRow`] projection, never re-scanning the ledger store.
fn run_sighting(
    rows: &[RunSightingRow],
    trait_id: &str,
    digest: &str,
) -> Option<trust_story::RunSighting> {
    rows.iter()
        .filter(|row| row.trait_id == trait_id && row.canonical_digest == digest)
        .max_by_key(|row| row.modified_epoch_secs)
        .map(|row| trust_story::RunSighting {
            run_id: row.run_id.clone(),
            session_id: row.session_id.clone(),
            when: format_epoch_ago(row.modified_epoch_secs),
        })
}

/// `"<elapsed> ago"` from a Unix-epoch-seconds timestamp — the ledger's own
/// `modified_epoch_secs`, never a claim about when the digest itself
/// changed (§4.4: a sighting is evidence bytes ran, not evidence of when
/// they moved).
fn format_epoch_ago(epoch_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(epoch_secs);
    let elapsed = now.saturating_sub(epoch_secs);
    format_elapsed_ago(Duration::from_secs(elapsed))
}

fn format_elapsed_ago(elapsed: Duration) -> String {
    format!("{} ago", tui::human_elapsed_text(elapsed))
}

/// Pure `(last terminal frame, drive-completed) -> MergeClass` mapping
/// (P472 §3.3), extracted out of [`merges_from_inventory`]'s inventory scan
/// so the classification is directly unit-testable without an inventory scan
/// or any IO. Reuses [`ctx_traits_core::procedure::session::MergeStatus::is_terminal`]
/// and [`super::run::disposition_for_merge_status`] rather than inventing a
/// second classifier; `drive_completed` is the exact `merge::merge`-applied
/// test (`Status::Completed` + `last_drive_outcome.outcome == "completed"`).
fn classify_merge(
    last_terminal_frame: Option<&ctx_traits_core::procedure::session::MergeFrame>,
    drive_completed: bool,
) -> Option<MergeClass> {
    match last_terminal_frame {
        Some(frame) => Some(
            match super::run::disposition_for_merge_status(frame.status) {
                super::run::CompletionDisposition::Merged => MergeClass::Landed,
                super::run::CompletionDisposition::Parked => MergeClass::Parked,
                _ => MergeClass::Failed,
            },
        ),
        None if drive_completed => Some(MergeClass::Mergeable),
        None => None,
    }
}

/// The list row's translated one-line headline (`MergeRow::headline`'s own
/// doc contract: empty for `Mergeable`/`Landed`). A landed run has nothing
/// to explain, so showing `merge_story`'s "landed cleanly" headline next to
/// the `landed` class column would be redundant, not
/// translated-vs-untranslated; a row with no terminal frame yet
/// (`Mergeable`) has nothing to translate at all.
fn merge_row_headline(
    class: MergeClass,
    last_terminal_frame: Option<&ctx_traits_core::procedure::session::MergeFrame>,
) -> String {
    match (class, last_terminal_frame) {
        (MergeClass::Landed, _) | (_, None) => String::new(),
        (_, Some(frame)) => merge_story::explain_frame(frame).headline,
    }
}

/// MERGES-screen rows, projected from the same inventory scan
/// [`sessions_from_inventory_tagged`] uses. Consumes `inventory` by value
/// since each row's merge-frame history is owned data. Widened from P468's
/// "latest frame is Parked" to every readable ledger whose last *terminal*
/// merge frame (or completed-drive-with-no-attempt) classifies as
/// mergeable/parked/failed/landed (§3.3) — an unreadable ledger, or a
/// non-terminal in-progress row, produces no row. `repo_path` tags every
/// produced row for ALL-mode git-fact gating (§3.4), mirroring
/// `sessions_from_inventory_tagged`.
fn merges_from_inventory(
    inventory: Vec<ctx_traits_io::run_session::RunInventoryRow>,
    repo_path: Option<&str>,
) -> Vec<MergeRow> {
    let mut rows = Vec::new();
    for row in inventory {
        let ctx_traits_io::run_session::InventoryOutcome::Readable { session, .. } = row.status
        else {
            continue;
        };
        let last_terminal_frame = session
            .provenance
            .merge_frames
            .iter()
            .rev()
            .find(|frame| frame.status.is_terminal());
        let drive_completed = session.status
            == ctx_traits_core::procedure::session::Status::Completed
            && session
                .last_drive_outcome
                .as_ref()
                .is_some_and(|outcome| outcome.outcome.is_completed());
        let Some(class) = classify_merge(last_terminal_frame, drive_completed) else {
            continue;
        };
        let stage = last_terminal_frame.map(|frame| frame.stage);
        let headline = merge_row_headline(class, last_terminal_frame);
        rows.push(MergeRow {
            session_id: row.session_id,
            run_id: session.run_id.as_str().to_string(),
            ledger_path: row.ledger_path,
            class,
            stage,
            headline,
            phase: ctx_traits_io::run_session::session_task(&session),
            trait_id: session.trait_id.clone(),
            last_frame: last_terminal_frame.cloned(),
            worktree: session.provenance.worktree.clone(),
            repo_path: repo_path.map(str::to_string),
        });
    }
    rows
}

/// Builds TRUST's trait-centric rows (P473 §4.2): joins each visible trait
/// (every tier, including built-ins — `all` is [`dashboard_trait_inventory`]'s
/// unfiltered result) against its identity-bound-or-legacy trust record —
/// the same preference [`trust_record_facts`] already implements for TRAITS
/// — then appends one row per record [`ctx_traits_io::trust::classify_records`]
/// classifies `Orphaned` (names no visible trait), so nothing in
/// `trust.toml` becomes invisible. An unreadable trait row (no canonical
/// digest was ever computed) is skipped, matching the old digest-centric
/// `load_trust`'s own rule.
fn build_trust_rows(all: &[DashboardTraitRow]) -> crate::Result<Vec<TrustRow>> {
    let document = ctx_traits_io::trust::read_store()?;
    Ok(build_trust_rows_from(&document, all))
}

/// Projects a supplied trust document for the TRUST screen. Keeping the join
/// pure makes exact-digest authority testable without machine-local store IO.
fn build_trust_rows_from(
    document: &ctx_traits_io::trust::Document,
    all: &[DashboardTraitRow],
) -> Vec<TrustRow> {
    let current: Vec<(String, String)> = all
        .iter()
        .filter(|row| row.error.is_none() && !row.canonical_digest.is_empty())
        .map(|row| (row.id.clone(), row.canonical_digest.clone()))
        .collect();

    let mut rows = Vec::new();
    for row in all {
        if row.error.is_some() || row.canonical_digest.is_empty() {
            continue;
        }
        let record = document.record_for_current(&row.id, &row.canonical_digest);
        let report_row = record.map(|record| ctx_traits_io::trust::TrustReportRow {
            trait_id: Some(row.id.clone()),
            digest: record.digest.clone(),
            current_digest: Some(row.canonical_digest.clone()),
            state: record.state,
            freshness: if record.digest == row.canonical_digest {
                ctx_traits_io::trust::TrustFreshness::Current
            } else {
                ctx_traits_io::trust::TrustFreshness::Stale
            },
            updated_at: record.updated_at.clone(),
            reason: record.reason.clone(),
            seq: record.seq,
            superseded: false,
        });
        rows.push(TrustRow {
            trait_id: Some(row.id.clone()),
            origin: row.origin.clone().unwrap_or_else(|| "repo".to_string()),
            family: row.family.clone(),
            variant: row.variant.clone(),
            current_digest: row.canonical_digest.clone(),
            recorded_digest: record.map(|record| record.digest.clone()),
            class: trust_story::classify_trust(report_row.as_ref()),
            updated_at: record.and_then(|record| record.updated_at.clone()),
            reason: record.and_then(|record| record.reason.clone()),
        });
    }

    let classified = ctx_traits_io::trust::classify_records(document, &current);
    for orphan in classified
        .into_iter()
        .filter(|row| row.freshness == ctx_traits_io::trust::TrustFreshness::Orphaned)
    {
        rows.push(TrustRow {
            trait_id: None,
            origin: "orphaned".to_string(),
            family: None,
            variant: None,
            current_digest: String::new(),
            recorded_digest: Some(orphan.digest.clone()),
            class: trust_story::TrustClass::Orphaned,
            updated_at: orphan.updated_at.clone(),
            reason: orphan.reason.clone(),
        });
    }

    sort_trust_rows(&mut rows);
    rows
}

/// The one ordering site for TRUST rows: actionable rows (a resolvable
/// trait) sort before orphan rows, on which `a`/`b`/`A` all refuse by
/// design — an orphan-heavy trust store must never leave the list opening
/// on a row nothing can be done with. Ties broken by trait id, then
/// recorded digest, mirroring `sessions_from_inventory_tagged`'s existing
/// stable-sort precedent in this file.
fn sort_trust_rows(rows: &mut [TrustRow]) {
    rows.sort_by(|a, b| {
        a.trait_id
            .is_none()
            .cmp(&b.trait_id.is_none())
            .then_with(|| {
                a.trait_id
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.trait_id.as_deref().unwrap_or(""))
            })
            .then_with(|| {
                a.recorded_digest
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.recorded_digest.as_deref().unwrap_or(""))
            })
    });
}

/// Entry point: run the dashboard until the user quits. Returns cleanly on
/// every path (quit key, panic-safe teardown via `RatatuiPane`); dashboard
/// exit never signals or otherwise touches any listed run.
pub(crate) fn run() -> crate::Result<()> {
    run_with_initial_session(None, None)
}

/// Opens SESSIONS with this identity selected once its first inventory snapshot
/// arrives. Used by the live run pane after it has restored terminal ownership.
pub(crate) fn run_for_session(
    session_id: String,
    guide_chat: Option<run_view::GuideChatHandle>,
) -> crate::Result<()> {
    run_with_initial_session(Some(session_id), guide_chat)
}

fn run_with_initial_session(
    initial_session_id: Option<String>,
    guide_chat: Option<run_view::GuideChatHandle>,
) -> crate::Result<()> {
    let mut pane = RatatuiPane::new_forwarding_ctrl_c().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: "<tty>".to_string(),
            source,
        })
    })?;
    let mut state = match initial_session_id {
        Some(session_id) => State::new_for_session_with_guide(session_id, guide_chat),
        None => State::new(),
    };
    state.reload();
    // `State::new()` already seeds `focus` on the SESSIONS list pane, which
    // every tree (even the narrow-terminal single-leaf one) includes; the
    // first `draw_screen` call reconciles it against the real tree.
    let mut last_reload = std::time::Instant::now();
    while !state.quit && !pane.detached() {
        if let Some(request) = state.attach_request.take() {
            // P081: tears down this process's own alt-screen pane before the
            // observer's inline pane exists — mirrors the `d`-handoff's own
            // ordering the other direction (dashboard thread takes the
            // terminal only after the live view has already quit its pane).
            pane.quit();
            let outcome = run_attached_observer(
                &request,
                state.guide_chat.as_ref(),
                state.guide_chat_session_id.as_deref(),
            );
            pane = RatatuiPane::new_forwarding_ctrl_c().map_err(|source| {
                ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                    path: "<tty>".to_string(),
                    source,
                })
            })?;
            state.reload();
            last_reload = std::time::Instant::now();
            // A failed attach never took the terminal — it must not collapse
            // into the footer message under whatever the screen looked like
            // mid-teardown. Report it through a blocking modal instead, on
            // the just-rebuilt (fully redrawn) dashboard pane; a successful
            // attach or an ordinary detach still reports on the footer.
            let short_session = state_short_session(&state, &request.session_id);
            match describe_attach_outcome(&outcome, &short_session) {
                Ok(footer) => state.message = Some(footer),
                Err(modal_body) => {
                    state.modal_host.open(
                        Action::AttachFailed,
                        Modal::confirm("attach failed", modal_body),
                    );
                }
            }
            continue;
        }
        if let Some(guide_chat) = state.guide_chat.as_ref() {
            guide_chat.poll_results();
        }
        state.apply_snapshots();
        draw_screen(&mut pane, &mut state).map_err(|source| {
            ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                path: "<tty>".to_string(),
                source,
            })
        })?;
        let key = pane.poll_key(TICK).map_err(|source| {
            ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                path: "<tty>".to_string(),
                source,
            })
        })?;
        let Some(key) = key else {
            if let Some(guide_chat) = state.guide_chat.as_ref() {
                guide_chat.poll_results();
            }
            state.apply_snapshots();
            // Timed out waiting for a key: on a bounded interval, reload the
            // list screens' stores so an externally started/completed drive
            // (or one spawned by `n`, or a run that finished while listed)
            // appears without the user having to press `r`. `State::reload`
            // also refreshes the SESSIONS preview/attach pane at this same
            // cadence — never per-draw. The TASKS board's own stat-sweep
            // (0063.7) rides the same cadence, regardless of which screen is
            // active — a continuous run of keypresses resets `last_reload`
            // and delays both alike, same as the pre-existing session poll;
            // read-your-writes (every provider write re-syncs immediately)
            // covers the interactive case.
            if last_reload.elapsed() >= RELOAD_INTERVAL {
                state.reload();
                refresh_tasks_board_if_stale(&mut state);
                last_reload = std::time::Instant::now();
            }
            continue;
        };
        // Dashboard action failures are rendered in-place. Terminal polling
        // and drawing remain fatal because the terminal can no longer be
        // safely owned after either fails.
        if let Err(error) = handle_key(&mut pane, &mut state, key) {
            state.message = Some(error.to_string());
        }
        last_reload = std::time::Instant::now();
    }
    Ok(())
}

/// P081: Enter on a live SESSIONS row IS the live run view — this builds and
/// runs the observer `run_view::RunPanel` exactly [`reconstruct_projection`]
/// builds its ledger-only reconstruction from (`load_trait_for_session` +
/// `plan_procedure_run`), then drives its own small poll/refresh loop until
/// the panel closes (`d`/confirmed `q`, or the run finishing while attached —
/// see `RunPanel::refresh_from_ledger`). Returns `Ok(Some(message))` only for
/// the finished-while-attached case; `Ok(None)` is an ordinary detach.
/// Trait-resolution failure degrades to `Err` (the caller renders it as a
/// dashboard message) with no terminal ever handed to a panel — this
/// function creates its own inline pane, so nothing is left half-torn-down.
fn run_attached_observer(
    request: &AttachRequest,
    guide_chat: Option<&run_view::GuideChatHandle>,
    guide_chat_session_id: Option<&str>,
) -> crate::Result<Option<String>> {
    let session = ctx_traits_io::run_session::read_run_session(&request.ledger_path)?;
    let loaded =
        ctx_traits_io::run::load_trait_for_session(None, None, &session, "dashboard-attach")?;
    let plan = ctx_traits_core::procedure::run::plan_procedure_run(
        &loaded.trait_ref,
        session.run_id.clone(),
    )?;
    let pane = RatatuiPane::new_inline().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: "<tty>".to_string(),
            source,
        })
    })?;
    let observer = run_view::RunPanel::new_observer(
        loaded.trait_ref.name.as_str().to_string(),
        loaded.trait_ref,
        plan,
        session,
        request.ledger_path.clone(),
        pane,
    );
    // P081 "ask: one deliberate rule" — the observer never dispatches a
    // fresh guide seat; the ONLY permitted handle is the one this dashboard
    // process already holds in-process from a `d`-handoff for this exact
    // session. Every other session's attach leaves `state.ask` `None`,
    // which `poll_and_apply_keys` answers with a visible refusal.
    if guide_chat_session_id == Some(request.session_id.as_str())
        && let Some(chat) = guide_chat
    {
        observer.install_guide_handle(chat.clone(), request.ledger_path.clone());
    }
    let mut last_reload = std::time::Instant::now();
    while !observer.presentation_closed() {
        observer.tick();
        std::thread::sleep(TICK);
        if last_reload.elapsed() >= RELOAD_INTERVAL {
            last_reload = std::time::Instant::now();
            // P081: a transient read error is tolerated (skip the refresh,
            // keep the last frame) — mirrors `refresh_attached_view`'s own
            // degrade discipline; only a fatal error at the INITIAL read
            // above (before any frame exists) surfaces as `Err`.
            if let Ok(session) = ctx_traits_io::run_session::read_run_session(&request.ledger_path)
            {
                observer.refresh_from_ledger(&session, &request.ledger_path);
            }
        }
    }
    let finished = observer.observer_finished();
    observer.close();
    Ok(finished.then(|| "the run finished while attached".to_string()))
}

/// P081/0145: pure mapping from [`run_attached_observer`]'s outcome to what
/// the dashboard reports — extracted so it is directly testable without a
/// `RatatuiPane`. `Ok` is footer text (a successful attach's finish message,
/// or an ordinary detach note); `Err` is the body of the blocking modal a
/// failed attach opens instead, since that outcome never took the terminal
/// and must not collapse into the footer under whatever the screen looked
/// like mid-teardown.
fn describe_attach_outcome(
    outcome: &crate::Result<Option<String>>,
    short_session: &str,
) -> Result<String, String> {
    match outcome {
        Ok(Some(message)) => Ok(message.clone()),
        Ok(None) => Ok(format!("detached from {short_session}")),
        Err(error) => Err(format!("attach failed: {error}")),
    }
}

fn handle_key(
    pane: &mut RatatuiPane,
    state: &mut State,
    key: crossterm::event::KeyEvent,
) -> crate::Result<()> {
    if let Some((tag, outcome)) = state.modal_host.handle_key(&key) {
        return apply_action(pane, state, tag, outcome);
    }
    if state.modal_host.is_open() {
        // The modal is still open and consumed this key as an edit
        // keystroke (`ModalOutcome::Pending`) — every other key routes
        // through it exclusively (the focus trap), never falling through to
        // screen-level handling below.
        return Ok(());
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state
            .modal_host
            .open(Action::Exit, Modal::confirm("exit", "Quit ctx traits?"));
        return Ok(());
    }
    if state.story_view.is_some() {
        return handle_story_view_key(state, &key);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        let dir = match key.code {
            KeyCode::Up => Some(tui_panes::MoveDir::Up),
            KeyCode::Down => Some(tui_panes::MoveDir::Down),
            KeyCode::Left => Some(tui_panes::MoveDir::Left),
            KeyCode::Right => Some(tui_panes::MoveDir::Right),
            _ => None,
        };
        if let Some(dir) = dir {
            state.focus.move_dir(dir, &state.last_pane_layout);
            return Ok(());
        }
    }
    if queue_sessions_pane_key(state, &key) {
        return Ok(());
    }
    if handle_navigation_key(state, &key) {
        return Ok(());
    }
    if handle_focus_key(state, &key) {
        return Ok(());
    }
    match key.code {
        KeyCode::Char('q') => {
            state
                .modal_host
                .open(Action::Exit, Modal::confirm("exit", "Quit ctx traits?"));
        }
        KeyCode::Tab => {
            let screens = Screen::all();
            let idx = screens.iter().position(|s| *s == state.screen).unwrap_or(0);
            state.switch_screen(screens[(idx + 1) % screens.len()]);
        }
        KeyCode::BackTab => {
            let screens = Screen::all();
            let idx = screens.iter().position(|s| *s == state.screen).unwrap_or(0);
            state.switch_screen(screens[(idx + screens.len() - 1) % screens.len()]);
        }
        KeyCode::Char('1') => state.switch_screen(Screen::Sessions),
        KeyCode::Char('2') => state.switch_screen(Screen::Traits),
        KeyCode::Char('3') => state.switch_screen(Screen::Merges),
        KeyCode::Char('4') => state.switch_screen(Screen::Trust),
        KeyCode::Char('5') => state.switch_screen(Screen::Tasks),
        KeyCode::Char('r') => state.reload(),
        KeyCode::Char(' ') if state.screen == Screen::Sessions => toggle_selected_group(state),
        KeyCode::Char('n') if state.screen == Screen::Sessions => {
            open_spawn_modal(state);
        }
        KeyCode::Char('x') if state.screen == Screen::Sessions => open_kill_modal(state),
        KeyCode::Char('s') if state.screen == Screen::Sessions => open_resume_modal(state),
        KeyCode::Char('S') if state.screen == Screen::Sessions => open_story_view(state),
        KeyCode::Char('a') if state.screen == Screen::Sessions => open_answer_modal(state),
        KeyCode::Char('d') if state.screen == Screen::Sessions => open_delete_modal(state),
        KeyCode::Char('v')
            if state.screen == Screen::Sessions || state.screen == Screen::Merges =>
        {
            state.all_repos = !state.all_repos;
            state.list_sessions.reset();
            state.list_merges.reset();
            state.reload();
        }
        KeyCode::Char('a') if state.screen == Screen::Traits => {
            open_trait_trust_modal(state, ctx_traits_io::trust::TrustState::Verified);
        }
        KeyCode::Char('b') if state.screen == Screen::Traits => {
            open_trait_trust_modal(state, ctx_traits_io::trust::TrustState::Blocked);
        }
        KeyCode::Char('e') if state.screen == Screen::Traits => {
            edit_selected_trait_source(pane, state)?;
        }
        KeyCode::Char('x') if state.screen == Screen::Traits => {
            explain_selected_trait(state);
        }
        KeyCode::Char('m') if state.screen == Screen::Merges => {
            open_merge_retry_modal(state, false)
        }
        KeyCode::Char('d') if state.screen == Screen::Merges => open_merge_retry_modal(state, true),
        KeyCode::Char('p') if state.screen == Screen::Merges => print_merge_worktree_path(state),
        KeyCode::Char('x') if state.screen == Screen::Merges => open_merge_drop_modal(state),
        KeyCode::Char('o') if state.screen == Screen::Trust => {
            toggle_trust_orphans(state);
        }
        KeyCode::Char('a') if state.screen == Screen::Trust => {
            open_trust_modal(state, ctx_traits_io::trust::TrustState::Verified);
        }
        KeyCode::Char('b') if state.screen == Screen::Trust => {
            open_trust_modal(state, ctx_traits_io::trust::TrustState::Blocked);
        }
        KeyCode::Char(' ') if state.screen == Screen::Trust => {
            if let Some(row) = selected_trust(state)
                && let Some(id) = row.trait_id.clone()
            {
                state.trust_marks.toggle(id);
            }
        }
        KeyCode::Char('A') if state.screen == Screen::Trust => {
            if state.trust_marks.is_empty() {
                open_trust_family_modal(state, ctx_traits_io::trust::TrustState::Verified);
            } else {
                open_trust_marked_modal(state, ctx_traits_io::trust::TrustState::Verified);
            }
        }
        KeyCode::Char(' ') if state.screen == Screen::Tasks => toggle_selected_task_group(state),
        KeyCode::Char('s') if state.screen == Screen::Tasks => sync_tasks_board(state),
        KeyCode::Char('S') if state.screen == Screen::Tasks => open_task_split_modal(state),
        KeyCode::Char('R') if state.screen == Screen::Tasks => open_task_reconcile(state),
        KeyCode::Char('a') if state.screen == Screen::Tasks => open_task_archive_modal(state),
        KeyCode::Char('e') if state.screen == Screen::Tasks => open_task_edit_modal(state),
        KeyCode::Char('y') if state.screen == Screen::Tasks => open_task_mark_done_modal(state),
        KeyCode::Char('d') if state.screen == Screen::Tasks => dispatch_selected_task(state),
        _ => {}
    }
    Ok(())
}

/// Focus transitions are local state changes, kept apart from action routing
/// so `Enter` and `Esc` remain directly state-machine-testable.
fn handle_focus_key(state: &mut State, key: &crossterm::event::KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            if state.screen == Screen::Sessions {
                state.set_session_follow_all(false);
            }
            focus_pane(&mut state.focus, list_pane_id(state.screen));
            true
        }
        KeyCode::Enter if state.screen == Screen::Sessions => {
            match state.sessions_visible.get(state.list_sessions.selected()) {
                Some(VisibleRow::GroupHeader { .. }) => toggle_selected_group(state),
                _ => attach_selected(state),
            }
            true
        }
        KeyCode::Enter => {
            focus_pane(&mut state.focus, preview_pane_id(state.screen));
            true
        }
        _ => false,
    }
}

/// P081: attach is a synchronous handoff to `run_view::RunPanel::new_observer`
/// (see `attach_selected`/`run_with_initial_session`'s attach loop) — the
/// SESSIONS list is always what this dashboard's own loop draws, so only
/// paging keys (`PageUp`/`PageDown`) queue here, targeting the journey pane
/// per `render_sessions_preview_body`'s `key_target`, letting a long journey
/// be inspected without leaving the list. `Tab`/`BackTab` are deliberately
/// left unqueued so they keep switching dashboard screens, and single-row
/// `Up`/`Down`/`j`/`k` are left to `handle_navigation_key`'s list-selection
/// path. Extracted out of `handle_key` so it stays directly testable without
/// a `RatatuiPane`.
fn queue_sessions_pane_key(state: &mut State, key: &crossterm::event::KeyEvent) -> bool {
    if state.screen != Screen::Sessions {
        return false;
    }
    if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        return false;
    }
    state.pending_keys.push(*key);
    true
}

/// Dashboard-local navigation deliberately differs from the generic focused
/// pane behavior: list movement is always available, while paging addresses a
/// preview even before it has received focus. P552 review
/// `live-run-pane-contract-absent`: a genuinely attached SESSIONS row never
/// reaches this function for a pane-cycle/scroll key — `handle_key` queues
/// those into `state.pending_keys` for the shared `render_pane_body` to drain
/// against its own just-drawn geometry instead. The ordinary (list-visible)
/// SESSIONS preview's own PageUp/PageDown is queued the same way (targeting
/// the journey pane — see `queue_sessions_pane_key`), so this function's own
/// direct page-scroll path below is for the OTHER screens (Traits/Merges/
/// Trust) only, whose single preview pane has no shared-renderer input path
/// of its own.
fn handle_navigation_key(state: &mut State, key: &crossterm::event::KeyEvent) -> bool {
    let selection_delta = match key.code {
        KeyCode::Down | KeyCode::Char('j') => Some(1),
        KeyCode::Up | KeyCode::Char('k') => Some(-1),
        _ => None,
    };
    if let Some(delta) = selection_delta {
        // Moving the list must also make its focus and SESSIONS attachment
        // agree with the row the next preview request will address.
        focus_pane(&mut state.focus, list_pane_id(state.screen));
        state.move_selection(delta);
        state.trait_explanation = None;
        match state.screen {
            Screen::Sessions => refresh_preview_for_selection(state),
            Screen::Traits => refresh_trait_preview_for_selection(state),
            Screen::Merges => refresh_merge_preview_for_selection(state),
            Screen::Trust => refresh_trust_preview_for_selection(state),
            Screen::Tasks => refresh_task_preview_for_selection(state),
        }
        return true;
    }

    if state.screen == Screen::Sessions {
        // Queued by `queue_sessions_pane_key` (attached or list-visible)
        // before this function ever runs.
        return false;
    }

    let Some(delta @ (ScrollDelta::Up(_) | ScrollDelta::Down(_))) = tui_kit::scroll_key(key) else {
        return false;
    };
    if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
        return false;
    }
    let pane_id = preview_pane_id(state.screen);
    apply_pane_scroll(state, pane_id, delta);
    let at_bottom = state
        .pane_scrolls
        .get(pane_id)
        .is_at_bottom(state_pane_scroll_rows(state, pane_id));
    if let Some(follow) = session_follow_field(state, pane_id) {
        *follow = at_bottom;
    }
    true
}

/// The `State` field backing `pane_id`'s attached-follow flag, or `None` for
/// a pane_id this module does not track follow state for.
fn session_follow_field(state: &mut State, pane_id: PaneId) -> Option<&mut bool> {
    match pane_id {
        PANE_SESSIONS_PROGRESS => Some(&mut state.session_progress_follow),
        PANE_SESSIONS_JOURNEY => Some(&mut state.session_journey_follow),
        PANE_SESSIONS_HISTORY => Some(&mut state.session_history_follow),
        PANE_SESSIONS_CURRENT => Some(&mut state.session_current_follow),
        _ => None,
    }
}

/// The `PaneId` of `screen`'s own list pane — the target `Esc` focuses back
/// onto and list movement restores before refreshing its preview.
fn list_pane_id(screen: Screen) -> PaneId {
    match screen {
        Screen::Sessions => PANE_SESSIONS_LIST,
        Screen::Traits => PANE_TRAITS_LIST,
        Screen::Merges => PANE_MERGES_LIST,
        Screen::Trust => PANE_TRUST_LIST,
        Screen::Tasks => PANE_TASKS_LIST,
    }
}

/// The `PaneId` `Enter` focuses on `screen` (P506 §3.3) — SESSIONS' progress
/// pane, or the single preview pane on every other screen.
fn preview_pane_id(screen: Screen) -> PaneId {
    match screen {
        Screen::Sessions => PANE_SESSIONS_PROGRESS,
        Screen::Traits => PANE_TRAITS_PREVIEW,
        Screen::Merges => PANE_MERGES_PREVIEW,
        Screen::Trust => PANE_TRUST_PREVIEW,
        Screen::Tasks => PANE_TASKS_PREVIEW,
    }
}

/// Moves `ring`'s focus to `target`, bounded by the ring's own (small, fixed)
/// leaf count — a no-op if `target` is not one of its leaves.
fn focus_pane(ring: &mut FocusRing, target: PaneId) {
    for _ in 0..8 {
        if ring.current() == Some(target) {
            return;
        }
        ring.next();
    }
}

fn state_pane_scroll_rows(state: &State, pane_id: PaneId) -> usize {
    state
        .last_pane_layout
        .rect(pane_id)
        .map_or(0, |rect| rect.height.saturating_sub(2) as usize)
}

/// Applies the explicit attachment follow state. Master-list previews never
/// enter this path and therefore retain their normal top-aligned position.
fn follow_session_preview(
    rows: usize,
    scroll: &mut tui_kit::ViewportScroll,
    follow: bool,
    new_len: usize,
) {
    scroll.set_len(new_len);
    if follow {
        scroll.apply(ScrollDelta::Down(new_len), rows);
    }
}

/// The scrollable content length backing `pane_id`, for clamping its
/// [`ViewportScroll`](tui_kit::ViewportScroll) before applying a scroll
/// delta — mirrors each screen's own preview-length source.
fn pane_content_len(state: &State, pane_id: PaneId) -> usize {
    if pane_id == PANE_SESSIONS_PROGRESS {
        state
            .session_preview
            .as_ref()
            .map_or(0, |p| p.progress_lines.len())
    } else if pane_id == PANE_SESSIONS_JOURNEY {
        sessions_journey_lines(state).len()
    } else if pane_id == PANE_SESSIONS_HISTORY {
        // P552: each history/current event is exactly one physical row
        // (never re-wrapped), so its row count is the event count itself.
        state
            .session_preview
            .as_ref()
            .map_or(0, |p| p.history.len())
    } else if pane_id == PANE_SESSIONS_CURRENT {
        state
            .session_preview
            .as_ref()
            .map_or(0, |p| p.current.len())
    } else if pane_id == PANE_TRAITS_PREVIEW {
        state.trait_preview.as_ref().map_or(0, |p| p.lines.len())
    } else if pane_id == PANE_MERGES_PREVIEW {
        state.merge_preview.as_ref().map_or(0, |p| p.lines.len())
    } else if pane_id == PANE_TRUST_PREVIEW {
        state.trust_preview.as_ref().map_or(0, |p| p.lines.len())
    } else if pane_id == PANE_TASKS_PREVIEW {
        state.task_preview.as_ref().map_or(0, |p| p.lines.len())
    } else {
        0
    }
}

fn apply_pane_scroll(state: &mut State, pane_id: PaneId, delta: ScrollDelta) {
    let rows = state_pane_scroll_rows(state, pane_id);
    if rows == 0 {
        return;
    }
    let len = pane_content_len(state, pane_id);
    let scroll = state.pane_scrolls.get_mut(pane_id);
    scroll.set_len(len);
    scroll.apply(delta, rows);
}

/// The draw pass knows the actual content and inner viewport dimensions, so it
/// is the authoritative place to repair persisted offsets after resize or
/// content changes.
fn clamp_visible_pane_scroll(state: &mut State, pane_id: PaneId) {
    let rows = state_pane_scroll_rows(state, pane_id);
    if rows == 0 {
        return;
    }
    let len = pane_content_len(state, pane_id);
    let scroll = state.pane_scrolls.get_mut(pane_id);
    scroll.set_len(len);
    scroll.clamp(rows);
}

// ---------------------------------------------------------------------------
// SESSIONS: preview/attach reconstruction (P469 §3.2)
// ---------------------------------------------------------------------------

/// Rebuilds (or reuses) [`State::session_preview`] for the currently
/// selected SESSIONS row. A selection change always rebuilds (a different
/// session_id is a different cache key); an unchanged selection only
/// rebuilds when the ledger's `state_digest` moved since the last build
/// (checked by [`refresh_attached_view`]) — so the 2s reload tick never
/// re-parses a trait package for a session that has not advanced.
fn refresh_preview_for_selection(state: &mut State) {
    // Preview reads are worker-owned. Do not show a prior row while the new
    // selected row's request is in flight.
    state.session_preview = None;
    state.set_session_follow_all(false);
    state.reload();
}

fn labeled_dim_line(text: &str) -> tui::Line {
    let mut line = tui::Line::blank();
    line.push(text.to_string(), tui::Tone::Muted);
    line
}

/// P552 review `dashboard-attach-contract-absent`: derived from the SAME
/// normalized [`ctx_traits_core::procedure::activity::SessionState`]
/// classification dashboard inventory uses for `session_group`/
/// `classify_session` (including the SAME `run_control::probe` liveness
/// check), not a second, narrower `Status` match — so an `Interrupted`/
/// `Killed` drive outcome (normalized to `Cancelled`) ends an attach in the
/// P550 story too, not only `Completed`/`Failed`, while a genuinely
/// resumed-and-held session with a stale prior `Interrupted`/`Killed`
/// outcome on its ledger is correctly kept live rather than forced into the
/// story view out from under the user.
pub(crate) fn session_is_terminal(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> bool {
    session_is_terminal_given_live(session, session_driver_live(ledger_path))
}

/// The ONE authoritative liveness probe backing both [`session_is_terminal`]
/// and title-generation gating ([`super::run_view::title_row_line`]): a
/// contended P423 driver flock is the sole source of truth for "is a driver
/// currently attached to this ledger" (see `ctx_traits_io::run_control`'s own
/// module docs). Split out so a caller that needs the raw signal — not
/// folded through [`ctx_traits_core::procedure::activity::SessionState`]'s
/// terminal classification — can read it without a second probe call.
pub(crate) fn session_driver_live(ledger_path: &camino::Utf8Path) -> bool {
    matches!(
        ctx_traits_io::run_control::probe(ledger_path),
        Ok(ctx_traits_io::run_control::DriverProbe::Held(_))
    )
}

/// [`session_is_terminal`] for a caller that already probed `live` itself
/// (e.g. also needs it for title-generation gating) and must not pay for a
/// second `flock` probe on the same ledger this tick.
pub(crate) fn session_is_terminal_given_live(
    session: &ctx_traits_core::procedure::session::Session,
    live: bool,
) -> bool {
    ctx_traits_core::procedure::activity::SessionState::derive(
        &session.status,
        session.last_drive_outcome.as_ref().map(|o| &o.outcome),
        live,
    )
    .is_terminal()
}

fn build_attached_view(
    session_id: &str,
    ledger_path: &camino::Utf8PathBuf,
    hint_run_id: &str,
) -> AttachedView {
    match ctx_traits_io::run_session::read_run_session(ledger_path) {
        Ok(session) => {
            let state_digest = session.state_digest.to_string();
            let run_id = session.run_id.as_str().to_string();
            let title = persisted_session_title(&session, ledger_path);
            let terminal = session_is_terminal(&session, ledger_path);
            let reconstruction = reconstruct_panes(&session, ledger_path);
            let history_available = !reconstruction.history.is_empty();
            AttachedView {
                session_id: session_id.to_string(),
                ledger_path: ledger_path.clone(),
                run_id,
                state_digest,
                progress_lines: reconstruction.progress,
                journey_lines: reconstruction.journey,
                post_run: reconstruction.post_run,
                history: reconstruction.history,
                current: reconstruction.current,
                title,
                title_state: session.provenance.session_title.clone(),
                trait_name: reconstruction.trait_name,
                started_at_epoch: reconstruction.started_at_epoch,
                trait_degraded: reconstruction.trait_degraded,
                activity_degraded: reconstruction.activity_degraded,
                activity_available: reconstruction.activity_available,
                history_available,
                terminal,
            }
        }
        Err(error) => AttachedView {
            session_id: session_id.to_string(),
            ledger_path: ledger_path.clone(),
            run_id: hint_run_id.to_string(),
            state_digest: String::new(),
            progress_lines: vec![labeled_dim_line(&format!("(unreadable: {error})"))],
            journey_lines: Vec::new(),
            post_run: Vec::new(),
            history: Vec::new(),
            current: Vec::new(),
            title: None,
            title_state: None,
            trait_name: None,
            started_at_epoch: None,
            trait_degraded: Some(error.to_string()),
            activity_degraded: None,
            activity_available: false,
            history_available: false,
            terminal: false,
        },
    }
}

/// Re-reads `view`'s own ledger. An unchanged digest (P552 review
/// `dashboard-attach-contract-absent`: digest equality ALONE selects this
/// path — never gated on `view.journey_lines` or any other reconstructed
/// content, which would retry the expensive trait+plan rebuild every reload
/// for a ledger stuck on a trait-resolution failure despite nothing having
/// changed) re-reads the activity sidecar directly, via
/// [`run_view::load_sidecar_activity_summary`], never [`reconstruct_projection`]
/// — both `history` and `current` are refreshed from it, since an
/// asynchronously landing narrator step summary or activity event does not
/// touch the ledger's own `state_digest` at all. `trait_degraded` (and
/// `trait_name`/`progress_lines`/`journey_lines`, which depend on it) is left
/// untouched: an unchanged digest means trait resolution's own verdict from
/// the last full reconstruction still holds.
fn refresh_attached_view(view: &mut AttachedView) {
    match ctx_traits_io::run_session::read_run_session(&view.ledger_path) {
        Ok(session) => {
            let state_digest = session.state_digest.to_string();
            view.title = persisted_session_title(&session, &view.ledger_path);
            view.title_state = session.provenance.session_title.clone();
            view.terminal = session_is_terminal(&session, &view.ledger_path);
            if state_digest == view.state_digest {
                let summary = run_view::load_sidecar_activity_summary(
                    &session,
                    &view.ledger_path,
                    session.provenance.started_at_epoch,
                );
                view.history = summary.history;
                view.current = summary.current;
                view.activity_degraded = summary.activity_degraded;
                view.activity_available = summary.activity_available;
                view.history_available = !view.history.is_empty();
                view.post_run = run_view::post_run_lines_from_frames(
                    session.status == ctx_traits_core::procedure::session::Status::Completed,
                    &session.provenance.merge_frames,
                );
                return;
            }
            view.state_digest = state_digest;
            view.run_id = session.run_id.as_str().to_string();
            let reconstruction = reconstruct_panes(&session, &view.ledger_path);
            view.progress_lines = reconstruction.progress;
            view.journey_lines = reconstruction.journey;
            view.post_run = reconstruction.post_run;
            view.history = reconstruction.history;
            view.current = reconstruction.current;
            view.trait_degraded = reconstruction.trait_degraded;
            view.activity_degraded = reconstruction.activity_degraded;
            view.activity_available = reconstruction.activity_available;
            view.history_available = !view.history.is_empty();
            view.trait_name = reconstruction.trait_name;
            view.started_at_epoch = reconstruction.started_at_epoch;
        }
        Err(error) => {
            mark_view_unreadable(view, error.to_string());
        }
    }
}

/// Owned counterpart of [`run_view::LedgerPaneProjection`].
struct PaneReconstruction {
    progress: Vec<tui::Line>,
    journey: Vec<run_view::JourneyRow>,
    post_run: Vec<tui::Line>,
    history: Vec<run_view::EventRow>,
    current: Vec<run_view::EventRow>,
    activity_available: bool,
    trait_degraded: Option<String>,
    activity_degraded: Option<String>,
    trait_name: Option<String>,
    started_at_epoch: Option<u64>,
}

/// The one reconstruction path (§3.2): resolves the ledger's own recorded
/// trait source (with digest verification), rebuilds the plan, and renders
/// through the exact same shared pane projection the live `--progress tui`
/// pane's own ledger reconstruction uses. P552 review
/// `dashboard-attach-contract-absent`: a trait-reconstruction failure here
/// means the plan's own step labels are unavailable, but the sidecar is read
/// independently regardless — `activity_available`/`activity_degraded` and
/// even a sidecar-only `history` (via [`run_view::load_sidecar_activity_summary`])
/// stay populated on their own, honest terms rather than being suppressed
/// just because trait resolution also failed.
fn reconstruct_panes(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> PaneReconstruction {
    match reconstruct_projection(session, ledger_path) {
        Ok(projection) => PaneReconstruction {
            progress: projection.progress,
            journey: projection.journey,
            post_run: projection.post_run,
            history: projection.history,
            current: projection.current,
            activity_available: projection.activity_available,
            trait_degraded: None,
            activity_degraded: projection.activity_degraded,
            trait_name: Some(projection.trait_name),
            started_at_epoch: projection.started_at_epoch,
        },
        Err(error) => {
            // P552 review `dashboard-attach-contract-absent`: the activity
            // sidecar is read independently of the failed trait resolution
            // above — a current-only (or fuller) sidecar must still surface
            // its `current`/sidecar-only `history` and `activity_available:
            // true`, and its OWN degradation reason (skipped lines) must
            // still reach the caller rather than being discarded in favor of
            // the (unrelated) trait-resolution failure.
            let started_at_epoch = session.provenance.started_at_epoch;
            let summary =
                run_view::load_sidecar_activity_summary(session, ledger_path, started_at_epoch);
            PaneReconstruction {
                progress: fallback_lines(session, &error.to_string()),
                journey: Vec::new(),
                post_run: run_view::post_run_lines_from_frames(
                    session.status == ctx_traits_core::procedure::session::Status::Completed,
                    &session.provenance.merge_frames,
                ),
                history: summary.history,
                current: summary.current,
                activity_available: summary.activity_available,
                trait_degraded: Some(error.to_string()),
                activity_degraded: summary.activity_degraded,
                // P552 review `live-run-pane-contract-absent`: a resolved
                // trait's display NAME is unavailable (that resolution is
                // exactly what failed), but the ledger's own persisted trait
                // ID always is — carried here so a persisted title still
                // renders alongside SOMETHING truthful instead of
                // disappearing entirely because trait loading failed.
                trait_name: Some(session.trait_id.clone()),
                started_at_epoch,
            }
        }
    }
}

fn reconstruct_projection(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> crate::Result<run_view::LedgerPaneProjection> {
    let loaded = ctx_traits_io::run::load_trait_for_session(None, None, session, "dashboard")?;
    let plan = ctx_traits_core::procedure::run::plan_procedure_run(
        &loaded.trait_ref,
        session.run_id.clone(),
    )?;
    Ok(run_view::render_ledger_run_view(
        &loaded.trait_ref,
        &plan,
        session,
        ledger_path,
    ))
}

/// Degrade path (§3.2): trait resolution can fail (source moved, digests
/// moved, or a foreign-repository row) — this is never a crash or an empty
/// pane, just today's plain ledger summary.
fn fallback_lines(
    session: &ctx_traits_core::procedure::session::Session,
    resolution_error: &str,
) -> Vec<tui::Line> {
    let mut lines = vec![
        labeled_dim_line(&format!("(live view unavailable: {resolution_error})")),
        tui::Line::blank(),
        labeled_dim_line(&format!(
            "status: {}",
            run_view::session_status(&session.status)
        )),
        labeled_dim_line(&format!(
            "elapsed-seconds: {}",
            session.ledger.elapsed_seconds
        )),
    ];
    if let Some(outcome) = &session.last_drive_outcome {
        lines.push(labeled_dim_line(&format!(
            "last-drive-outcome: {}",
            outcome.outcome.as_str()
        )));
    }
    lines
}

/// The session's display title: the ledger's resolved title when it exists,
/// else the one the title worker parked in the activity sidecar. The ledger
/// resolves only at a frame boundary — after the whole first step — while
/// the sidecar record lands the moment the narrator answers, so the fallback
/// is what makes a fresh run's title visible in seconds instead of minutes.
/// The sidecar read happens only on the not-yet-resolved path, so a settled
/// session costs no extra IO.
fn persisted_session_title(
    session: &ctx_traits_core::procedure::session::Session,
    ledger_path: &camino::Utf8Path,
) -> Option<String> {
    session
        .provenance
        .session_title
        .as_ref()
        .and_then(ctx_traits_core::procedure::session::SessionTitleState::resolved_title)
        .map(str::to_string)
        .or_else(|| ctx_traits_io::activity_sidecar::read_session_title(ledger_path))
}

fn mark_view_unreadable(view: &mut AttachedView, error: String) {
    // Force a recovered ledger with the same prior digest through reconstruction.
    view.state_digest.clear();
    view.journey_lines.clear();
    view.progress_lines = vec![labeled_dim_line(&format!("(unreadable: {error})"))];
    view.history.clear();
    view.current.clear();
    view.post_run.clear();
    view.trait_degraded = Some(error);
    view.activity_degraded = None;
    view.activity_available = false;
}

fn session_preview_matches_current(state: &State, session_id: &str) -> bool {
    selected_session(state).is_some_and(|row| row.session_id == session_id)
}

// ---------------------------------------------------------------------------
// TRAITS: preview reconstruction (P471 §4.2–4.3)
// ---------------------------------------------------------------------------

/// The cache gate (§4.3, test-covered): a pure predicate over the cached key
/// (if any) and the selected row's current key, so the 2s reload tick and
/// every selection change funnel through the exact same "does this need a
/// rebuild" decision as the unit tests exercise, no wall-clock or call-count
/// involved.
fn trait_preview_needs_rebuild(
    cached: Option<(&str, &str)>,
    trait_id: &str,
    canonical_digest: &str,
) -> bool {
    match cached {
        Some((cached_id, cached_digest)) => {
            cached_id != trait_id || cached_digest != canonical_digest
        }
        None => true,
    }
}

/// Rebuilds (or reuses) [`State::trait_preview`] for the currently selected
/// TRAITS row, gated by [`trait_preview_needs_rebuild`] — never in the draw
/// path, only on selection change, reload, and (forced) after an edit.
fn refresh_trait_preview_for_selection(state: &mut State) {
    refresh_trait_preview_impl(state, false);
}

/// Forces a rebuild regardless of the cache key: used after an EDIT SOURCE
/// round-trip (§4.6), where the canonical digest deliberately does NOT move
/// (editing authored source never touches the generated artifact) but the
/// preview's drift/error facts must still refresh.
fn force_rebuild_trait_preview_for_selection(state: &mut State) {
    refresh_trait_preview_impl(state, true);
}

fn refresh_trait_preview_impl(state: &mut State, force: bool) {
    let Some(row) = state.traits.get(state.selected()) else {
        state.trait_preview = None;
        return;
    };
    let cached = state
        .trait_preview
        .as_ref()
        .map(|preview| (preview.trait_id.as_str(), preview.canonical_digest.as_str()));
    if !force && !trait_preview_needs_rebuild(cached, &row.id, &row.canonical_digest) {
        return;
    }
    let trust_document = ctx_traits_io::trust::read_store().unwrap_or_default();
    state.trait_preview = Some(build_trait_preview(row, &trust_document));
}

/// IO edge (§4.2): resolves the trait document, the trust record, and a
/// bounded source excerpt, then hands off to the pure [`trait_preview_lines`]
/// for rendering. `trust_document` is passed in rather than re-read per
/// selection — `load_trust` already reads it once per reload.
fn build_trait_preview(
    row: &TraitRow,
    trust_document: &ctx_traits_io::trust::Document,
) -> TraitPreview {
    let facts = trait_preview_facts(row, trust_document);
    let lines = trait_preview_lines(&facts)
        .iter()
        .map(tui_ratatui::render_line)
        .collect();
    TraitPreview {
        trait_id: row.id.clone(),
        canonical_digest: row.canonical_digest.clone(),
        lines,
    }
}

fn trait_preview_facts(
    row: &TraitRow,
    trust_document: &ctx_traits_io::trust::Document,
) -> TraitPreviewFacts {
    let (trust_state, trust_reason, trust_stale, has_trust_record) =
        trust_record_facts(trust_document, &row.id, &row.canonical_digest);
    if let Some(error) = &row.error {
        return TraitPreviewFacts {
            id: row.id.clone(),
            version: row.version.clone(),
            status: row.status.clone(),
            canonical_digest: row.canonical_digest.clone(),
            trust_state,
            trust_reason,
            trust_stale,
            has_trust_record,
            // Drift is intentionally a selected-preview cost, never part of
            // the TRAITS inventory scan.
            drift: dashboard_trait_drift(&row.source_path),
            source_drift_checked: false,
            procedure: ProcedureShape::Unknown,
            source_path: row.source_path.clone(),
            source_excerpt: Vec::new(),
            error: Some(error.clone()),
        };
    }
    let (procedure, load_error) = match ctx_traits_io::run::load_trait(&row.source_path) {
        Ok((trait_ref, ..)) => {
            let procedure = match &trait_ref.procedure {
                None => ProcedureShape::GuidanceOnly,
                Some(procedure) => ProcedureShape::Sequence(
                    procedure
                        .sequence
                        .iter()
                        .map(|item| {
                            (
                                item.id.clone().unwrap_or_else(|| "(unnamed)".to_string()),
                                item.kind
                                    .map(sequence_kind_label)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| "(no kind)".to_string()),
                            )
                        })
                        .collect(),
                ),
            };
            (procedure, None)
        }
        Err(error) => (ProcedureShape::Unknown, Some(error.to_string())),
    };
    let editable_source = dashboard_trait_editable_source(&row.source_path);
    let source_path = editable_source
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| row.source_path.clone());
    let source_excerpt = editable_source
        .as_deref()
        .map(|path| read_source_excerpt(path, 40))
        .unwrap_or_default();
    TraitPreviewFacts {
        id: row.id.clone(),
        version: row.version.clone(),
        status: row.status.clone(),
        canonical_digest: row.canonical_digest.clone(),
        trust_state,
        trust_reason,
        trust_stale,
        has_trust_record,
        drift: dashboard_trait_drift(&row.source_path),
        // `dashboard_trait_drift` unconditionally passes `skip_cdk_drift:
        // true` (lifecycle_reporting.rs), so the authored source is never
        // actually compared here yet.
        source_drift_checked: false,
        procedure,
        source_path,
        source_excerpt,
        error: load_error,
    }
}

/// Joins `trust_document` against `trait_id`/`canonical_digest` (§4.2 point
/// 3): exact current-digest evidence is preferred before identity history.
/// Returns `(state, reason, stale, has_record)` —
/// `stale` is `load_trust`'s own "record's digest moved" notion, reused
/// rather than re-derived.
fn trust_record_facts(
    document: &ctx_traits_io::trust::Document,
    trait_id: &str,
    canonical_digest: &str,
) -> (String, String, bool, bool) {
    if let Some(record) = document.record_for_current(trait_id, canonical_digest) {
        let stale = record.digest != canonical_digest;
        return (
            record.state.as_str().to_string(),
            record.reason.clone().unwrap_or_default(),
            stale,
            true,
        );
    }
    if let Some(record) = document.record(canonical_digest) {
        return (
            record.state.as_str().to_string(),
            record.reason.clone().unwrap_or_default(),
            false,
            true,
        );
    }
    ("pending".to_string(), String::new(), false, false)
}

/// Bounded read (§4.2 point 4): reads at most `max_lines` lines via a
/// `BufReader`, never slurping an arbitrarily large source file into memory
/// first.
fn read_source_excerpt(path: &camino::Utf8Path, max_lines: usize) -> Vec<String> {
    use std::io::BufRead;
    let Ok(file) = std::fs::File::open(path.as_std_path()) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .take(max_lines)
        .map_while(Result::ok)
        .collect()
}

/// Pure renderer (§4.2, directly unit-testable): no IO, just `facts` ->
/// styled lines. An error surfaces at the top in `Tone::Fail` (§4.6's degrade
/// path) with the rest of the pane still rendering from whatever facts
/// survive — never an empty pane.
fn trait_preview_lines(facts: &TraitPreviewFacts) -> Vec<tui::Line> {
    let mut lines = Vec::new();
    if let Some(error) = &facts.error {
        let mut line = tui::Line::blank();
        line.push(format!("({error})"), tui::Tone::Fail);
        lines.push(line);
        lines.push(tui::Line::blank());
    }

    let mut header = tui::Line::blank();
    header.push(format!("{} ", facts.id), tui::Tone::Bold);
    header.push(format!("v{}", facts.version), tui::Tone::Muted);
    lines.push(header);

    let mut status_line = tui::Line::blank();
    status_line.push("status: ", tui::Tone::Muted);
    status_line.push(facts.status.clone(), tui::Tone::Default);
    lines.push(status_line);
    lines.push(tui::Line::blank());

    let mut procedure_header = tui::Line::blank();
    procedure_header.push("procedure:", tui::Tone::Muted);
    lines.push(procedure_header);
    match &facts.procedure {
        ProcedureShape::Unknown => {
            let mut line = tui::Line::blank();
            line.push(
                "(procedure unknown — trait could not be read)",
                tui::Tone::Fail,
            );
            lines.push(line);
        }
        ProcedureShape::GuidanceOnly => {
            let mut line = tui::Line::blank();
            line.push("(no procedure — guidance-only trait)", tui::Tone::Muted);
            lines.push(line);
        }
        ProcedureShape::Sequence(items) if items.is_empty() => {
            let mut line = tui::Line::blank();
            line.push(
                "(procedure declared with no sequence items)",
                tui::Tone::Muted,
            );
            lines.push(line);
        }
        ProcedureShape::Sequence(items) => {
            for (id, kind) in items {
                let mut line = tui::Line::blank();
                line.push(format!("  {id} "), tui::Tone::Default);
                line.push(format!("({kind})"), tui::Tone::Muted);
                lines.push(line);
            }
        }
    }
    lines.push(tui::Line::blank());

    let mut digest_line = tui::Line::blank();
    digest_line.push("digest: ", tui::Tone::Muted);
    digest_line.push(
        if facts.canonical_digest.is_empty() {
            "(unreadable)".to_string()
        } else {
            facts.canonical_digest.clone()
        },
        tui::Tone::Default,
    );
    lines.push(digest_line);

    let mut trust_line = tui::Line::blank();
    trust_line.push("trust: ", tui::Tone::Muted);
    let trust_tone = match facts.trust_state.as_str() {
        "verified" => tui::Tone::Pass,
        "blocked" => tui::Tone::Fail,
        _ => tui::Tone::Default,
    };
    trust_line.push(facts.trust_state.clone(), trust_tone);
    lines.push(trust_line);

    if !facts.trust_reason.is_empty() {
        let mut reason_line = tui::Line::blank();
        reason_line.push("reason: ", tui::Tone::Muted);
        reason_line.push(facts.trust_reason.clone(), tui::Tone::Default);
        lines.push(reason_line);
    }

    if facts.has_trust_record && facts.trust_stale {
        let mut line = tui::Line::blank();
        line.push(
            "trust record is for an older digest — re-approval required",
            tui::Tone::Warn,
        );
        lines.push(line);
    }

    let mut drift_line = tui::Line::blank();
    drift_line.push("drift: ", tui::Tone::Muted);
    drift_line.push(facts.drift.clone(), tui::Tone::Default);
    lines.push(drift_line);
    // The pane must never present a positive all-clear over a narrower
    // scope than it appears to cover (blocker
    // `trait-preview-drift-omits-authored-source`) — this qualifier is a
    // required, always-present fact, not a conditional decoration, so an
    // authored-source edit that leaves the canonical digest untouched can
    // never look like an unqualified clean bill.
    if !facts.source_drift_checked {
        let mut line = tui::Line::blank();
        line.push(
            "authored source not re-checked — run `ctx traits build` to verify",
            tui::Tone::Warn,
        );
        lines.push(line);
    }
    lines.push(tui::Line::blank());

    let mut source_header = tui::Line::blank();
    source_header.push(format!("source: {}", facts.source_path), tui::Tone::Muted);
    lines.push(source_header);
    for text in &facts.source_excerpt {
        let mut line = tui::Line::blank();
        line.push(text.clone(), tui::Tone::Default);
        lines.push(line);
    }

    lines
}

/// Identity-addressed re-location (§4.6 point 1): the new index of
/// `trait_id` within `traits`, or `None` when an edit made it vanish from
/// the inventory entirely (never "select whatever now sits at the old
/// index").
fn reposition_trait_selection(traits: &[TraitRow], trait_id: &str) -> Option<usize> {
    traits.iter().position(|row| row.id == trait_id)
}

/// One resolved trust-write attempt's outcome, decided as a pure function of
/// already-resolved inputs (§4.4 step 3, test-covered): no IO, so the
/// digest-movement refusal is directly unit-testable without a trust store
/// or a modal.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TrustApplyDecision {
    Proceed,
    RowGone,
    DigestMoved { captured: String, current: String },
}

/// Every member of a `TraitAction::Trust` write (TRAITS' or TRUST's) is
/// re-looked-up in `state.trust` — the superset of all tiers both screens
/// share (§4.7) — rather than `state.traits`, so a family block-approve's
/// whole-set abort checks the same source TRUST itself lists from, no IO, so
/// the digest-movement refusal is directly unit-testable without a trust
/// store or a modal.
fn decide_member_apply(
    trust_rows: &[TrustRow],
    trait_id: &str,
    captured_digest: &str,
) -> TrustApplyDecision {
    let current = trust_rows
        .iter()
        .find(|row| row.trait_id.as_deref() == Some(trait_id))
        .map(|row| row.current_digest.as_str());
    match current {
        None => TrustApplyDecision::RowGone,
        Some(current) if current != captured_digest => TrustApplyDecision::DigestMoved {
            captured: captured_digest.to_string(),
            current: current.to_string(),
        },
        Some(_) => TrustApplyDecision::Proceed,
    }
}

// ---------------------------------------------------------------------------
// TRUST: preview reconstruction (P473 §4.5)
// ---------------------------------------------------------------------------

/// Rebuilds (or reuses) [`State::trust_preview`] for the currently selected
/// TRUST row, gated by [`trust_preview_needs_rebuild`]-style identity
/// caching (§4.5) — a selection change, a moved digest, or a class change
/// (a trust write never moves the digest, only the class) forces a rebuild.
fn refresh_trust_preview_for_selection(state: &mut State) {
    refresh_trust_preview_impl(state, false);
}

/// Forces a rebuild regardless of the cache key — used after a trust write
/// applies (§4.7), where the digest deliberately does not move but the
/// class/recorded-digest facts must still refresh.
fn force_rebuild_trust_preview_for_selection(state: &mut State) {
    refresh_trust_preview_impl(state, true);
}

fn refresh_trust_preview_impl(state: &mut State, force: bool) {
    let Some(row) = selected_trust(state) else {
        state.trust_preview = None;
        return;
    };
    let unchanged = !force
        && row.class != trust_story::TrustClass::Orphaned
        && state.trust_preview.as_ref().is_some_and(|preview| {
            preview.trait_id == row.trait_id
                && preview.current_digest == row.current_digest
                && preview.class == row.class
        });
    if unchanged {
        return;
    }
    state.trust_preview = Some(build_trust_preview(row, &state.trust, &state.run_sightings));
}

/// IO edge (§4.5): resolves the run-ledger sighting and this row's family
/// members, then hands off to the pure [`trust_preview_lines`] for
/// rendering.
fn build_trust_preview(
    row: &TrustRow,
    all_trust: &[TrustRow],
    run_sightings: &[RunSightingRow],
) -> TrustPreview {
    let sighting = row
        .trait_id
        .as_deref()
        .filter(|_| !row.current_digest.is_empty())
        .and_then(|trait_id| run_sighting(run_sightings, trait_id, &row.current_digest));
    let family_members = match &row.family {
        Some(family) => all_trust
            .iter()
            .filter(|member| member.family.as_deref() == Some(family.as_str()))
            .filter_map(|member| Some((member.trait_id.clone()?, member.class)))
            .collect(),
        None => Vec::new(),
    };
    let facts = TrustPreviewFacts {
        trait_id: row.trait_id.clone(),
        origin: row.origin.clone(),
        family: row.family.clone(),
        variant: row.variant.clone(),
        current_digest: row.current_digest.clone(),
        recorded_digest: row.recorded_digest.clone(),
        class: row.class,
        updated_at: row.updated_at.clone(),
        reason: row.reason.clone(),
        sighting,
        family_members,
    };
    let lines = trust_preview_lines(&facts)
        .iter()
        .map(tui_ratatui::render_line)
        .collect();
    TrustPreview {
        trait_id: row.trait_id.clone(),
        current_digest: row.current_digest.clone(),
        class: row.class,
        lines,
    }
}

/// Renders [`TrustPreviewFacts`] into the TRUST detail pane (§4.5): identity,
/// state sentence + next action, what changed (recorded → current digest),
/// run sighting, the fixed "what approving means" block, and — for a family
/// row — every member with its own class, so a block-approve's blast radius
/// is visible before any keypress.
fn trust_preview_lines(facts: &TrustPreviewFacts) -> Vec<tui::Line> {
    let mut lines = Vec::new();

    let mut header = tui::Line::blank();
    header.push(
        facts.trait_id.as_deref().unwrap_or("(orphaned record)"),
        tui::Tone::Default,
    );
    lines.push(header);

    if facts.class != trust_story::TrustClass::Orphaned {
        let mut origin_line = tui::Line::blank();
        origin_line.push("origin: ", tui::Tone::Muted);
        origin_line.push(facts.origin.clone(), tui::Tone::Default);
        if let Some(family) = &facts.family {
            origin_line.push("  family: ", tui::Tone::Muted);
            origin_line.push(family.clone(), tui::Tone::Default);
        }
        if let Some(variant) = &facts.variant {
            origin_line.push("  variant: ", tui::Tone::Muted);
            origin_line.push(variant.clone(), tui::Tone::Default);
        }
        lines.push(origin_line);
    }
    lines.push(tui::Line::blank());

    let mut state_line = tui::Line::blank();
    state_line.push(trust_story::state_sentence(facts.class), tui::Tone::Default);
    lines.push(state_line);
    let mut next_line = tui::Line::blank();
    next_line.push("next: ", tui::Tone::Muted);
    next_line.push(
        trust_story::next_action(
            facts.class,
            facts.family.as_deref(),
            &trust_story::Surface::Tui,
        ),
        tui::Tone::Default,
    );
    lines.push(next_line);
    lines.push(tui::Line::blank());

    let mut digest_line = tui::Line::blank();
    digest_line.push("current digest: ", tui::Tone::Muted);
    digest_line.push(
        if facts.current_digest.is_empty() {
            "(none)".to_string()
        } else {
            facts.current_digest.clone()
        },
        tui::Tone::Default,
    );
    lines.push(digest_line);
    let mut recorded_line = tui::Line::blank();
    recorded_line.push("recorded digest: ", tui::Tone::Muted);
    recorded_line.push(
        facts
            .recorded_digest
            .clone()
            .unwrap_or_else(|| "(none)".to_string()),
        tui::Tone::Default,
    );
    lines.push(recorded_line);
    if let Some(updated_at) = &facts.updated_at {
        let mut updated_line = tui::Line::blank();
        updated_line.push("updated at: ", tui::Tone::Muted);
        updated_line.push(updated_at.clone(), tui::Tone::Default);
        lines.push(updated_line);
    }
    if let Some(reason) = &facts.reason {
        let mut reason_line = tui::Line::blank();
        reason_line.push("recorded reason: ", tui::Tone::Muted);
        reason_line.push(reason.clone(), tui::Tone::Default);
        lines.push(reason_line);
    }
    lines.push(tui::Line::blank());

    let mut sighting_line = tui::Line::blank();
    sighting_line.push(
        trust_story::sighting_sentence(facts.sighting.as_ref()),
        tui::Tone::Muted,
    );
    lines.push(sighting_line);
    lines.push(tui::Line::blank());

    let mut meaning_header = tui::Line::blank();
    meaning_header.push("what approving means:", tui::Tone::Muted);
    lines.push(meaning_header);
    for meaning in trust_story::approval_meaning() {
        let mut line = tui::Line::blank();
        line.push(format!("  - {meaning}"), tui::Tone::Default);
        lines.push(line);
    }

    if !facts.family_members.is_empty() {
        lines.push(tui::Line::blank());
        let mut family_header = tui::Line::blank();
        family_header.push(
            format!(
                "family {} members ({}):",
                facts.family.as_deref().unwrap_or(""),
                facts.family_members.len()
            ),
            tui::Tone::Muted,
        );
        lines.push(family_header);
        for (member_id, member_class) in &facts.family_members {
            let mut line = tui::Line::blank();
            line.push(format!("  {member_id}: "), tui::Tone::Default);
            line.push(member_class.label(), tui::Tone::Muted);
            lines.push(line);
        }
    }

    lines
}

// ---------------------------------------------------------------------------
// MERGES: preview reconstruction (P472 §3.4)
// ---------------------------------------------------------------------------

/// Rebuilds (or reuses) [`State::merge_preview`] for the currently selected
/// MERGES row, gated by the same identity-plus-revision cache key discipline
/// [`trait_preview_needs_rebuild`] established — never in the draw path,
/// only on selection change and reload.
fn refresh_merge_preview_for_selection(state: &mut State) {
    let Some(row) = state.merges.get(state.selected()) else {
        state.merge_preview = None;
        return;
    };
    let cache_key = merge_preview_cache_key(row);
    let same_selection = state
        .merge_preview
        .as_ref()
        .is_some_and(|preview| preview.session_id == row.session_id);
    if same_selection
        && let Some(preview) = &state.merge_preview
        && preview.cache_key == cache_key
    {
        return;
    }
    state.merge_preview = Some(build_merge_preview(row, cache_key));
}

/// The cache key for a MERGES row's preview: the last terminal frame's stage
/// and status (moves whenever a retry appends a new frame) paired with the
/// worktree's branch (moves if the row's worktree identity itself changes,
/// which never happens in practice but keeps the key honest).
fn merge_preview_cache_key(row: &MergeRow) -> (String, String) {
    let frame_identity = row
        .last_frame
        .as_ref()
        .map(|frame| format!("{:?}/{:?}", frame.stage, frame.status))
        .unwrap_or_else(|| "none".to_string());
    let branch = row
        .worktree
        .as_ref()
        .map(|worktree| worktree.branch.clone())
        .unwrap_or_default();
    (frame_identity, branch)
}

/// IO edge (§3.4): resolves git facts (merge-base, changed paths, commit
/// count) for a same-repository row with a still-registered worktree, then
/// hands off to the pure [`merge_preview_lines`] for rendering. Never claims
/// a produced-artifact fact it could not compute — a foreign-repository row
/// or an unregistered worktree renders "gone"/unavailable, never guessed.
fn build_merge_preview(row: &MergeRow, cache_key: (String, String)) -> MergePreview {
    let explanation = row.last_frame.as_ref().map(merge_story::explain_frame);
    let gate_rows = row
        .last_frame
        .as_ref()
        .map(|frame| merge_story::gate_rows(&frame.evidence))
        .unwrap_or_default();
    let worktree_path = resolve_merge_worktree_path(row);
    let produced = worktree_path.as_ref().and_then(|path| {
        merge_produced(
            path,
            row.worktree
                .as_ref()
                .map(|worktree| worktree.branch.as_str()),
        )
    });
    let facts = MergePreviewFacts {
        run_id: row.run_id.clone(),
        phase: row.phase.clone(),
        trait_id: row.trait_id.clone(),
        class: row.class,
        stage: row.stage,
        produced,
        explanation,
        gate_rows,
        worktree_path: worktree_path.as_ref().map(|path| path.to_string()),
        worktree_branch: row
            .worktree
            .as_ref()
            .map(|worktree| worktree.branch.clone()),
    };
    let lines = merge_preview_lines(&facts)
        .iter()
        .map(tui_ratatui::render_line)
        .collect();
    MergePreview {
        session_id: row.session_id.clone(),
        cache_key,
        lines,
    }
}

/// Classifies what a run produced (§3.4 point 2): `merge_base(main, branch)`
/// then `changed_paths` then a commit count — never run for a row whose
/// worktree could not be resolved (the caller already degraded to `None`).
fn merge_produced(worktree_path: &camino::Utf8Path, branch: Option<&str>) -> Option<MergeProduced> {
    let branch = branch?;
    let repo_root = ctx_traits_io::repository::discover_repo_root().ok()?;
    let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
    let default_branch =
        ctx_traits_io::worktree::resolve_default_branch(&repo_root, None, &mut warnings)
            .ok()?
            .0;
    let base =
        ctx_traits_io::worktree::merge_base(&repo_root, &default_branch, branch, &mut warnings)
            .ok()?;
    let changed =
        ctx_traits_io::worktree::changed_paths(worktree_path, &base, "HEAD", &mut warnings).ok()?;
    if changed.is_empty() {
        return Some(MergeProduced::Nothing);
    }
    let docs_only = changed.iter().all(|path| path.ends_with(".md"));
    if docs_only {
        return Some(MergeProduced::DocsOnly {
            files: changed.len(),
        });
    }
    let commits = ctx_traits_io::worktree::commits_touching_paths(
        worktree_path,
        &base,
        "HEAD",
        &[],
        &mut warnings,
    )
    .ok()?;
    Some(MergeProduced::Commits {
        commits: commits.len(),
        files: changed.len(),
    })
}

/// Pure renderer (§3.4, directly unit-testable): no IO, just `facts` ->
/// styled lines.
fn merge_preview_lines(facts: &MergePreviewFacts) -> Vec<tui::Line> {
    let mut lines = Vec::new();

    let mut header = tui::Line::blank();
    header.push(format!("run {} ", facts.run_id), tui::Tone::Bold);
    if let Some(phase) = &facts.phase {
        header.push(format!("phase {phase} "), tui::Tone::Muted);
    }
    header.push(format!("trait {}", facts.trait_id), tui::Tone::Muted);
    lines.push(header);
    lines.push(tui::Line::blank());

    let mut class_line = tui::Line::blank();
    class_line.push("status: ", tui::Tone::Muted);
    let tone = match facts.class {
        MergeClass::Landed => tui::Tone::Pass,
        MergeClass::Failed => tui::Tone::Fail,
        MergeClass::Parked => tui::Tone::Warn,
        MergeClass::Mergeable => tui::Tone::Default,
    };
    class_line.push(facts.class.label().to_string(), tone);
    lines.push(class_line);

    let mut produced_line = tui::Line::blank();
    produced_line.push("produced: ", tui::Tone::Muted);
    produced_line.push(
        match &facts.produced {
            Some(MergeProduced::Nothing) => "nothing".to_string(),
            Some(MergeProduced::DocsOnly { files }) => format!("docs only ({files} file(s))"),
            Some(MergeProduced::Commits { commits, files }) => {
                format!("{commits} commit(s), {files} file(s)")
            }
            None => "(unavailable — worktree not registered or unreachable)".to_string(),
        },
        tui::Tone::Default,
    );
    lines.push(produced_line);

    let mut stage_line = tui::Line::blank();
    stage_line.push("reached: ", tui::Tone::Muted);
    stage_line.push(
        facts
            .stage
            .map(merge_story::stage_sentence)
            .unwrap_or("no merge attempted yet")
            .to_string(),
        tui::Tone::Default,
    );
    lines.push(stage_line);

    if let Some(explanation) = &facts.explanation
        && (facts.class == MergeClass::Parked || facts.class == MergeClass::Failed)
    {
        lines.push(tui::Line::blank());
        let mut why_line = tui::Line::blank();
        why_line.push("why: ", tui::Tone::Muted);
        why_line.push(explanation.sentence.clone(), tui::Tone::Fail);
        lines.push(why_line);
        let mut next_line = tui::Line::blank();
        next_line.push("next: ", tui::Tone::Muted);
        next_line.push(explanation.next_action.clone(), tui::Tone::Default);
        lines.push(next_line);
    }

    if !facts.gate_rows.is_empty() {
        lines.push(tui::Line::blank());
        let mut evidence_header = tui::Line::blank();
        evidence_header.push("evidence:", tui::Tone::Muted);
        lines.push(evidence_header);
        for row in &facts.gate_rows {
            let mut line = tui::Line::blank();
            line.push(format!("  {}: ", row.label), tui::Tone::Muted);
            line.push(row.value.clone(), tui::Tone::Default);
            lines.push(line);
        }
    }

    lines.push(tui::Line::blank());
    let mut worktree_line = tui::Line::blank();
    worktree_line.push("worktree: ", tui::Tone::Muted);
    worktree_line.push(
        facts
            .worktree_path
            .clone()
            .unwrap_or_else(|| "(gone — not registered)".to_string()),
        tui::Tone::Default,
    );
    lines.push(worktree_line);
    if let Some(branch) = &facts.worktree_branch {
        let mut branch_line = tui::Line::blank();
        branch_line.push("branch: ", tui::Tone::Muted);
        branch_line.push(branch.clone(), tui::Tone::Default);
        lines.push(branch_line);
    }

    lines
}

// ---------------------------------------------------------------------------
// SESSIONS: verbs (P469 §3.3–3.6)
// ---------------------------------------------------------------------------

/// P081: `Enter` on a SESSIONS row records an attach request (session id +
/// ledger path) rather than switching this dashboard into an in-process
/// attached-pane mode. `run_with_initial_session`'s loop drains the request
/// and hands the terminal to an observer `run_view::RunPanel` — the same
/// renderer `--progress tui` builds — until it detaches or the run finishes.
fn attach_selected(state: &mut State) {
    let Some(row) = selected_session(state) else {
        return;
    };
    if !row.class.can_attach() {
        state.message = Some(format!(
            "attach refused: session {} is unreadable",
            state_short_session(state, &row.session_id)
        ));
        return;
    }
    state.attach_request = Some(AttachRequest {
        session_id: row.session_id.clone(),
        ledger_path: row.ledger_path.clone(),
    });
}

/// `x`: opens the KILL confirm modal for the selected row. Refuses outright
/// (no modal) when the row is not currently `Live` — opening a modal that
/// cannot proceed would be less honest than refusing before it opens.
fn open_kill_modal(state: &mut State) {
    let Some(row) = selected_session(state) else {
        return;
    };
    let session_id = row.session_id.clone();
    let display_id = state_short_session(state, &session_id);
    let (title, body) = match ctx_traits_io::run_control::probe(&row.ledger_path) {
        Ok(ctx_traits_io::run_control::DriverProbe::Held(_)) => (
            "stop session",
            format!(
                "Request cooperative stop for {display_id}?\n\nIt finishes the current frame, then parks. This will not force-kill it."
            ),
        ),
        Ok(ctx_traits_io::run_control::DriverProbe::Unheld { .. }) if has_running_evidence(row) => {
            (
                "clear orphaned driver",
                format!(
                    "No driver holds {display_id}'s lock, but persisted evidence says a drive was running. Clear this orphaned driver evidence and mark the drive interrupted?"
                ),
            )
        }
        Ok(ctx_traits_io::run_control::DriverProbe::Unheld { .. })
            if row.status == Some(ctx_traits_core::procedure::session::Status::WaitingOnHuman) =>
        {
            (
                "cancel parked question",
                format!("Mark the unanswered question in {display_id} interrupted?"),
            )
        }
        Ok(ctx_traits_io::run_control::DriverProbe::Unheld { .. }) => {
            state.message = Some(format!(
                "stop refused: {display_id} has no driver; resume or delete it instead"
            ));
            return;
        }
        Err(error) => {
            state.message = Some(format!(
                "stop refused: could not probe {display_id}'s driver lock: {error}"
            ));
            return;
        }
    };
    state.modal_host.open(
        Action::Session(SessionAction::Kill(session_id)),
        Modal::confirm(title, body),
    );
}

fn open_answer_modal(state: &mut State) {
    let Some(row) = selected_session(state) else {
        return;
    };
    let display_id = state_short_session(state, &row.session_id);
    let session = match ctx_traits_io::run_session::read_run_session(&row.ledger_path) {
        Ok(session) => session,
        Err(error) => {
            state.message = Some(format!(
                "answer refused: could not read {display_id}: {error}"
            ));
            return;
        }
    };
    if matches!(
        session.last_drive_outcome.as_ref().map(|o| &o.outcome),
        Some(ctx_traits_core::procedure::session::DriveOutcomeKind::Interrupted)
    ) {
        state.message = Some(format!(
            "answer refused: {display_id}'s question was cancelled"
        ));
        return;
    }
    let Some(frame) = session.next_frame.as_ref().filter(|frame| {
        session.status == ctx_traits_core::procedure::session::Status::WaitingOnHuman
            && frame.kind == ctx_traits_core::procedure::runtime::SequenceFrameKind::Ask
    }) else {
        state.message = Some(format!(
            "answer refused: {display_id} is not waiting for a human"
        ));
        return;
    };
    let Some(output) = frame.requested_outputs.first() else {
        state.message = Some(format!(
            "answer refused: {display_id}'s question has no answer slot"
        ));
        return;
    };
    let trait_file = resolve_answer_trait_file(&session, row.repo_path.as_deref());
    let loaded = match ctx_traits_io::run::load_trait_for_session(
        trait_file.as_deref(),
        None,
        &session,
        "dashboard",
    ) {
        Ok(loaded) => loaded,
        Err(error) => {
            state.message = Some(format!(
                "answer unavailable: could not resolve question for {display_id}: {error}"
            ));
            return;
        }
    };
    let question = match resolved_human_question_body(&loaded, &session, frame) {
        Ok(body) => format!(
            "{body}\n---\nanswer slot: {} (schema: {})",
            output.slot_ref,
            output.schema_ref.as_deref().unwrap_or("schema:any")
        ),
        Err(error) => {
            state.message = Some(format!(
                "answer unavailable: could not resolve question for {display_id}: {error}"
            ));
            return;
        }
    };
    state.modal_host.open(
        Action::Session(SessionAction::Answer {
            session_id: row.session_id.clone(),
            state_digest: session.state_digest.as_str().to_string(),
            target: output.slot_ref.to_string(),
            schema_ref: output.schema_ref.clone(),
        }),
        Modal::text_input_with_body("answer question", question, "", true),
    );
}

fn has_running_evidence(row: &SessionRow) -> bool {
    ctx_traits_io::run_session::read_run_session(&row.ledger_path)
        .ok()
        .and_then(|session| session.last_drive_outcome)
        .is_some_and(|outcome| {
            outcome.outcome == ctx_traits_core::procedure::session::DriveOutcomeKind::Running
        })
}

/// `s`: opens the RESUME confirm modal for the selected row. Refuses outright
/// when the row is not `Resumable` (a live or terminal session cannot be
/// resumed).
fn open_resume_modal(state: &mut State) {
    let Some(row) = selected_session(state) else {
        return;
    };
    let can_resume = row.class.can_resume();
    let session_id = row.session_id.clone();
    let display_id = state_short_session(state, &session_id);
    let ledger_path = row.ledger_path.clone();
    if !can_resume {
        state.message = Some(format!(
            "resume refused: session {display_id} cannot be resumed from its current state"
        ));
        return;
    }
    let worktree_line = ctx_traits_io::run_session::read_run_session(&ledger_path)
        .ok()
        .and_then(|session| session.provenance.worktree)
        .map(|worktree| format!(" (worktree {}, branch {})", worktree.id, worktree.branch))
        .unwrap_or_default();
    let body = format!(
        "Resume {display_id}{worktree_line} as a detached `ctx traits drive` child, reusing its \
         recorded worktree provenance?"
    );
    state.modal_host.open(
        Action::Session(SessionAction::Resume(session_id)),
        Modal::confirm("resume session", body),
    );
}

/// `d`: opens the DELETE confirm modal for the selected row, listing every
/// artifact it will remove by exact path/ref first. Refuses outright on a
/// `Live` or `Resumable` row — DELETE is scoped to terminal sessions only.
fn open_delete_modal(state: &mut State) {
    let Some(row) = selected_session(state) else {
        return;
    };
    let session_id = row.session_id.clone();
    let display_id = state_short_session(state, &session_id);
    let ledger_path = row.ledger_path.clone();
    let repo_path = row.repo_path.clone();
    match ctx_traits_io::run_control::probe(&ledger_path) {
        Ok(ctx_traits_io::run_control::DriverProbe::Unheld { .. }) => {}
        Ok(ctx_traits_io::run_control::DriverProbe::Held(_)) => {
            state.message = Some(format!(
                "delete refused: {display_id}'s driver lock is held"
            ));
            return;
        }
        Err(error) => {
            state.message = Some(format!(
                "delete refused: could not probe {display_id}'s driver lock: {error}"
            ));
            return;
        }
    }
    let plan = plan_delete_for_ledger(&ledger_path, repo_path.as_deref());
    let warning = if row.class == SessionClass::Resumable {
        "Deleting discards resumable state.\n\n"
    } else if row.class == SessionClass::Unreadable {
        "Ledger provenance cannot be recovered; only known derivable artifacts will be touched.\n\n"
    } else {
        ""
    };
    let body = format!(
        "{warning}Delete the following?\n\n{}",
        plan.artifact_lines().join("\n")
    );
    state.modal_host.open(
        Action::Session(SessionAction::Delete(session_id, plan)),
        Modal::confirm("delete session", body),
    );
}

/// Resolves the delete plan's worktree location with exactly one git probe
/// (§3.4/§3.6), then hands off to the pure [`plan_delete`] to build the
/// artifact list. Only reached from `open_delete_modal` — an action edge,
/// never the draw path.
/// Resolves a DELETE/DROP plan's worktree location with exactly one git
/// probe (§3.4/§3.6), then hands off to the pure [`plan_delete`] to build the
/// artifact list. Narrowed to `(ledger_path, repo_path)` rather than
/// `&SessionRow` (P472 §3.5) so both SESSIONS' DELETE and MERGES' DROP call
/// it — a genuine extraction, not a copy.
fn plan_delete_for_ledger(ledger_path: &camino::Utf8Path, repo_path: Option<&str>) -> DeletePlan {
    let driver_lock = ctx_traits_io::run_control::driver_lock_path(ledger_path);
    let driver_lock_path = driver_lock.as_std_path().exists().then_some(driver_lock);
    let sidecars = ctx_traits_io::run_branch::sidecars_root(ledger_path);
    let sidecars_root = sidecars.as_std_path().exists().then_some(sidecars);
    let worktree = ctx_traits_io::run_session::read_run_session(ledger_path)
        .ok()
        .and_then(|session| session.provenance.worktree);
    let Some(worktree) = worktree else {
        return plan_delete(
            ledger_path,
            driver_lock_path,
            sidecars_root,
            None,
            true,
            None,
        );
    };
    let repo_root = ctx_traits_io::repository::discover_repo_root().ok();
    let same_repo =
        repo_path.is_none() || repo_root.as_ref().map(|root| root.as_str()) == repo_path;
    let verified = if same_repo {
        repo_root.as_ref().and_then(|root| {
            let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
            ctx_traits_io::worktree::verify_worktree_registration(
                &worktree.id,
                &worktree.branch,
                &mut warnings,
            )
            .ok()
            .map(|path| (root.clone(), path))
        })
    } else {
        None
    };
    plan_delete(
        ledger_path,
        driver_lock_path,
        sidecars_root,
        Some((worktree.id.as_str(), worktree.branch.as_str())),
        same_repo,
        verified,
    )
}

/// Pure artifact-list builder (§3.4, test-covered): `worktree_provenance` is
/// `None` when the ledger names no worktree; `verified` is the resolved
/// `(repo_root, worktree_path)` only when `same_repo` AND registration was
/// confirmed. No IO — every input is already resolved by the caller.
fn plan_delete(
    ledger_path: &camino::Utf8Path,
    driver_lock_path: Option<camino::Utf8PathBuf>,
    sidecars_root: Option<camino::Utf8PathBuf>,
    worktree_provenance: Option<(&str, &str)>,
    same_repo: bool,
    verified: Option<(camino::Utf8PathBuf, camino::Utf8PathBuf)>,
) -> DeletePlan {
    let (worktree, worktree_note) = match worktree_provenance {
        None => (None, None),
        Some((id, branch)) => {
            if !same_repo {
                (
                    None,
                    Some(format!(
                        "worktree {id} (branch {branch}) belongs to a different repository; left behind"
                    )),
                )
            } else if let Some((repo_root, path)) = verified {
                (Some((repo_root, path, branch.to_string())), None)
            } else {
                (
                    None,
                    Some(format!(
                        "worktree {id} (branch {branch}) is not registered; left behind"
                    )),
                )
            }
        }
    };
    DeletePlan {
        ledger_path: ledger_path.to_path_buf(),
        driver_lock_path,
        sidecars_root,
        worktree,
        worktree_note,
    }
}

/// Executes a confirmed [`DeletePlan`]: `remove_worktree` then `delete_branch`
/// (which uses `-d`, so git itself refuses an unmerged branch rather than
/// this code forcing `-D`), then the ledger and its known-orphaned siblings.
/// Reports every artifact's own outcome so a partial failure never claims
/// more than actually happened.
fn execute_delete(plan: &DeletePlan) -> String {
    let mut messages = Vec::new();
    if let Some((repo_root, path, branch)) = &plan.worktree {
        let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
        match ctx_traits_io::worktree::remove_worktree(repo_root, path, &mut warnings) {
            Ok(()) => {
                messages.push(format!("worktree {path} removed"));
                let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
                match ctx_traits_io::worktree::delete_branch(repo_root, branch, &mut warnings) {
                    Ok(()) => messages.push(format!("branch {branch} deleted")),
                    Err(error) => {
                        messages.push(format!("branch {branch} left in place: {error}"));
                    }
                }
            }
            Err(error) => {
                messages.push(format!("worktree {path} left in place: {error}"));
                messages.push(format!(
                    "branch {branch} left in place (worktree removal failed)"
                ));
            }
        }
    }
    match std::fs::remove_file(plan.ledger_path.as_std_path()) {
        Ok(()) => messages.push("ledger deleted".to_string()),
        Err(error) => messages.push(format!("ledger left in place: {error}")),
    }
    // Keep the lock file's inode stable. Removing it while holding flock would
    // let a new driver lock a replacement inode before this guard drops.
    if plan.driver_lock_path.is_some() {
        messages.push("driver lock retained (stable lock inode)".to_string());
    }
    if let Some(root) = &plan.sidecars_root {
        match std::fs::remove_dir_all(root.as_std_path()) {
            Ok(()) => messages.push("sidecars deleted".to_string()),
            Err(error) => messages.push(format!("sidecars left in place: {error}")),
        }
    }
    ctx_traits_io::run_summary::remove_summary_for_ledger(&plan.ledger_path);
    ctx_traits_io::activity_sidecar::remove_activity_for_ledger(&plan.ledger_path);
    messages.join("; ")
}

/// Resolve the current executable path as UTF-8 and the session-keyed log
/// path both dashboard spawn call sites share, rather than each
/// hand-rolling its own exe/log-path construction. `log_key` is the session
/// id when known ahead of spawn (RESUME); a spawn-request `ctx traits run`
/// mints its session id only after the child starts, so that call site
/// passes its own pid-keyed fallback instead.
fn resolve_spawn_exe_and_log(
    log_key: &str,
) -> crate::Result<(camino::Utf8PathBuf, camino::Utf8PathBuf)> {
    let exe = std::env::current_exe().map_err(|source| {
        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
            path: "<current-exe>".to_string(),
            source,
        })
    })?;
    let exe = camino::Utf8PathBuf::from_path_buf(exe).map_err(|_| crate::Error::Command {
        message: "current executable path is not UTF-8".to_string(),
    })?;
    let log_dir = ctx_traits_io::state::current_global_debug_root()?;
    std::fs::create_dir_all(log_dir.as_std_path()).ok();
    let log_path = log_dir.join(format!("dashboard-spawn-{log_key}.log"));
    Ok((exe, log_path))
}

/// `s` confirmed: spawns `ctx traits drive --session <id> --progress none`
/// detached (never `--worktree` — the drive's own `resolve_resume_worktree`
/// reuses the ledger's recorded provenance by construction), with `cwd` set
/// to the row's own repository path so ALL-mode resume is correct
/// cross-repo (§3.6).
fn spawn_resume(row: &SessionRow) -> crate::Result<()> {
    let (exe, log_path) = resolve_spawn_exe_and_log(&row.session_id)?;
    let cwd = match &row.repo_path {
        Some(path) => camino::Utf8PathBuf::from(path.clone()),
        None => super::lifecycle_reporting::current_utf8_dir()?,
    };
    let args = resume_argv(&row.session_id);
    ctx_traits_io::process::spawn_detached(
        &exe,
        &args,
        &cwd,
        &log_path,
        &[(
            ctx_traits_io::run_liveness::SPAWNED_LOG_PATH_ENV,
            log_path.as_str(),
        )],
    )?;
    Ok(())
}

/// The exact argv RESUME spawns: `traits drive --session <id> --progress
/// none`, deliberately with no `--worktree` (§3.5).
fn resume_argv(session_id: &str) -> Vec<String> {
    vec![
        "traits".to_string(),
        "drive".to_string(),
        "--session".to_string(),
        session_id.to_string(),
        "--progress".to_string(),
        "none".to_string(),
    ]
}

/// Applies a resolved SESSIONS modal outcome (§3.3): a `Cancelled` outcome
/// never mutates anything for any tag, including `Exit` — cancelling leaves
/// `quit == false` and sets no stop flag (the pane's own
/// `CtrlCPolicy::ForwardKey` already guarantees the latter). Every other
/// outcome re-looks-up its row by `session_id` and re-checks classification
/// before acting (test 6: identity survives a reload).
/// Top-level dispatcher for a resolved `Action` (P471's `Action`
/// generalization of P469's `SessionAction`): a `Cancelled` outcome never
/// mutates anything for any tag, including `Exit` — cancelling leaves
/// `quit == false` and sets no stop flag. Every other outcome routes to its
/// own family's applier, which re-looks-up its target by identity and
/// re-checks eligibility before acting.
/// The footer message for a `Cancelled` outcome (P473 §1 note 1, pure and
/// unit-tested): a trust modal (TRAITS' or TRUST's `a`/`b`/`A`) reports
/// these exact words — nothing was written — every other screen keeps the
/// generic "cancelled".
fn cancel_message(tag: &Action) -> String {
    match tag {
        Action::Trait(_) => "no trust change recorded".to_string(),
        _ => "cancelled".to_string(),
    }
}

fn apply_action(
    pane: &mut RatatuiPane,
    state: &mut State,
    tag: Action,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    // Reconcile/split queue steps handle `Cancelled` themselves — a reject
    // still advances the queue and opens the next modal, unlike every other
    // `Action`, where `Cancelled` is a dead end. So these two route BEFORE
    // the generic early return below, not through it.
    match tag {
        Action::Task(TaskAction::ReconcileStep { proposal, digest }) => {
            return apply_reconcile_step(state, proposal, digest, outcome);
        }
        Action::Task(TaskAction::SplitStep { parent, child }) => {
            return apply_split_step(state, parent, child, outcome);
        }
        _ => {}
    }
    if outcome == ModalOutcome::Cancelled {
        state.message = Some(cancel_message(&tag));
        return Ok(());
    }
    match tag {
        Action::Exit => {
            if outcome == ModalOutcome::Confirmed {
                state.quit = true;
            }
            Ok(())
        }
        Action::Session(action) => apply_session_action(pane, state, action, outcome),
        Action::Trait(action) => apply_trait_action(state, action, outcome),
        Action::Merge(action) => apply_merge_action(state, action, outcome),
        Action::Task(action) => apply_task_action(state, action, outcome),
        Action::AttachFailed => Ok(()),
    }
}

fn apply_session_action(
    pane: &mut RatatuiPane,
    state: &mut State,
    tag: SessionAction,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    let _ = pane;
    // `Cancelled` is already filtered out by `apply_action` before any tag
    // reaches here. Kill/Resume/Delete are `Confirm` modals, which resolve to
    // exactly `Confirmed` at this point, so there is nothing left for those
    // branches to switch on; `Spawn` is a `TextInput` modal and reads its
    // typed text from `outcome` directly.
    if let SessionAction::Spawn = tag {
        let ModalOutcome::Submitted(text) = outcome else {
            return Ok(());
        };
        return apply_spawn_request(state, text);
    }
    match tag {
        SessionAction::Spawn => unreachable!("handled above"),
        SessionAction::Answer {
            session_id,
            state_digest,
            target,
            schema_ref,
        } => {
            let ModalOutcome::Submitted(text) = outcome else {
                return Ok(());
            };
            let display_id = state_short_session(state, &session_id);
            let Some(row) = state
                .sessions
                .iter()
                .find(|row| row.session_id == session_id)
            else {
                state.message = Some(format!(
                    "answer refused: session {display_id} is no longer listed"
                ));
                return Ok(());
            };
            let Some(maintenance) =
                ctx_traits_io::run_control::try_acquire_maintenance(&row.ledger_path)?
            else {
                state.message = Some(format!(
                    "answer refused: {display_id}'s driver lock is held"
                ));
                state.reload();
                return Ok(());
            };
            let current = ctx_traits_io::run_session::read_run_session(&row.ledger_path)?;
            // Re-check against the *derived* state (blocker 2, P509), not raw
            // `status`: `record_interrupted_outcome` never rewrites `status`
            // away from `WaitingOnHuman`, so a cancel that lands between
            // modal open and submit is only visible through the outcome the
            // derivation folds in. This closes the TOCTOU the raw-status
            // check left open.
            let current_outcome = current.last_drive_outcome.as_ref().map(|o| &o.outcome);
            let current_state = ctx_traits_core::procedure::activity::SessionState::derive(
                &current.status,
                current_outcome,
                false,
            );
            let valid = current_state
                == ctx_traits_core::procedure::activity::SessionState::WaitingOnHuman
                && current.state_digest.as_str() == state_digest
                && current.next_frame.as_ref().is_some_and(|frame| {
                    frame.kind == ctx_traits_core::procedure::runtime::SequenceFrameKind::Ask
                        && frame.requested_outputs.first().is_some_and(|output| {
                            output.slot_ref.to_string() == target && output.schema_ref == schema_ref
                        })
                });
            if !valid {
                let message = if current_state
                    == ctx_traits_core::procedure::activity::SessionState::Cancelled
                {
                    format!("answer refused: {display_id}'s question was cancelled")
                } else {
                    format!("answer refused: {display_id}'s question changed; reopen it")
                };
                state.message = Some(message);
                state.reload();
                return Ok(());
            }
            let value = if schema_ref.as_deref() == Some("schema:text") {
                serde_json::Value::String(text)
            } else {
                match serde_json::from_str(&text) {
                    Ok(value) => value,
                    Err(error) => {
                        state.message = Some(format!(
                            "answer rejected: enter JSON for {}: {error}",
                            schema_ref.as_deref().unwrap_or("schema:any")
                        ));
                        return Ok(());
                    }
                }
            };
            let answer_trait_file = resolve_answer_trait_file(&current, row.repo_path.as_deref());
            let result = ctx_traits_io::run::set(ctx_traits_io::run::SetRequest {
                trait_file: answer_trait_file.as_deref(),
                trait_id: None,
                session: row.ledger_path.as_str(),
                session_store: None,
                target: &target,
                value,
                out: None,
                caller: ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "dashboard".to_string(),
                    caller: "ctx traits dashboard".to_string(),
                    agent: None,
                    harness: None,
                },
                existing_input_evidence: "ctx traits dashboard answer",
            });
            drop(maintenance);
            match result {
                Ok(ctx_traits_io::run::SetOutcome::Call { response, .. })
                    if response.response_kind == ctx_traits_core::procedure::session::CallResponseKind::RejectedCorrectionRequired => {
                    state.message = Some("answer rejected; correct it and reopen the question".to_string());
                }
                Ok(ctx_traits_io::run::SetOutcome::Call { response, .. })
                    if response.session.status == ctx_traits_core::procedure::session::Status::Completed => {
                    state.message = Some(format!("answer accepted; {display_id} completed"));
                }
                Ok(ctx_traits_io::run::SetOutcome::Call { .. }) => match spawn_resume(row) {
                    Ok(()) => state.message = Some(format!("answer accepted; resume started for {display_id}")),
                    Err(error) => state.message = Some(format!("answer accepted, but resume failed: {error}")),
                },
                Ok(ctx_traits_io::run::SetOutcome::Session { .. }) => {
                    state.message = Some("answer refused: question did not route to its current frame".to_string());
                }
                Err(error) => state.message = Some(format!("answer rejected: {error}")),
            }
            state.reload();
        }
        SessionAction::Kill(session_id) => {
            let display_id = state_short_session(state, &session_id);
            let Some(row) = state
                .sessions
                .iter()
                .find(|row| row.session_id == session_id)
            else {
                state.message = Some(format!(
                    "stop refused: session {display_id} is no longer listed"
                ));
                return Ok(());
            };
            let ledger_path = row.ledger_path.clone();
            state.message = Some(match ctx_traits_io::run_control::probe(&ledger_path)? {
                ctx_traits_io::run_control::DriverProbe::Held(_) => {
                    if ctx_traits_io::run_control::request_interrupt(&ledger_path)? {
                        format!("stop requested for {display_id}")
                    } else {
                        format!(
                            "stop refused: {display_id}'s driver did not acknowledge the request"
                        )
                    }
                }
                ctx_traits_io::run_control::DriverProbe::Unheld { .. }
                    if has_running_evidence(row)
                        || row.status
                            == Some(
                                ctx_traits_core::procedure::session::Status::WaitingOnHuman,
                            ) =>
                {
                    let Some(mut maintenance) =
                        ctx_traits_io::run_control::try_acquire_maintenance(&ledger_path)?
                    else {
                        state.reload();
                        return Ok(());
                    };
                    ctx_traits_io::run_session::record_interrupted_outcome(&ledger_path)?;
                    maintenance.clear_stale_metadata()?;
                    let _ = ctx_traits_io::run_liveness::remove_row(
                        &ctx_traits_io::run_control::runtime_root(),
                        &session_id,
                    );
                    format!("recorded interrupted outcome for {display_id}")
                }
                ctx_traits_io::run_control::DriverProbe::Unheld { .. } => {
                    format!("stop refused: {display_id} has no driver; resume or delete it instead")
                }
            });
            state.reload();
        }
        SessionAction::Resume(session_id) => {
            let display_id = state_short_session(state, &session_id);
            let Some(row) = state
                .sessions
                .iter()
                .find(|row| row.session_id == session_id)
            else {
                state.message = Some(format!(
                    "resume refused: session {display_id} is no longer listed"
                ));
                return Ok(());
            };
            if !row.class.can_resume() {
                state.message = Some(format!(
                    "resume refused: session {display_id} can no longer be resumed"
                ));
                return Ok(());
            }
            match spawn_resume(row) {
                Ok(()) => {
                    state.message = Some(format!("resume started for {display_id}"));
                    state.session_preview = None;
                    state.attach_request = Some(AttachRequest {
                        session_id: row.session_id.clone(),
                        ledger_path: row.ledger_path.clone(),
                    });
                    state.reload();
                }
                Err(error) => {
                    state.message = Some(format!("resume failed: {error}"));
                }
            }
        }
        SessionAction::Delete(session_id, plan) => {
            let display_id = state_short_session(state, &session_id);
            let Some(row) = state
                .sessions
                .iter()
                .find(|row| row.session_id == session_id)
            else {
                state.message = Some(format!(
                    "delete refused: session {display_id} is no longer listed"
                ));
                return Ok(());
            };
            let Some(mut maintenance) =
                ctx_traits_io::run_control::try_acquire_maintenance(&row.ledger_path)?
            else {
                state.message = Some(format!(
                    "delete refused: {display_id}'s driver lock is now held"
                ));
                state.reload();
                return Ok(());
            };
            let refreshed = plan_delete_for_ledger(&row.ledger_path, row.repo_path.as_deref());
            if refreshed.artifact_lines() != plan.artifact_lines() {
                state.message =
                    Some("delete plan changed; review the refreshed artifact list".to_string());
                state.reload();
                return Ok(());
            }
            maintenance.clear_stale_metadata()?;
            state.message = Some(execute_delete(&refreshed));
            state.reload();
        }
    }
    Ok(())
}

/// `m`/`d`: opens the retry confirm modal for the selected MERGES row,
/// naming the run and warning that the merge runs synchronously on the UI
/// thread (§3.5 risk 2 — moving it off-thread is new behavior, out of this
/// phase's scope). Refuses outright — no modal — on a `Landed` row, which
/// has nothing left to retry.
fn open_merge_retry_modal(state: &mut State, deep: bool) {
    let Some(row) = state.merges.get(state.selected()) else {
        return;
    };
    let session_id = row.session_id.clone();
    let display_id = state_short_session(state, &session_id);
    if !row.class.can_retry() {
        state.message = Some(format!(
            "merge refused: session {display_id} is already landed"
        ));
        return;
    }
    let verb = if deep { "deep-merge" } else { "merge" };
    let body = format!(
        "Retry {verb} for {}?\n\nThe UI is unresponsive while the gate runs — this is not backgrounded.",
        display_id
    );
    state.modal_host.open(
        Action::Merge(MergeAction::Retry {
            run_id: row.run_id.clone(),
            deep,
        }),
        Modal::confirm(if deep { "deep-merge" } else { "merge" }, body),
    );
}

/// `p`: writes the selected row's worktree path into the footer message
/// line. No clipboard — the house rules forbid a new dependency for it; the
/// path is also visible in the detail pane for manual selection.
fn print_merge_worktree_path(state: &mut State) {
    let Some(row) = state.merges.get(state.selected()) else {
        return;
    };
    match resolve_merge_worktree_path(row) {
        Some(path) => state.message = Some(format!("worktree: {path}")),
        None => {
            state.message = Some(format!(
                "worktree unavailable: session {} has no registered worktree",
                state_short_session(state, &row.session_id)
            ));
        }
    }
}

/// Resolves the selected row's worktree path via one git registration probe
/// — `None` when the row names no worktree, belongs to a foreign repository
/// (ALL mode), or is no longer registered.
fn resolve_merge_worktree_path(row: &MergeRow) -> Option<camino::Utf8PathBuf> {
    let worktree = row.worktree.as_ref()?;
    let repo_root = ctx_traits_io::repository::discover_repo_root().ok()?;
    let same_repo = row.repo_path.is_none() || row.repo_path.as_deref() == Some(repo_root.as_str());
    if !same_repo {
        return None;
    }
    let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
    ctx_traits_io::worktree::verify_worktree_registration(
        &worktree.id,
        &worktree.branch,
        &mut warnings,
    )
    .ok()
}

/// `x`: opens the DROP confirm modal for the selected MERGES row, reusing
/// SESSIONS' own `plan_delete`/`DeletePlan` mechanism unchanged (§3.5) so
/// there is exactly one destructive artifact-removal path. Refuses outright
/// on a `Mergeable` row — a driver may still hold it.
fn open_merge_drop_modal(state: &mut State) {
    let Some(row) = state.merges.get(state.selected()) else {
        return;
    };
    let display_id = state_short_session(state, &row.session_id);
    if !row.class.can_drop() {
        state.message = Some(format!(
            "drop refused: session {display_id} is still mergeable"
        ));
        return;
    }
    let plan = plan_delete_for_ledger(&row.ledger_path, row.repo_path.as_deref());
    let body = format!(
        "Drop the following from the merge queue?\n\n{}",
        plan.artifact_lines().join("\n")
    );
    state.modal_host.open(
        Action::Merge(MergeAction::Drop {
            session_id: row.session_id.clone(),
            plan,
        }),
        Modal::confirm("drop from queue", body),
    );
}

/// Applies a resolved MERGES modal (§3.5): `Cancelled` is handled by the
/// caller (`apply_action`). Every tag re-looks-up its row by `session_id`
/// and re-checks eligibility before acting (mirrors `apply_session_action`'s
/// identity-addressed re-lookup).
fn apply_merge_action(
    state: &mut State,
    tag: MergeAction,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    let _ = outcome;
    match tag {
        MergeAction::Retry { run_id, deep } => {
            let Some(row) = state.merges.iter().find(|row| row.run_id == run_id) else {
                state.message = Some(format!("run {run_id} is no longer listed"));
                return Ok(());
            };
            if !row.class.can_retry() {
                state.message = Some(format!("run {run_id} is already landed"));
                return Ok(());
            }
            state.message = Some(format!(
                "{} {run_id}\u{2026}",
                if deep { "deep-merging" } else { "merging" }
            ));
            let report = merge(MergeInputs {
                run_id: &run_id,
                session_store: None,
                session_path_override: None,
                assignments: &[],
                no_wait: false,
                force_wait: false,
                json: false,
                force_merger: false,
                park_on_overlap: false,
                force_land_on_overlap: false,
                allow_stale_overlap: false,
                deep,
                live: None,
                merger_stdout_observer: None,
            })?;
            let explanation = merge_story::explain_report(&report);
            state.message = Some(if report.status == "merged" {
                format!("merge {run_id}: landed")
            } else {
                format!("merge {run_id}: {}", explanation.sentence)
            });
            state.reload();
        }
        MergeAction::Drop { session_id, plan } => {
            let display_id = state_short_session(state, &session_id);
            let Some(row) = state.merges.iter().find(|row| row.session_id == session_id) else {
                state.message = Some(format!(
                    "drop refused: session {display_id} is no longer listed"
                ));
                return Ok(());
            };
            if !row.class.can_drop() {
                state.message = Some(format!(
                    "drop refused: session {display_id} is still mergeable"
                ));
                return Ok(());
            }
            state.message = Some(execute_delete(&plan));
            state.reload();
        }
    }
    Ok(())
}

fn trust_verb(verdict: ctx_traits_io::trust::TrustState) -> &'static str {
    match verdict {
        ctx_traits_io::trust::TrustState::Verified => "approve",
        ctx_traits_io::trust::TrustState::Blocked => "block",
    }
}

/// First 12 chars after a `sha256:`-style prefix, for compact modal-body
/// enumeration — never used for the actual write, only display.
fn short_digest(digest: &str) -> String {
    let bare = digest
        .split_once(':')
        .map(|(_, rest)| rest)
        .unwrap_or(digest);
    bare.chars().take(12).collect()
}

/// Bound on how many members a block-approve modal body enumerates before
/// collapsing the remainder into an explicit `+N more` count (P489: never a
/// silent cut).
const TRUST_MODAL_BODY_CAP: usize = 8;

/// `a`/`b` on TRAITS: opens the reason modal under an identity-bound
/// `Action::Trait(TraitAction::Trust)` tag with exactly one member (§4.4).
/// Refuses outright — no modal — when the row is unreadable (no canonical
/// digest was ever computed): opening a modal that cannot proceed is less
/// honest than refusing before it opens (the P469 `open_kill_modal`
/// precedent).
fn open_trait_trust_modal(state: &mut State, verdict: ctx_traits_io::trust::TrustState) {
    let Some(row) = state.traits.get(state.selected()) else {
        return;
    };
    if row.canonical_digest.is_empty() {
        state.message = Some(format!("{} is unreadable; nothing to trust", row.id));
        return;
    }
    state.modal_host.open(
        Action::Trait(TraitAction::Trust {
            label: row.id.clone(),
            members: vec![(row.id.clone(), row.canonical_digest.clone())],
            verdict,
        }),
        Modal::text_input(format!("{} {}", trust_verb(verdict), row.id), "", false),
    );
}

/// `a`/`b` on TRUST: single-member identity-bound write, sharing the same
/// `Action::Trait(TraitAction::Trust)` tag TRAITS uses (§4.7 — one write
/// path). Refuses outright — no modal — for an orphan row (`trait_id: None`)
/// or a row with no current digest (§4.6).
fn open_trust_modal(state: &mut State, verdict: ctx_traits_io::trust::TrustState) {
    let Some(row) = selected_trust(state) else {
        return;
    };
    let Some(trait_id) = &row.trait_id else {
        state.message = Some("orphaned record; nothing to approve".to_string());
        return;
    };
    if row.current_digest.is_empty() {
        state.message = Some(format!("{trait_id} is unreadable; nothing to trust"));
        return;
    }
    state.modal_host.open(
        Action::Trait(TraitAction::Trust {
            label: trait_id.clone(),
            members: vec![(trait_id.clone(), row.current_digest.clone())],
            verdict,
        }),
        Modal::text_input(format!("{} {trait_id}", trust_verb(verdict)), "", false),
    );
}

/// `A` on TRUST: family-level block-approve (§4.6) — one modal, one reason,
/// applied to every member of `metadata.family`, keyed off `state.trust`
/// (not a package layout) so P450's later variants-map fold needs no rework.
/// Refuses outright when the row has no family (no degenerating into `a`) or
/// is an orphan/unreadable row. The modal body enumerates every member and
/// its class — bounded, with an explicit `+N more` count, never a silent
/// cut — so the blast radius is visible before any keypress.
fn open_trust_family_modal(state: &mut State, verdict: ctx_traits_io::trust::TrustState) {
    let Some(row) = selected_trust(state) else {
        return;
    };
    if row.trait_id.is_none() {
        state.message = Some("orphaned record; nothing to approve".to_string());
        return;
    }
    let Some(family) = row.family.clone() else {
        state.message =
            Some("no family on this trait — use `a`/`b` for a single approve".to_string());
        return;
    };
    let members: Vec<&TrustRow> = state
        .trust
        .iter()
        .filter(|member| member.family.as_deref() == Some(family.as_str()))
        .filter(|member| member.trait_id.is_some() && !member.current_digest.is_empty())
        .collect();
    if members.is_empty() {
        state.message = Some(format!("no readable members in family {family}"));
        return;
    }
    let (body_lines, write_members) = trust_block_approve_body(&members);
    state.modal_host.open(
        Action::Trait(TraitAction::Trust {
            label: family.clone(),
            members: write_members,
            verdict,
        }),
        Modal::text_input_with_body(
            format!("{} family {family}", trust_verb(verdict)),
            body_lines.join("\n"),
            "",
            false,
        ),
    );
}

/// `A` on TRUST when marks exist (P506 §3.6): the same block-approve write
/// path as [`open_trust_family_modal`], sourced from [`State::trust_marks`]
/// instead of `metadata.family` — a different source of the member list, not
/// a second apply path. Refuses outright when no marked row still resolves
/// to a readable trait.
fn open_trust_marked_modal(state: &mut State, verdict: ctx_traits_io::trust::TrustState) {
    let members: Vec<&TrustRow> = state
        .trust
        .iter()
        .filter(|row| {
            row.trait_id
                .as_deref()
                .is_some_and(|id| state.trust_marks.contains(&id.to_string()))
        })
        .filter(|row| !row.current_digest.is_empty())
        .collect();
    if members.is_empty() {
        state.message = Some("no readable marked members to approve".to_string());
        return;
    }
    let (body_lines, write_members) = trust_block_approve_body(&members);
    let label = format!("{} marked", write_members.len());
    state.modal_host.open(
        Action::Trait(TraitAction::Trust {
            label: label.clone(),
            members: write_members,
            verdict,
        }),
        Modal::text_input_with_body(
            format!("{} {label} traits", trust_verb(verdict)),
            body_lines.join("\n"),
            "",
            false,
        ),
    );
}

/// The block-approve modal body/write-set builder shared by
/// [`open_trust_family_modal`] and [`open_trust_marked_modal`] (P506 §3.6):
/// bounded enumeration with an explicit `+N more` count — never a silent cut
/// — plus the `(trait_id, digest-captured-now)` pairs [`apply_trait_action`]
/// re-checks before writing.
fn trust_block_approve_body(members: &[&TrustRow]) -> (Vec<String>, Vec<(String, String)>) {
    let mut body_lines: Vec<String> = members
        .iter()
        .take(TRUST_MODAL_BODY_CAP)
        .map(|member| {
            format!(
                "{} {} ({})",
                member.trait_id.as_deref().unwrap_or(""),
                short_digest(&member.current_digest),
                member.class.label()
            )
        })
        .collect();
    if members.len() > TRUST_MODAL_BODY_CAP {
        body_lines.push(format!("+{} more", members.len() - TRUST_MODAL_BODY_CAP));
    }
    let write_members: Vec<(String, String)> = members
        .iter()
        .filter_map(|member| Some((member.trait_id.clone()?, member.current_digest.clone())))
        .collect();
    (body_lines, write_members)
}

/// Applies a resolved TRAITS/TRUST trust modal (§4.4, unified for N members
/// by §4.7): `Cancelled` is handled by the caller (`apply_action`); only
/// `Submitted` ever writes. Every member is re-looked-up in `state.trust`
/// (the superset of all tiers both screens share) and the WHOLE set aborts,
/// naming the offending member, if any one digest moved out from under the
/// open modal — fail-closed, so nobody ever approves bytes they did not see.
fn apply_trait_action(
    state: &mut State,
    action: TraitAction,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    let ModalOutcome::Submitted(reason) = outcome else {
        return Ok(());
    };
    let reason = Some(reason).filter(|text| !text.trim().is_empty());
    let TraitAction::Trust {
        label,
        members,
        verdict,
    } = action;
    for (trait_id, captured_digest) in &members {
        match decide_member_apply(&state.trust, trait_id, captured_digest) {
            TrustApplyDecision::RowGone => {
                state.message = Some(format!(
                    "{trait_id} is no longer listed; no trust change recorded"
                ));
                return Ok(());
            }
            TrustApplyDecision::DigestMoved { captured, current } => {
                state.message = Some(format!(
                    "{trait_id}'s digest moved since this reason was opened (was {captured}, now {current}); no trust change recorded — re-open to re-approve"
                ));
                return Ok(());
            }
            TrustApplyDecision::Proceed => {}
        }
    }
    let updates: Vec<ctx_traits_io::trust::DigestTrustUpdate> = members
        .iter()
        .map(|(trait_id, digest)| {
            ctx_traits_io::trust::DigestTrustUpdate::named(
                trait_id.clone(),
                digest.clone(),
                verdict,
                reason.clone(),
            )
        })
        .collect();
    ctx_traits_io::trust::update_digests_locked(&updates)?;
    state.message = Some(format!("trust {label} -> {}", verdict.as_str()));
    // Every write path (single `a`/`b`, family `A`, marked `A`) lands here —
    // clearing marks unconditionally after any successful write means a
    // block-approve never leaves stale marks that would silently re-apply on
    // the next `A` press.
    state.trust_marks.clear();
    state.reload();
    // Only the writing screen's own preview needs a forced rebuild here —
    // `state.reload()` already rebuilds whichever preview belongs to
    // `state.screen` via `State::reload`'s own per-screen dispatch; the
    // other screen's preview is rebuilt for free when it is next selected.
    match state.screen {
        Screen::Traits => force_rebuild_trait_preview_for_selection(state),
        Screen::Trust => force_rebuild_trust_preview_for_selection(state),
        _ => {}
    }
    Ok(())
}

/// `e`: opens `$EDITOR` on the selected trait's authored source, then
/// (§4.6) re-locates it by `trait_id` after the reload — never by index —
/// and force-rebuilds the preview so the drift/error facts refresh even
/// though the canonical digest itself never moves from an authored-source
/// edit.
fn edit_selected_trait_source(pane: &mut RatatuiPane, state: &mut State) -> crate::Result<()> {
    let Some(row) = state.traits.get(state.selected()) else {
        return Ok(());
    };
    let Some(path) = dashboard_trait_editable_source(&row.source_path) else {
        state.message = Some(format!("{} has no editable authored source", row.id));
        return Ok(());
    };
    let trait_id = row.id.clone();
    let ok = tui_kit::edit_file(pane, &path)?;
    state.message = Some(if ok {
        format!("edited {}", path)
    } else {
        format!("editor exited nonzero for {}", path)
    });
    state.reload();
    match reposition_trait_selection(&state.traits, &trait_id) {
        Some(idx) => state.list_traits.set_selected(idx),
        None => state.message = Some(format!("{trait_id} is no longer listed")),
    }
    force_rebuild_trait_preview_for_selection(state);
    Ok(())
}

/// Explicitly checks for an already-generated advisory explanation. This
/// never runs during selection or preview refresh, keeping those paths free
/// of model work. A cache miss remains an in-pane failure until a configured
/// narrator is available.
fn explain_selected_trait(state: &mut State) {
    let Some(row) = state.traits.get(state.selected()) else {
        return;
    };
    let request = worker::ExplanationRequest {
        trait_id: row.id.clone(),
        canonical_digest: row.canonical_digest.clone(),
        canonical_path: row.source_path.clone(),
    };
    state.trait_explanation = Some((
        request.trait_id.clone(),
        request.canonical_digest.clone(),
        Err("working...".to_string()),
        std::time::Instant::now(),
    ));
    if let Some(worker) = &state.worker {
        worker.explain(request);
    }
}

/// `n`: opens the kit's own multi-line text-input modal seeded with a
/// one-argument-per-line template (P506 §3.5 — the `$EDITOR` temp-file round
/// trip is gone). Submission is resolved through [`apply_spawn_request`].
fn open_spawn_modal(state: &mut State) {
    let seed = "# One argument per line. First non-comment line is the trait id.\n\
                # Example:\n\
                # my-trait\n\
                # --set\n\
                # code-diff=...\n";
    state.modal_host.open(
        Action::Session(SessionAction::Spawn),
        Modal::text_input("spawn run", seed, true),
    );
}

/// Validates a submitted spawn request through the real clap parser, injects
/// `--progress none`, and detaches a `ctx traits run` child. Never runs
/// anything the parser itself would not accept as a plain `ctx traits run`
/// invocation.
fn apply_spawn_request(state: &mut State, text: String) -> crate::Result<()> {
    let user_args: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if user_args.is_empty() {
        state.message = Some("spawn request was empty".to_string());
        return Ok(());
    }
    const FORBIDDEN: &[&str] = &[
        "--no-drive",
        "--ephemeral",
        "--out",
        "--session-store",
        "--json",
        "--progress",
    ];
    for arg in &user_args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if FORBIDDEN.contains(&flag) {
            state.message = Some(format!(
                "{flag} is not permitted in a dashboard spawn request"
            ));
            return Ok(());
        }
    }
    let mut full_argv: Vec<std::ffi::OsString> = vec!["ctx".into(), "traits".into(), "run".into()];
    full_argv.extend(user_args.iter().map(std::ffi::OsString::from));
    full_argv.push("--progress".into());
    full_argv.push("none".into());
    match super::surface::cli::parse(full_argv.clone()) {
        Ok(Some(super::surface::cli::Command::Traits {
            subcommand: Some(super::surface::cli::TraitsCommand::Run { .. }),
            ..
        })) => {}
        Ok(_) => {
            state.message = Some("spawn request did not parse as `ctx traits run`".to_string());
            return Ok(());
        }
        Err(error) => {
            state.message = Some(format!("spawn request rejected: {error}"));
            return Ok(());
        }
    }
    // No session id exists yet (minted by the child once it starts), so this
    // call site keys its log by pid instead of session id — the one place
    // the shared `resolve_spawn_exe_and_log` helper's session-keyed naming
    // (§3.8) cannot apply.
    let (exe, log_path) = resolve_spawn_exe_and_log(&format!("pid-{}", std::process::id()))?;
    let cwd = super::lifecycle_reporting::current_utf8_dir()?;
    let mut args: Vec<String> = vec!["traits".to_string(), "run".to_string()];
    args.extend(user_args);
    args.push("--progress".to_string());
    args.push("none".to_string());
    let _child = ctx_traits_io::process::spawn_detached(
        &exe,
        &args,
        &cwd,
        &log_path,
        &[(
            ctx_traits_io::run_liveness::SPAWNED_LOG_PATH_ENV,
            log_path.as_str(),
        )],
    )?;
    state.message = Some("spawn started".to_string());
    state.reload();
    Ok(())
}

// ---------------------------------------------------------------------------
// TASKS (0063)
// ---------------------------------------------------------------------------

/// Pure `(derived status, joined session rows) -> TaskGroup` mapping,
/// decided in exactly one place: any joined row currently live outranks
/// everything (in-flight), any joined row currently pending outranks a
/// blocked/ready/done board status (parked), and only once neither applies
/// does the board's own derived status decide. A run keyed to a parent while
/// doing a child's work joins the parent only — a child with no joined runs
/// of its own is simply not in-flight, which is the accepted shape (0063's
/// own Watch), not a bug here.
fn task_group(derived: DerivedStatus, joined: &[&SessionRow]) -> TaskGroup {
    let group_of =
        |row: &&SessionRow| session_group(row.class, row.status.as_ref(), row.outcome.as_ref());
    if joined.iter().any(|row| group_of(row) == SessionGroup::Live) {
        return TaskGroup::InFlight;
    }
    if joined
        .iter()
        .any(|row| group_of(row) == SessionGroup::Pending)
    {
        return TaskGroup::Parked;
    }
    match derived {
        DerivedStatus::Blocked => TaskGroup::Blocked,
        DerivedStatus::Ready => TaskGroup::Ready,
        DerivedStatus::Done | DerivedStatus::Cancelled => TaskGroup::Done,
    }
}

/// `task_key -> session indices` for the CURRENT repository only (0063's own
/// risk: with `v` ALL-repos scope on, a sibling repository's session can
/// carry the same task key for a different board — a row is joined only when
/// its own `repo_path` is `None` (the default, current-repository-only scope)
/// or matches the current repository root exactly).
fn task_session_join(state: &State) -> std::collections::HashMap<String, Vec<usize>> {
    let repo_root = super::command_handlers::resolve_repo_root(None)
        .ok()
        .map(|path| path.to_string());
    let mut map: std::collections::HashMap<String, Vec<usize>> = std::collections::HashMap::new();
    for (idx, row) in state.sessions.iter().enumerate() {
        let Some(key) = &row.task_key else { continue };
        let same_repo = row.repo_path.is_none() || row.repo_path == repo_root;
        if !same_repo {
            continue;
        }
        map.entry(key.clone()).or_default().push(idx);
    }
    map
}

/// Rebuilds [`State::tasks_visible`] from `state.tasks_board` and
/// `state.sessions` — called after every sync, every reload (the join
/// overlay refreshes on the existing 2s session-poll cadence against a fixed
/// board snapshot), and every collapse toggle, never in the draw path. A
/// `None` board cache clears the visible list rather than showing anything —
/// the screen is empty until the first `s`.
fn rebuild_visible_tasks(state: &mut State) {
    // 0063.7: auto-refresh reshuffles groups on any tick, not just on a
    // keypress — preserve the selected task's identity across the rebuild
    // rather than letting the cursor jump to whatever row now sits at the
    // old numeric index.
    let selected_key = match state.tasks_visible.get(state.list_tasks.selected()) {
        Some(TaskVisibleRow::Task(key)) => Some(key.clone()),
        _ => None,
    };
    let Some(summaries) = state
        .tasks_board
        .as_ref()
        .map(|board| board.summaries.clone())
    else {
        state.tasks_visible = Vec::new();
        state.list_tasks.set_len(0);
        return;
    };
    let join = task_session_join(state);
    let duplicate_keys = state
        .tasks_board
        .as_ref()
        .map(|board| board.sync_report.duplicate_keys.clone())
        .unwrap_or_default();
    let mut runs: Vec<(Option<&str>, &str, Option<&str>)> = Vec::new();
    for (key, indices) in &join {
        for idx in indices {
            let Some(row) = state.sessions.get(*idx) else {
                continue;
            };
            runs.push((
                Some(key.as_str()),
                row.run_id.as_str(),
                row.merged_landed.as_deref(),
            ));
        }
    }
    let proposals = super::task_proposals::derive_proposals(&runs, &summaries, &duplicate_keys);
    state.task_proposals = proposals
        .into_iter()
        .map(|proposal| (proposal.task_key.clone(), proposal))
        .collect();
    let mut buckets: Vec<(TaskGroup, Vec<String>)> = TaskGroup::order()
        .into_iter()
        .map(|group| (group, Vec::new()))
        .collect();
    for summary in &summaries {
        let joined_rows: Vec<&SessionRow> = join
            .get(&summary.key)
            .into_iter()
            .flatten()
            .filter_map(|idx| state.sessions.get(*idx))
            .collect();
        let group = task_group(summary.derived_status, &joined_rows);
        if let Some((_, keys)) = buckets.iter_mut().find(|(g, _)| *g == group) {
            keys.push(summary.key.clone());
        }
    }
    let mut visible = Vec::new();
    for (group, keys) in buckets {
        let collapsed = state.collapsed_task_groups.contains(&group);
        visible.push(TaskVisibleRow::GroupHeader {
            group,
            count: keys.len(),
            collapsed,
        });
        if !collapsed {
            visible.extend(keys.into_iter().map(TaskVisibleRow::Task));
        }
    }
    state.tasks_visible = visible;
    state.list_tasks.set_len(state.tasks_visible.len());
    if let Some(key) = selected_key
        && let Some(index) = state
            .tasks_visible
            .iter()
            .position(|row| matches!(row, TaskVisibleRow::Task(candidate) if candidate == &key))
    {
        state.list_tasks.set_selected(index);
    }
}

/// The ONLY accessor TASKS' action keys route through, mirroring
/// [`selected_session`]: `None` when the cursor sits on a group header (no
/// verb of its own) or the board cache is empty.
fn selected_task(state: &State) -> Option<&TaskSummary> {
    let board = state.tasks_board.as_ref()?;
    match state.tasks_visible.get(state.list_tasks.selected())? {
        TaskVisibleRow::Task(key) => board.summaries.iter().find(|s| &s.key == key),
        TaskVisibleRow::GroupHeader { .. } => None,
    }
}

/// Enter/space on a TASKS group header: flips its collapsed state and
/// rebuilds the visible-row list, mirroring [`toggle_selected_group`].
fn toggle_selected_task_group(state: &mut State) {
    let Some(TaskVisibleRow::GroupHeader { group, .. }) =
        state.tasks_visible.get(state.list_tasks.selected())
    else {
        return;
    };
    let group = *group;
    if !state.collapsed_task_groups.remove(&group) {
        state.collapsed_task_groups.insert(group);
    }
    rebuild_visible_tasks(state);
}

fn wall_clock_now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// One board read: `list` + `get` per key + `sync`, plus the fingerprint
/// sweep a later tick compares against. The one place a live board read
/// happens — shared by startup, the `s` keypress, and the 2s tick sweep.
fn read_board_snapshot(dir: &camino::Utf8Path) -> Result<TasksBoardSnapshot, String> {
    let provider = FilesTaskBoard::open_read(dir.to_owned());
    let summaries = provider.list(false).map_err(|error| error.to_string())?;
    let mut resolved = BTreeMap::new();
    for summary in &summaries {
        if let Ok(Some(task)) = provider.get(&summary.key) {
            resolved.insert(summary.key.clone(), task);
        }
    }
    let sync_report = provider.sync().unwrap_or_default();
    let fingerprint = task_files::board_fingerprint(dir).map_err(|error| error.to_string())?;
    Ok(TasksBoardSnapshot {
        summaries,
        resolved,
        sync_report,
        captured_at: wall_clock_now_secs(),
        fingerprint,
    })
}

/// The persisted snapshot's cache root: `state.tasks_cache_root` if a test
/// has overridden it, else the real per-repository cache root. Kept as a
/// seam so no test writes through `current_repo_key()` into the user's real
/// `~/.config/ctx/cache`.
fn tasks_cache_root(state: &State) -> Option<camino::Utf8PathBuf> {
    if let Some(root) = &state.tasks_cache_root {
        return Some(root.clone());
    }
    ctx_traits_io::state::current_global_cache_root().ok()
}

/// Persist `board` to `cache_root` (0063.7). Best-effort: a write failure
/// never surfaces to the user — the cache is derived evidence, not
/// authority, and the in-memory board just read is what the screen renders
/// regardless of whether the write lands.
fn persist_board_snapshot(
    cache_root: &camino::Utf8Path,
    dir: &camino::Utf8Path,
    board: &TasksBoardSnapshot,
) {
    let record = BoardSnapshotRecord::new(
        dir,
        board.captured_at,
        board.summaries.clone(),
        board.resolved.clone(),
        board.sync_report.clone(),
        board.fingerprint.clone(),
    );
    let _ = task_board_cache::write_snapshot(cache_root, &record);
}

/// `s`: forces a re-read regardless of the tick's own fingerprint check —
/// useful against impatience or (once a remote backend exists) a slow
/// upstream. Synchronous — the board is a handful of small TOML files, well
/// under a tick.
fn sync_tasks_board(state: &mut State) {
    let dir = match super::tasks::board_dir(None) {
        Ok(dir) => dir,
        Err(error) => {
            state.message = Some(format!("sync failed: {error}"));
            return;
        }
    };
    sync_tasks_board_in(state, &dir);
}

fn sync_tasks_board_in(state: &mut State, dir: &camino::Utf8Path) {
    match apply_board_snapshot(state, dir) {
        Ok(clean) => {
            state.message = Some(if clean {
                "synced".to_string()
            } else {
                "synced — sync issues found, see task detail".to_string()
            });
        }
        Err(error) => {
            state.message = Some(format!("sync failed: {error}"));
        }
    }
}

/// Re-reads `dir` into `state.tasks_board` and its dependents, returning
/// whether the sync report was clean. Shared by `sync_tasks_board_in` (the
/// `s`-keypress path, which reports "synced" on top) and
/// `resync_tasks_board_after_write_in` (which must not touch
/// `state.message`, since the write that triggered it already set the
/// confirmation the owner needs to see).
fn apply_board_snapshot(state: &mut State, dir: &camino::Utf8Path) -> Result<bool, String> {
    match read_board_snapshot(dir) {
        Ok(board) => {
            let clean = board.sync_report.dangling_edges.is_empty()
                && board.sync_report.parse_failures.is_empty()
                && board.sync_report.duplicate_keys.is_empty();
            if let Some(cache_root) = tasks_cache_root(state) {
                persist_board_snapshot(&cache_root, dir, &board);
            }
            state.tasks_board = Some(board);
            state.tasks_refresh_error = None;
            rebuild_visible_tasks(state);
            refresh_task_preview_for_selection(state);
            Ok(clean)
        }
        Err(error) => {
            state.tasks_refresh_error = Some(error.clone());
            Err(error)
        }
    }
}

/// Resyncs the board after a write whose own success/refusal message is
/// already in `state.message` (e.g. "0146 marked done — …") — unlike
/// `sync_tasks_board`, this never overwrites that message with "synced".
/// Previously every write handler called `sync_tasks_board` right after
/// setting its confirmation, which `sync_tasks_board_in` immediately
/// clobbered with "synced" (or, worse, "sync failed: …" on a transient
/// re-read race), making a write that had just succeeded look like a
/// silent no-op or an outright failure on the dashboard footer. A resync
/// failure here is appended to the existing message instead of replacing
/// it, so the write confirmation always survives.
fn resync_tasks_board_after_write(state: &mut State) -> crate::Result<()> {
    let dir = super::tasks::board_dir(None)?;
    resync_tasks_board_after_write_in(state, &dir);
    Ok(())
}

fn resync_tasks_board_after_write_in(state: &mut State, dir: &camino::Utf8Path) {
    if let Err(error) = apply_board_snapshot(state, dir) {
        let prefix = state.message.take().unwrap_or_default();
        state.message = Some(format!("{prefix} (resync failed: {error})"));
    }
}

/// The 2s tick's board freshness check (0063.7): a stat sweep, then a
/// re-read only when the sweep disagrees with the last-captured fingerprint
/// (or there is no board yet). No parsing happens when nothing changed —
/// the sweep is the whole per-tick cost. A failed re-read never blanks the
/// screen: the previous `tasks_board` (if any) stays exactly as it was, with
/// `tasks_refresh_error` set so the title can note the failure.
fn refresh_tasks_board_if_stale(state: &mut State) {
    let dir = match super::tasks::board_dir(None) {
        Ok(dir) => dir,
        Err(error) => {
            state.tasks_refresh_error = Some(error.to_string());
            return;
        }
    };
    refresh_tasks_board_if_stale_in(state, &dir);
}

fn refresh_tasks_board_if_stale_in(state: &mut State, dir: &camino::Utf8Path) {
    let current_fingerprint = task_files::board_fingerprint(dir);
    let stale = match (&state.tasks_board, &current_fingerprint) {
        (Some(board), Ok(fingerprint)) => &board.fingerprint != fingerprint,
        _ => true,
    };
    if !stale {
        return;
    }
    match read_board_snapshot(dir) {
        Ok(board) => {
            if let Some(cache_root) = tasks_cache_root(state) {
                persist_board_snapshot(&cache_root, dir, &board);
            }
            state.tasks_board = Some(board);
            state.tasks_refresh_error = None;
            rebuild_visible_tasks(state);
            if state.screen == Screen::Tasks {
                refresh_task_preview_for_selection(state);
            }
        }
        Err(error) => {
            state.tasks_refresh_error = Some(error);
        }
    }
}

/// Startup population (0063.7): the persisted snapshot cache if it hits and
/// still names this exact `board_dir`, else one synchronous live read — the
/// screen opens populated either way, or with a visible failure rather than
/// silently empty.
fn load_tasks_board_at_startup(state: &mut State) {
    let dir = match super::tasks::board_dir(None) {
        Ok(dir) => dir,
        Err(error) => {
            state.tasks_refresh_error = Some(error.to_string());
            return;
        }
    };
    let cache_root = tasks_cache_root(state);
    if let Some(cache_root) = &cache_root
        && let Some(record) = task_board_cache::read_snapshot(cache_root, &dir)
    {
        state.tasks_board = Some(TasksBoardSnapshot {
            summaries: record.summaries,
            resolved: record.resolved,
            sync_report: record.sync_report,
            captured_at: record.captured_at,
            fingerprint: record.fingerprint,
        });
        state.tasks_refresh_error = None;
        rebuild_visible_tasks(state);
        return;
    }
    match read_board_snapshot(&dir) {
        Ok(board) => {
            if let Some(cache_root) = &cache_root {
                persist_board_snapshot(cache_root, &dir, &board);
            }
            state.tasks_board = Some(board);
            state.tasks_refresh_error = None;
            rebuild_visible_tasks(state);
        }
        Err(error) => {
            state.tasks_board = None;
            state.tasks_refresh_error = Some(error);
        }
    }
}

/// Rebuilds (or clears) [`State::task_preview`] for the currently selected
/// TASKS row — called on selection change and after every sync, never in the
/// draw path.
fn refresh_task_preview_for_selection(state: &mut State) {
    let Some(summary) = selected_task(state).cloned() else {
        state.task_preview = None;
        return;
    };
    let Some(board) = &state.tasks_board else {
        state.task_preview = None;
        return;
    };
    let join = task_session_join(state);
    let joined_rows: Vec<&SessionRow> = join
        .get(&summary.key)
        .into_iter()
        .flatten()
        .filter_map(|idx| state.sessions.get(*idx))
        .collect();
    let wrap_width = state
        .last_pane_layout
        .rect(PANE_TASKS_PREVIEW)
        .map_or(80, |rect| rect.width.saturating_sub(2));
    let proposal = state.task_proposals.get(&summary.key);
    state.task_preview = Some(build_task_preview(
        &summary,
        board,
        &joined_rows,
        proposal,
        wrap_width,
    ));
}

/// The TASKS detail pane (0063): status, relations resolved with the other
/// side's live status (both directions), open steps, then joined runs — a
/// parked run reachable from its task row — then, when 0063.8 derived a
/// pending merge-time done-proposal for this task, one line naming it next
/// to the joined-runs section. Pure over already-in-hand facts; no IO here
/// (the board read happened at `sync` time).
fn build_task_preview(
    summary: &TaskSummary,
    board: &TasksBoardSnapshot,
    joined: &[&SessionRow],
    proposal: Option<&super::task_proposals::DoneProposal>,
    wrap_width: u16,
) -> TaskPreview {
    let mut lines = Vec::new();
    let mut header = tui::Line::blank();
    header.push(format!("{} ", summary.key), tui::Tone::Bold);
    header.push(summary.title.clone(), tui::Tone::Default);
    lines.push(header);

    let mut status_line = tui::Line::blank();
    status_line.push("status: ", tui::Tone::Muted);
    status_line.push(
        super::tasks::status_text(summary.derived_status),
        tui::Tone::Default,
    );
    lines.push(status_line);
    for edge in &board.sync_report.dangling_edges {
        if edge.from == summary.key {
            let mut line = tui::Line::blank();
            line.push(
                format!("(dangling {}: {})", edge.field, edge.to),
                tui::Tone::Fail,
            );
            lines.push(line);
        }
    }
    lines.push(tui::Line::blank());

    let Some(resolved) = board.resolved.get(&summary.key) else {
        let mut line = tui::Line::blank();
        line.push(
            "(relations unavailable — resolve failed at the last sync)",
            tui::Tone::Fail,
        );
        lines.push(line);
        let rlines: Vec<RLine<'static>> = lines.iter().map(tui_ratatui::render_line).collect();
        return TaskPreview {
            key: summary.key.clone(),
            lines: tui_panes::wrapped_lines(&rlines, wrap_width),
        };
    };

    for (label, text) in [
        ("content", &resolved.document.content),
        ("scope", &resolved.document.scope),
        ("validation", &resolved.document.validation),
    ] {
        if text.trim().is_empty() {
            continue;
        }
        lines.push(tui::Line::blank());
        let mut label_line = tui::Line::blank();
        label_line.push(format!("{label}:"), tui::Tone::Muted);
        lines.push(label_line);
        let raw: Vec<&str> = text.split('\n').collect();
        let start = raw
            .iter()
            .position(|l| !l.trim().is_empty())
            .unwrap_or(raw.len());
        let end = raw
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(start, |i| i + 1);
        for raw_line in &raw[start..end] {
            let tone = if raw_line.trim_start().starts_with('#') {
                tui::Tone::Muted
            } else {
                tui::Tone::Default
            };
            let mut line = tui::Line::blank();
            line.push((*raw_line).to_string(), tone);
            lines.push(line);
        }
    }

    push_task_relation_lines(&mut lines, "blocked by", &resolved.relations.depends_on);
    push_task_relation_lines(&mut lines, "blocks", &resolved.relations.blocks);
    if let Some(parent) = &resolved.relations.parent {
        push_task_relation_lines(&mut lines, "parent", std::slice::from_ref(parent));
    }
    push_task_relation_lines(&mut lines, "children", &resolved.relations.children);

    lines.push(tui::Line::blank());
    let mut steps_header = tui::Line::blank();
    steps_header.push("open steps:", tui::Tone::Muted);
    lines.push(steps_header);
    if resolved.open_steps.is_empty() {
        let mut line = tui::Line::blank();
        line.push("  (none)", tui::Tone::Muted);
        lines.push(line);
    } else {
        for step in &resolved.open_steps {
            let mut line = tui::Line::blank();
            line.push(format!("  {} ", step.id), tui::Tone::Default);
            line.push(step.title.clone(), tui::Tone::Muted);
            lines.push(line);
        }
    }

    lines.push(tui::Line::blank());
    let mut runs_header = tui::Line::blank();
    runs_header.push("joined runs:", tui::Tone::Muted);
    lines.push(runs_header);
    if joined.is_empty() {
        let mut line = tui::Line::blank();
        line.push("  (none)", tui::Tone::Muted);
        lines.push(line);
    } else {
        for row in joined {
            let mut line = tui::Line::blank();
            let label = row.title.as_deref().unwrap_or(&row.session_id);
            line.push(format!("  {label} "), tui::Tone::Default);
            line.push(format!("({})", row.state_text), tui::Tone::Muted);
            lines.push(line);
        }
    }
    if let Some(proposal) = proposal {
        let mut line = tui::Line::blank();
        let citations = proposal
            .evidence
            .iter()
            .map(|evidence| format!("run {} merged as {}", evidence.run_id, evidence.sha))
            .collect::<Vec<_>>()
            .join("; ");
        line.push("  pending: ", tui::Tone::Muted);
        line.push(
            format!("{citations} — press y to mark done"),
            tui::Tone::Default,
        );
        lines.push(line);
    }

    let rlines: Vec<RLine<'static>> = lines.iter().map(tui_ratatui::render_line).collect();
    TaskPreview {
        key: summary.key.clone(),
        lines: tui_panes::wrapped_lines(&rlines, wrap_width),
    }
}

/// One relation direction's lines, `label edge.key (edge.title) [status] glyph`
/// per edge — a bare list of ids is useless; the status is the point (0063).
fn push_task_relation_lines(
    lines: &mut Vec<tui::Line>,
    label: &str,
    edges: &[ctx_traits_core::task::graph::ResolvedEdge],
) {
    if edges.is_empty() {
        return;
    }
    for edge in edges {
        let mut line = tui::Line::blank();
        let glyph = if edge.status.is_closed() {
            "\u{2713}"
        } else {
            "\u{2717}"
        };
        line.push(format!("{label}: "), tui::Tone::Muted);
        line.push(
            format!(
                "{} ({}) [{}] {glyph}",
                edge.key,
                edge.title,
                super::tasks::status_text(edge.status)
            ),
            tui::Tone::Default,
        );
        lines.push(line);
    }
}

/// `S`: split. When the selected task's latest bound `Blocked`
/// `implement-*` session carries a park report (or an oversized feasibility
/// verdict), one child per open blocker (or `missing` entry) is proposed,
/// confirmed one at a time (0064). Absent that evidence, falls back to
/// today's manual text-input modal for the child's title unchanged.
fn open_task_split_modal(state: &mut State) {
    let Some(summary) = selected_task(state) else {
        state.message = Some("no task selected".to_string());
        return;
    };
    let parent = summary.key.clone();
    let children = match latest_blocked_split_source(state, &parent) {
        Some(SplitSource::Park(report)) => split_children_from_park(&report),
        Some(SplitSource::OversizedFeasibility(verdict)) => {
            split_children_from_feasibility(&verdict)
        }
        None => Vec::new(),
    };
    if children.is_empty() {
        state.modal_host.open(
            Action::Task(TaskAction::Split {
                parent: parent.clone(),
            }),
            Modal::text_input(format!("split {parent} — child title"), "", false),
        );
        return;
    }
    state.split_queue = children;
    open_next_split_step(state, &parent);
}

/// `a`: archive — a text-input modal for the closing status (`done` or
/// `cancelled`), optionally followed by `release` to run the dependents
/// sweep (0063.6), defaulting to `done`. Reads the task fresh to capture
/// the digest the eventual write is validated against. When the task has
/// dependents in the last-synced board, the prompt names how many and
/// hints at the `release` token.
fn open_task_archive_modal(state: &mut State) {
    let Some(summary) = selected_task(state) else {
        state.message = Some("no task selected".to_string());
        return;
    };
    let key = summary.key.clone();
    let digest = match fetch_task_digest(&key) {
        Ok(digest) => digest,
        Err(error) => {
            state.message = Some(format!("archive refused: {error}"));
            return;
        }
    };
    let dependents = state
        .tasks_board
        .as_ref()
        .and_then(|board| board.resolved.get(&key))
        .map(|resolved| resolved.relations.blocks.len())
        .unwrap_or(0);
    let prompt = if dependents > 0 {
        format!(
            "archive {key} — done/cancelled ({dependents} task(s) depend on this — \
             add 'release' to release them)"
        )
    } else {
        format!("archive {key} — done/cancelled")
    };
    state.modal_host.open(
        Action::Task(TaskAction::Archive {
            key: key.clone(),
            digest,
        }),
        Modal::text_input(prompt, "done", false),
    );
}

/// `e`: edit — a text-input modal for the mini-grammar
/// [`parse_task_edit_input`] accepts. Reads the task fresh to capture the
/// digest the eventual write is validated against.
fn open_task_edit_modal(state: &mut State) {
    let Some(summary) = selected_task(state) else {
        state.message = Some("no task selected".to_string());
        return;
    };
    let key = summary.key.clone();
    let digest = match fetch_task_digest(&key) {
        Ok(digest) => digest,
        Err(error) => {
            state.message = Some(format!("edit refused: {error}"));
            return;
        }
    };
    state.modal_host.open(
        Action::Task(TaskAction::Edit {
            key: key.clone(),
            digest,
        }),
        Modal::text_input(
            format!("edit {key} — status <s> | dep +<k> | dep -<k> | dep <old> <new>"),
            "",
            false,
        ),
    );
}

/// `y`: accept a merge-time done-proposal (0063.8). Refuses inline (no
/// modal) when the selected task has no derived proposal or the board's
/// last-synced resolve has nothing for its key; otherwise reads the task
/// fresh (the digest the eventual write is validated against) and opens a
/// confirm showing the run id, merged sha per cited run, and the task's own
/// `validation` prose — the owner judges against the contract, not the
/// commit's existence.
fn open_task_mark_done_modal(state: &mut State) {
    let Some(summary) = selected_task(state) else {
        state.message = Some("no task selected".to_string());
        return;
    };
    let key = summary.key.clone();
    let Some(proposal) = state.task_proposals.get(&key).cloned() else {
        state.message = Some(format!("{key}: no merge-time done-proposal"));
        return;
    };
    let Some(resolved) = state
        .tasks_board
        .as_ref()
        .and_then(|board| board.resolved.get(&key))
    else {
        state.message = Some(format!("{key}: not resolvable at the last sync"));
        return;
    };
    let digest = match fetch_task_digest(&key) {
        Ok(digest) => digest,
        Err(error) => {
            state.message = Some(format!("mark done refused: {error}"));
            return;
        }
    };
    let mut body = String::new();
    for evidence in &proposal.evidence {
        body.push_str(&format!(
            "run {} for {key} merged as {} — mark done?\n",
            evidence.run_id, evidence.sha
        ));
    }
    if !resolved.document.validation.trim().is_empty() {
        body.push_str("\ndone-when:\n");
        body.push_str(resolved.document.validation.trim());
    }
    append_declared_checks_notice(&mut body, &resolved.document.checks);
    state.modal_host.open(
        Action::Task(TaskAction::MarkDone {
            key: key.clone(),
            digest,
            evidence: proposal.evidence.clone(),
        }),
        Modal::confirm(format!("mark {key} done"), body),
    );
}

/// 0144 trust surfacing: name the exact declared-check commands in the
/// confirm modal body, before `Confirmed` ever runs them — the same posture
/// as dispatching a trait per the task's own Watch. A task with no declared
/// checks leaves `body` untouched.
fn append_declared_checks_notice(body: &mut String, checks: &[ctx_traits_core::task::Check]) {
    if checks.is_empty() {
        return;
    }
    body.push_str("\n\ndeclared checks (run on confirm):\n");
    for check in checks {
        body.push_str(&format!("- {}: {}\n", check.name, check.command));
    }
}

/// The snapshot digest a modal-open captures for a task, by a fresh read —
/// not the (possibly stale) board cache — so the write it eventually backs
/// is validated against what is on disk right now.
fn fetch_task_digest(key: &str) -> crate::Result<String> {
    let dir = super::tasks::board_dir(None)?;
    let provider = FilesTaskBoard::open_read(dir);
    let resolved = provider
        .get(key)
        .map_err(|e| crate::Error::Command {
            message: e.to_string(),
        })?
        .ok_or_else(|| crate::Error::Command {
            message: format!("task {key} not found"),
        })?;
    Ok(resolved.digest)
}

/// `R`: builds a fresh [`super::task_proposals::ReconcileReport`] from a
/// live board read (`list(true)` so archived tasks are considered, per
/// 0064's Watch) plus this repository's ledger inventory, and opens the
/// first proposal's review modal. An empty report reports so inline, no
/// modal opened.
fn open_task_reconcile(state: &mut State) {
    let dir = match super::tasks::board_dir(None) {
        Ok(dir) => dir,
        Err(error) => {
            state.message = Some(format!("reconcile failed: {error}"));
            return;
        }
    };
    let provider = FilesTaskBoard::open_read(dir.clone());
    let summaries = match provider.list(true) {
        Ok(summaries) => summaries,
        Err(error) => {
            state.message = Some(format!("reconcile failed: {error}"));
            return;
        }
    };
    let sync_report = provider.sync().unwrap_or_default();
    let mut resolved = BTreeMap::new();
    for summary in &summaries {
        if let Ok(Some(task)) = provider.get(&summary.key) {
            resolved.insert(summary.key.clone(), task);
        }
    }
    let inventory = match ctx_traits_io::run_session::current_repo_run_inventory() {
        Ok(inventory) => inventory,
        Err(error) => {
            state.message = Some(format!("reconcile failed: {error}"));
            return;
        }
    };
    let facts = super::tasks::session_facts_from_inventory(&inventory);
    let report = super::task_proposals::derive_reconcile_report(
        &facts,
        &summaries,
        &resolved,
        &sync_report.duplicate_keys,
    );
    state.reconcile_ambiguous = report.ambiguous;
    state.reconcile_queue = report.proposals;
    if state.reconcile_queue.is_empty() {
        state.message = Some(reconcile_completion_message(state));
        return;
    }
    open_next_reconcile_step(state);
}

/// Pops the next queued reconcile proposal (if any) and opens its review
/// modal, fetching a fresh digest against the proposal's own task —
/// `apply_reconcile_step` calls this again after every resolution
/// (`Confirmed` or `Cancelled` alike), so the queue always ends either
/// empty or on an open modal.
fn open_next_reconcile_step(state: &mut State) {
    while !state.reconcile_queue.is_empty() {
        let proposal = state.reconcile_queue.remove(0);
        let task_key = proposal.task_key().to_string();
        let digest = match fetch_task_digest(&task_key) {
            Ok(digest) => digest,
            Err(error) => {
                state.message = Some(format!("reconcile: skipping {task_key} — {error}"));
                continue;
            }
        };
        let (title, body) = match &proposal {
            super::task_proposals::ReconcileProposal::MarkDone { task_key, evidence } => {
                let mut body = String::new();
                for e in evidence {
                    body.push_str(&format!(
                        "run {} for {task_key} merged as {} (verified ancestor of main) — mark done?\n",
                        e.run_id, e.sha
                    ));
                }
                if let Ok(dir) = super::tasks::board_dir(None)
                    && let Ok(Some(resolved)) = FilesTaskBoard::open_read(dir).get(task_key)
                {
                    append_declared_checks_notice(&mut body, &resolved.document.checks);
                }
                (format!("reconcile: mark {task_key} done"), body)
            }
            super::task_proposals::ReconcileProposal::RemoveDependsOn(remove) => (
                format!("reconcile: {}", remove.from),
                format!(
                    "remove depends-on {} ({}) — {}?",
                    remove.to,
                    super::tasks::status_text(remove.to_status),
                    remove.evidence
                ),
            ),
        };
        state.modal_host.open(
            Action::Task(TaskAction::ReconcileStep { proposal, digest }),
            Modal::confirm(title, body),
        );
        return;
    }
    state.message = Some(reconcile_completion_message(state));
}

fn reconcile_completion_message(state: &State) -> String {
    if state.reconcile_ambiguous.is_empty() {
        "reconcile: no ambiguous findings".to_string()
    } else {
        format!(
            "reconcile: {} ambiguous — {}",
            state.reconcile_ambiguous.len(),
            state
                .reconcile_ambiguous
                .iter()
                .map(|finding| format!("{} ({})", finding.task_key, finding.reason))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

/// `Confirmed`/`Cancelled` alike for a reconcile step: a reject just skips
/// (no write), an accept writes through the provider — `MarkDone` reuses
/// [`apply_task_mark_done`], `RemoveDependsOn` calls `update` directly with
/// `remove_depends_on` set. Either way, advances to the next queued
/// proposal.
fn apply_reconcile_step(
    state: &mut State,
    proposal: super::task_proposals::ReconcileProposal,
    digest: String,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    if outcome == ModalOutcome::Confirmed {
        match proposal {
            super::task_proposals::ReconcileProposal::MarkDone { task_key, evidence } => {
                apply_task_mark_done(state, task_key, digest, evidence)?;
            }
            super::task_proposals::ReconcileProposal::RemoveDependsOn(remove) => {
                let dir = super::tasks::board_dir(None)?;
                let provider = FilesTaskBoard::open_read_write(dir);
                match provider.update(
                    &remove.from,
                    TaskUpdate {
                        remove_depends_on: vec![remove.to.clone()],
                        expected_digest: Some(digest),
                        ..Default::default()
                    },
                ) {
                    Ok(outcome) => {
                        state.message = Some(format!(
                            "reconcile: removed depends-on {} from {}{}",
                            remove.to,
                            remove.from,
                            effects_summary(&outcome.effects)
                        ));
                        resync_tasks_board_after_write(state)?;
                    }
                    Err(error) => {
                        state.message = Some(format!("reconcile step refused: {error}"));
                    }
                }
            }
        }
    } else {
        state.message = Some("reconcile: skipped".to_string());
    }
    open_next_reconcile_step(state);
    Ok(())
}

/// The source a split-from-park-report queue is built from: the latest
/// bound `Blocked` `implement-*` session's typed park report, or (absent a
/// park report) its typed feasibility verdict when that verdict is
/// `oversized`.
enum SplitSource {
    Park(ctx_traits_io::run_session::ParkReportEntry),
    OversizedFeasibility(ctx_traits_io::run_session::FeasibilityVerdict),
}

/// The split source for `task_key`'s latest (by terminal epoch) bound
/// `Blocked` `implement-*` session, if any — read fresh from each
/// candidate's ledger (never cached), since this only runs on an `S`
/// keypress, not the tick path.
fn latest_blocked_split_source(state: &State, task_key: &str) -> Option<SplitSource> {
    let join = task_session_join(state);
    let mut best: Option<(u64, SplitSource)> = None;
    for row in join
        .get(task_key)
        .into_iter()
        .flatten()
        .filter_map(|idx| state.sessions.get(*idx))
        .filter(|row| row.status == Some(ctx_traits_core::procedure::session::Status::Blocked))
    {
        let Ok(session) = ctx_traits_io::run_session::read_run_session(&row.ledger_path) else {
            continue;
        };
        // No trait-id filter: the join above already scopes rows to sessions
        // that carried THIS task key, and any trait can be task-dispatched.
        let epoch = session
            .last_drive_outcome
            .as_ref()
            .map(|outcome| outcome.recorded_at_epoch)
            .unwrap_or(0);
        let source = if let Some(report) = ctx_traits_io::run_session::typed_park_report(&session) {
            Some(SplitSource::Park(report))
        } else {
            ctx_traits_io::run_session::typed_feasibility_verdict(&session)
                .filter(|verdict| verdict.verdict == "oversized")
                .map(SplitSource::OversizedFeasibility)
        };
        if let Some(source) = source
            && best
                .as_ref()
                .is_none_or(|(best_epoch, _)| epoch > *best_epoch)
        {
            best = Some((epoch, source));
        }
    }
    best.map(|(_, source)| source)
}

/// Blocker `what` truncated to a title-sized prefix (task titles are
/// conventionally one line) — the blocker's full text still lands in the
/// child's `content`, so nothing is lost, only what heads the row.
fn split_child_title(text: &str) -> String {
    const MAX: usize = 96;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(MAX).collect();
    format!("{}…", truncated.trim_end())
}

/// One [`PendingSplitChild`] per open blocker, title from `what`, content
/// from `what` + `root-cause` + `required-fix`, `validation` from
/// `done-when`, steps mapped one-for-one onto [`ctx_traits_core::task::Step`].
/// A blocker whose fix is already fully done (`ParkBlocker::is_open` false)
/// proposes no child — reconcile's own park-report reader already treats
/// its steps as evidence, and a closed blocker in a parked run is nothing
/// left to split off.
fn split_children_from_park(
    report: &ctx_traits_io::run_session::ParkReportEntry,
) -> Vec<PendingSplitChild> {
    report
        .blockers
        .iter()
        .filter(|blocker| blocker.is_open())
        .map(|blocker| PendingSplitChild {
            title: split_child_title(&blocker.what),
            content: format!(
                "{}\n\nRoot cause: {}\n\nRequired fix: {}",
                blocker.what.trim(),
                blocker.root_cause.trim(),
                blocker.required_fix.trim()
            ),
            validation: blocker.done_when.clone(),
            steps: blocker
                .steps
                .iter()
                .enumerate()
                .map(|(index, step)| ctx_traits_core::task::Step {
                    id: format!("step-{}", index + 1),
                    title: step.step.clone(),
                    done: step.status == "done",
                    content: step.evidence.clone(),
                })
                .collect(),
        })
        .collect()
}

/// One [`PendingSplitChild`] per `missing` entry of an oversized feasibility
/// verdict — no steps or done-when, since the verdict names what is
/// missing, not how to fix it.
fn split_children_from_feasibility(
    verdict: &ctx_traits_io::run_session::FeasibilityVerdict,
) -> Vec<PendingSplitChild> {
    verdict
        .missing
        .iter()
        .map(|missing| PendingSplitChild {
            title: split_child_title(missing),
            content: format!("{}\n\n{}", missing.trim(), verdict.evidence.trim()),
            validation: String::new(),
            steps: Vec::new(),
        })
        .collect()
}

/// Pops the next queued split child and opens its confirm modal, or reports
/// completion when the queue drains.
fn open_next_split_step(state: &mut State, parent: &str) {
    if state.split_queue.is_empty() {
        state.message = Some(format!("split from park report: done for {parent}"));
        return;
    }
    let child = state.split_queue.remove(0);
    let mut body = child.content.clone();
    if !child.validation.trim().is_empty() {
        body.push_str("\n\ndone-when:\n");
        body.push_str(child.validation.trim());
    }
    state.modal_host.open(
        Action::Task(TaskAction::SplitStep {
            parent: parent.to_string(),
            child: child.clone(),
        }),
        Modal::confirm(format!("split {parent} — create {:?}?", child.title), body),
    );
}

/// `Confirmed`/`Cancelled` alike for a split-queue step: a reject skips this
/// child with no write; an accept creates it via `TaskProviderMut::create`
/// with `parent` set, carrying `validation`/`steps` through (0064's one
/// provider-surface change). Either way, advances to the next queued child.
fn apply_split_step(
    state: &mut State,
    parent: String,
    child: PendingSplitChild,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    if outcome == ModalOutcome::Confirmed {
        let dir = super::tasks::board_dir(None)?;
        let provider = FilesTaskBoard::open_read_write(dir);
        match provider.create(NewTask {
            title: child.title.clone(),
            content: child.content.clone(),
            status: None,
            depends_on: Vec::new(),
            parent: Some(parent.clone()),
            validation: child.validation.clone(),
            steps: child.steps.clone(),
        }) {
            Ok(created) => {
                state.message = Some(format!("created {} under {parent}", created.key));
                resync_tasks_board_after_write(state)?;
            }
            Err(error) => {
                state.message = Some(format!("split refused: {error}"));
            }
        }
    } else {
        state.message = Some(format!("split: skipped {:?}", child.title));
    }
    open_next_split_step(state, &parent);
    Ok(())
}

fn apply_task_action(
    state: &mut State,
    action: TaskAction,
    outcome: ModalOutcome,
) -> crate::Result<()> {
    // `MarkDone` is a `Confirm` modal (`Confirmed`/`Cancelled`, `Cancelled`
    // already filtered by `apply_action`), unlike every other `TaskAction`,
    // which is a `TextInput` modal reading `Submitted(text)` — mirrors
    // `apply_session_action`'s own `Spawn` special-case split.
    if let TaskAction::MarkDone {
        key,
        digest,
        evidence,
    } = action
    {
        if outcome != ModalOutcome::Confirmed {
            return Ok(());
        }
        return apply_task_mark_done(state, key, digest, evidence);
    }
    let ModalOutcome::Submitted(text) = outcome else {
        return Ok(());
    };
    match action {
        TaskAction::MarkDone { .. } => unreachable!("handled above"),
        TaskAction::ReconcileStep { .. } | TaskAction::SplitStep { .. } => {
            unreachable!("apply_action routes reconcile/split queue steps before reaching here")
        }
        TaskAction::Split { parent } => {
            let title = text.trim();
            if title.is_empty() {
                state.message = Some("split refused: a title is required".to_string());
                return Ok(());
            }
            let dir = super::tasks::board_dir(None)?;
            let provider = FilesTaskBoard::open_read_write(dir);
            match provider.create(NewTask {
                title: title.to_string(),
                parent: Some(parent.clone()),
                ..Default::default()
            }) {
                Ok(created) => {
                    state.message = Some(format!("created {} under {parent}", created.key));
                    resync_tasks_board_after_write(state)?;
                }
                Err(error) => {
                    state.message = Some(format!("split refused: {error}"));
                }
            }
            Ok(())
        }
        TaskAction::Archive { key, digest } => {
            let (status, release_dependents) = match parse_task_archive_input(text.trim()) {
                Ok(parsed) => parsed,
                Err(reason) => {
                    state.message = Some(format!("archive refused: {reason}"));
                    return Ok(());
                }
            };
            let dir = super::tasks::board_dir(None)?;
            let provider = FilesTaskBoard::open_read_write(dir);
            match provider.update(
                &key,
                TaskUpdate {
                    status: Some(status),
                    expected_digest: Some(digest),
                    release_dependents,
                    ..Default::default()
                },
            ) {
                Ok(outcome) => {
                    state.message = Some(format!(
                        "archived {key}{}",
                        effects_summary(&outcome.effects)
                    ));
                    resync_tasks_board_after_write(state)?;
                }
                Err(error) => {
                    state.message = Some(format!("archive refused: {error}"));
                }
            }
            Ok(())
        }
        TaskAction::Edit { key, digest } => {
            let mut update = match parse_task_edit_input(text.trim()) {
                Ok(update) => update,
                Err(reason) => {
                    state.message = Some(format!("edit refused: {reason}"));
                    return Ok(());
                }
            };
            update.expected_digest = Some(digest);
            let dir = super::tasks::board_dir(None)?;
            let provider = FilesTaskBoard::open_read_write(dir);
            match provider.update(&key, update) {
                Ok(outcome) => {
                    state.message =
                        Some(format!("edited {key}{}", effects_summary(&outcome.effects)));
                    resync_tasks_board_after_write(state)?;
                }
                Err(error) => {
                    state.message = Some(format!("edit refused: {error}"));
                }
            }
            Ok(())
        }
    }
}

/// 0144: the effective `auto-close` policy for one task — its own
/// `auto_close` override wins over the `[tasks] auto-close` config leaf in
/// either direction ([`super::task_proposals::resolve_auto_close_policy`]).
/// `None` when neither is set — the existing confirm-only flow, unchanged.
fn resolve_task_close_policy(
    document: &ctx_traits_core::task::TaskDocument,
) -> Option<ctx_traits_core::task::AutoClosePolicy> {
    let config_default =
        ctx_traits_io::harness_config::resolve_runtime_config(camino::Utf8Path::new("."))
            .ok()
            .and_then(|config| config.effective_auto_close());
    super::task_proposals::resolve_auto_close_policy(document.auto_close, config_default)
}

/// Run `document.checks` against `sha` in a clean worktree, mapping a
/// whole-set failure (`UnrunnableSet`) to a single `Unrunnable` record so
/// [`super::task_proposals::close_disposition`] always has a uniform
/// `Vec<CheckRecord>` to reason about. Only called when there is at least
/// one declared check — the no-checks flow never reaches here.
fn run_declared_checks(
    checks: &[ctx_traits_core::task::Check],
    sha: &str,
) -> Vec<ctx_traits_core::task::CheckRecord> {
    let Ok(repo_root) = super::command_handlers::resolve_repo_root(None) else {
        return vec![ctx_traits_core::task::CheckRecord {
            name: "(check set)".to_string(),
            command: String::new(),
            outcome: ctx_traits_core::task::CheckOutcome::Unrunnable,
            detail: "could not resolve the repository root".to_string(),
        }];
    };
    match super::task_checks::run_checks(checks, &repo_root, sha) {
        Ok(records) => records,
        Err(unrunnable) => vec![ctx_traits_core::task::CheckRecord {
            name: "(check set)".to_string(),
            command: String::new(),
            outcome: ctx_traits_core::task::CheckOutcome::Unrunnable,
            detail: unrunnable.reason,
        }],
    }
}

/// `y`'s `Confirmed` write: `status: done` with the digest captured at
/// modal-open, folding the newest cited evidence into `origin` when the
/// document has none yet (0063.8's own ruling — `origin` today means "which
/// run raised this task"; silently overwriting an existing one would
/// destroy provenance to record provenance, so an already-set origin is
/// left untouched, and the evidence still lands in the message below
/// either way). Declared close effects (0063.6) run as usual, reported the
/// same way `Archive`/`Edit` already report them.
///
/// 0144: when the task declares checks and an `auto-close` policy resolves
/// (document override or `[tasks] auto-close`), the checks run here against
/// the cited sha before the write, and their disposition
/// ([`super::task_proposals::close_disposition`]) decides the outcome — a
/// disposition of `AutoClose` writes `set_closure` alongside `status:
/// done`; `Proposal` (a failing or un-runnable check under `checked`)
/// refuses the write and names why, leaving the task open for a later,
/// corrected attempt. A task with no declared checks, or no policy
/// configured, takes the exact path it took before this feature.
fn apply_task_mark_done(
    state: &mut State,
    key: String,
    digest: String,
    evidence: Vec<super::task_proposals::MergedRunEvidence>,
) -> crate::Result<()> {
    let dir = super::tasks::board_dir(None)?;
    apply_task_mark_done_in(state, &dir, key, digest, evidence)
}

fn apply_task_mark_done_in(
    state: &mut State,
    dir: &camino::Utf8Path,
    key: String,
    digest: String,
    evidence: Vec<super::task_proposals::MergedRunEvidence>,
) -> crate::Result<()> {
    let Some(latest) = evidence.last() else {
        state.message = Some(format!("mark done refused: {key} has no evidence"));
        return Ok(());
    };
    let provider = FilesTaskBoard::open_read_write(dir.to_owned());
    let current = provider.get(&key).ok().flatten();
    let has_origin = current
        .as_ref()
        .is_some_and(|resolved| resolved.document.origin.is_some());
    let set_origin = if has_origin {
        None
    } else {
        Some(Some(format!(
            "run {} merged as {}",
            latest.run_id, latest.sha
        )))
    };

    let mut set_closure = None;
    if let Some(resolved) = &current
        && !resolved.document.checks.is_empty()
        && let Some(policy) = resolve_task_close_policy(&resolved.document)
    {
        let results = run_declared_checks(&resolved.document.checks, &latest.sha);
        match super::task_proposals::close_disposition(
            policy,
            &resolved.document.checks,
            Some(&results),
        ) {
            super::task_proposals::CloseDisposition::AutoClose { checks } => {
                set_closure = Some(ctx_traits_core::task::Closure {
                    mode: policy,
                    commit: Some(latest.sha.clone()),
                    checks,
                });
            }
            super::task_proposals::CloseDisposition::Proposal { reason } => {
                state.message = Some(format!(
                    "{key}: not closed — {}",
                    reason.unwrap_or_else(|| "declared checks did not clear".to_string())
                ));
                return Ok(());
            }
        }
    }

    match provider.update(
        &key,
        TaskUpdate {
            status: Some(TaskDocStatus::Done),
            expected_digest: Some(digest),
            set_origin,
            set_closure,
            ..Default::default()
        },
    ) {
        Ok(outcome) => {
            state.message = Some(format!(
                "{key} marked done — run {} merged as {}{}",
                latest.run_id,
                latest.sha,
                effects_summary(&outcome.effects)
            ));
            resync_tasks_board_after_write_in(state, dir);
        }
        Err(error) => {
            state.message = Some(format!("mark done refused: {error}"));
        }
    }
    Ok(())
}

/// Fold recorded effects (0063.6) into the trailing clause of a dashboard
/// status message — "" when nothing beyond the field write happened, else
/// "; moved to archived/; released 0071, 0072; 0074 failed: <reason>".
fn effects_summary(effects: &[EffectRecord]) -> String {
    let mut clauses = Vec::new();
    for effect in effects {
        let joined = effect.documents.join(", ");
        match (&effect.effect, &effect.outcome) {
            (EffectKind::ArchivePlacement, EffectOutcome::Applied) => {
                clauses.push(format!("moved to {joined}"));
            }
            (EffectKind::ArchivePlacement, EffectOutcome::Failed { reason }) => {
                clauses.push(format!("archive placement failed: {reason}"));
            }
            (EffectKind::ReleaseDependents, EffectOutcome::Applied) => {
                clauses.push(format!("released {joined}"));
            }
            (EffectKind::ReleaseDependents, EffectOutcome::Failed { reason }) => {
                clauses.push(format!("{joined} {reason}"));
            }
        }
    }
    if clauses.is_empty() {
        String::new()
    } else {
        format!(" — {}", clauses.join("; "))
    }
}

/// `d`: dispatch. A blocked task refuses inline, with its reason in the
/// footer message, never a silently disabled key (0063's own Done-when
/// clause) — the same [`ctx_traits_io::dispatch_preflight::unmet_dependencies`]
/// filter and [`ctx_traits_io::dispatch_preflight::dependency_refusal_message`]
/// dispatch itself uses. A task the board has not resolved (never synced, or
/// the per-key `get` failed at the last sync) refuses the same way, naming
/// the reason, rather than silently opening the spawn modal against unknown
/// dependencies.
fn dispatch_selected_task(state: &mut State) {
    let Some(summary) = selected_task(state) else {
        state.message = Some("no task selected".to_string());
        return;
    };
    let key = summary.key.clone();
    let Some(board) = &state.tasks_board else {
        state.message = Some("not synced — press s before dispatching".to_string());
        return;
    };
    let Some(resolved) = board.resolved.get(&key) else {
        state.message = Some(format!(
            "{key} could not be resolved at the last sync — press s to retry"
        ));
        return;
    };
    if let Some(unmet) = ctx_traits_io::dispatch_preflight::unmet_dependencies(resolved) {
        state.message =
            Some(ctx_traits_io::dispatch_preflight::dependency_refusal_message(&key, &unmet));
        return;
    }
    open_spawn_modal_for_task(state, &key);
}

/// The existing spawn modal (§5050), seeded with `--set`/`task=<key>` lines
/// so submission flows through [`apply_spawn_request`]'s existing clap
/// validation and detached spawn unchanged — the run's own dispatch preflight
/// remains the real gate; this screen's check above is UX only, and
/// `--override-dependencies` stays reachable by editing the modal text.
/// 0063.4: the seed's first line is the `[tasks] dispatch-trait` config
/// default, editable before submit — resolved synchronously at keypress
/// (the same `resolve_runtime_config` entry point `run` itself uses), never
/// cached on `State`, so a config edit takes effect on the next dispatch.
/// Absent config, the modal opens as before (blank first line) but its
/// leading comment names exactly the missing key.
fn open_spawn_modal_for_task(state: &mut State, key: &str) {
    let dispatch_trait =
        ctx_traits_io::harness_config::resolve_runtime_config(camino::Utf8Path::new("."))
            .ok()
            .and_then(|config| config.effective_dispatch_trait());
    let seed = spawn_modal_seed(dispatch_trait.as_deref(), key);
    state.modal_host.open(
        Action::Session(SessionAction::Spawn),
        Modal::text_input("spawn run", seed, true),
    );
}

/// The spawn modal's seed text for a board dispatch: `dispatch_trait`
/// (`[tasks] dispatch-trait`, 0063.4) as the modal's first line when
/// configured — the spawn modal's own contract requires a trait id as the
/// first non-comment line — or, absent config, a leading comment naming
/// exactly the missing key so the owner is never left guessing.
fn spawn_modal_seed(dispatch_trait: Option<&str>, key: &str) -> String {
    // `--task-dispatch` is what makes this a BOARD dispatch: without it the
    // run treats `task=<key>` as plain port text and never binds, preflights,
    // or materialises the board document. This seed is the one place the
    // flow supplies it automatically.
    match dispatch_trait {
        Some(trait_id) => format!("{trait_id}\n--task-dispatch\n--set\ntask={key}\n"),
        None => format!(
            "# no dispatch trait configured — set [tasks] dispatch-trait in config.toml\n\n--task-dispatch\n--set\ntask={key}\n"
        ),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn draw_screen(pane: &mut RatatuiPane, state: &mut State) -> std::io::Result<()> {
    pane.draw(|frame| {
        let area = frame.area();
        // P550: while a story view is open, it replaces the screen's own
        // tree with a single full-area lines pane — precedent: the attached
        // session view is already a state-swapped rendering of the same
        // area. The footer still draws below it (`footer_line` branches on
        // `story_view` too) so the close hint stays visible.
        if let Some(view) = state.story_view.as_mut() {
            let regions = tui_panes::screen_regions(area);
            let inner = tui_panes::render_pane(frame, regions[1], &view.title, true);
            let styled = super::story::story_document(
                &view.session,
                &view.report,
                ctx_traits_core::procedure::story::StoryLevel::Default,
            );
            let rendered: Vec<ratatui::text::Line<'static>> =
                styled.iter().map(render_line).collect();
            let wrapped = tui_panes::wrapped_lines(&rendered, inner.width.max(1));
            view.scroll.set_len(wrapped.len());
            tui_panes::render_lines_pane(frame, inner, &wrapped, view.scroll);
            frame.render_widget(footer_line(state), regions[2]);
            if let Some(modal) = state.modal_host.modal() {
                tui_kit::render_modal(frame, area, modal);
            }
            return;
        }
        // P081: a live SESSIONS row's attach is a synchronous handoff to the
        // shared `run_view::RunPanel` observer (`run_with_initial_session`'s
        // attach loop) — this draw pass never renders a second, in-dashboard
        // attached body; the SESSIONS screen always draws its ordinary list
        // + preview tree below.
        let regions = tui_panes::screen_regions(area);
        let titles: Vec<String> = Screen::all()
            .into_iter()
            .enumerate()
            .map(|(idx, screen)| format!("[{}] {}", idx + 1, screen.title()))
            .collect();
        let current_idx = Screen::all()
            .iter()
            .position(|s| *s == state.screen)
            .unwrap_or(0);
        frame.render_widget(tui_panes::tab_bar(&titles, current_idx), regions[0]);

        let tree = build_tree_for_screen(state, regions[1].width);
        let resolved = tree.resolve(regions[1]);
        // Cached so `alt`+arrow directional focus movement in `handle_key`
        // reads the SAME rects this frame just drew, instead of
        // re-resolving the tree outside a draw pass.
        state.last_pane_layout = resolved.clone();
        // P552 review `live-run-pane-contract-absent`: SESSIONS' own outer
        // tree carries only ONE placeholder leaf (`PANE_SESSIONS_PREVIEW_REGION`)
        // for its whole progress/journey region — `run_view::pane_tree` is the
        // only code that ever creates and sizes the real `PANE_SESSIONS_PROGRESS`/
        // `PANE_SESSIONS_JOURNEY` leaves (inside `render_sessions_preview_body`
        // below), so focus reconciliation must fold THOSE leaf ids in here
        // rather than reconciling against the outer tree's own placeholder id.
        let focus_leaf_ids = if state.screen == Screen::Sessions {
            sessions_focus_leaf_ids(state, &resolved)
        } else {
            tree.leaf_ids()
        };
        // Reconciled against THIS tree — the one actually resolved at the
        // real terminal width — never a hypothetical maximum-width one, so
        // focus can never name a pane that has no rect this frame (P506
        // review: `focus-ring-includes-undrawn-panes`).
        state
            .focus
            .reconcile(focus_leaf_ids, list_pane_id(state.screen));
        for id in tree.leaf_ids() {
            clamp_visible_pane_scroll(state, id);
            if id == PANE_SESSIONS_PREVIEW_REGION {
                continue;
            }
            let Some(rect) = resolved.rect(id) else {
                continue;
            };
            let focused = state.focus.is_focused(id);
            let title = tree.title(id).unwrap_or(id).to_string();
            let inner = tui_panes::render_pane(frame, rect, &title, focused);
            render_pane_content(frame, id, inner, state);
        }
        if let Some(region_rect) = resolved.rect(PANE_SESSIONS_PREVIEW_REGION) {
            render_sessions_preview_body(frame, region_rect, state);
        }

        frame.render_widget(footer_line(state), regions[2]);
        if let Some(modal) = state.modal_host.modal() {
            tui_kit::render_modal(frame, area, modal);
        }
    })
}

/// The pane tree for `state.screen`, at `width` columns (P506 §3.2). Below
/// each screen's own narrow-terminal threshold, degrades to the list pane
/// alone — `PaneTree` itself has no floor/cap policy (`tui_panes.rs`'s own
/// doc), so that decision is made here, before the tree is built, rather
/// than re-imported.
fn build_tree_for_screen(state: &State, width: u16) -> PaneTree {
    match state.screen {
        Screen::Sessions => build_sessions_tree(state, width),
        Screen::Traits => build_two_pane_tree(
            PANE_TRAITS_LIST,
            "traits",
            PANE_TRAITS_PREVIEW,
            traits_preview_title(state),
            TRAITS_LEFT_MIN,
            TRAITS_RIGHT_MIN,
            width,
        ),
        Screen::Merges => build_two_pane_tree(
            PANE_MERGES_LIST,
            "merges",
            PANE_MERGES_PREVIEW,
            "preview".to_string(),
            MERGES_LEFT_MIN,
            MERGES_RIGHT_MIN,
            width,
        ),
        Screen::Trust => build_two_pane_tree(
            PANE_TRUST_LIST,
            "trust",
            PANE_TRUST_PREVIEW,
            "preview".to_string(),
            TRUST_LEFT_MIN,
            TRUST_RIGHT_MIN,
            width,
        ),
        Screen::Tasks => build_two_pane_tree(
            PANE_TASKS_LIST,
            &tasks_list_title(state),
            PANE_TASKS_PREVIEW,
            tasks_preview_title(state),
            TASKS_LEFT_MIN,
            TASKS_RIGHT_MIN,
            width,
        ),
    }
}

/// TASKS' list pane title (0063; freshness automated by 0063.7): the age of
/// the currently-rendered snapshot, honest about staleness rather than
/// implying a live view — with a failure suffix when the most recent
/// refresh attempt (tick or `s`) did not land.
fn tasks_list_title(state: &State) -> String {
    let base = match &state.tasks_board {
        Some(board) => format!("tasks (as of {} ago)", format_epoch_ago(board.captured_at)),
        None => "tasks (no board read yet)".to_string(),
    };
    match &state.tasks_refresh_error {
        Some(_) => format!("{base} — refresh failed"),
        None => base,
    }
}

fn tasks_preview_title(state: &State) -> String {
    state
        .task_preview
        .as_ref()
        .map(|preview| format!("preview: {}", preview.key))
        .unwrap_or_else(|| "preview".to_string())
}

/// P552 review `live-run-pane-contract-absent`: the ordinary (list-visible)
/// SESSIONS preview's progress/journey source content, WITHOUT resolving any
/// geometry — the one place both [`render_sessions_preview_body`] and
/// [`sessions_focus_leaf_ids`] (which needs the same content only to size the
/// leaves it reports to focus reconciliation, before any drawing happens)
/// read it from, so the two never drift. Any activity-sidecar degradation
/// reason is painted into the always-rendered progress pane, since this
/// preview never draws a title row for a suffix to reach otherwise.
fn sessions_preview_pane_lines(state: &State) -> (Vec<tui::Line>, Vec<run_view::JourneyRow>) {
    let Some(preview) = state.session_preview.as_ref() else {
        return (Vec::new(), Vec::new());
    };
    let mut progress_lines = preview.progress_lines.clone();
    push_degradation_lines(&mut progress_lines, preview);
    (progress_lines, preview.journey_lines.clone())
}

/// P552 review `dashboard-attach-contract-absent`: renders BOTH
/// [`AttachedView::trait_degraded`] and [`AttachedView::activity_degraded`]
/// as their own dim line, composing rather than one replacing the other — a
/// ledger can simultaneously fail trait resolution and have a
/// partially-corrupt (or entirely absent) activity sidecar, and both facts
/// must reach the screen.
fn push_degradation_lines(lines: &mut Vec<tui::Line>, view: &AttachedView) {
    if let Some(reason) = &view.trait_degraded {
        lines.push(labeled_dim_line(&format!("({reason})")));
    }
    if let Some(reason) = &view.activity_degraded {
        lines.push(labeled_dim_line(&format!("({reason})")));
    }
}

/// P552: the ordinary (list-visible) SESSIONS preview's progress/journey
/// pair, drawn through the shared [`run_view::render_pane_body`] renderer —
/// never a second, independently constructed subtree or content renderer.
/// `area` is [`PANE_SESSIONS_PREVIEW_REGION`]'s own resolved rect (never re-derived
/// here), so this call inherits the bounded-progress/rest-to-journey
/// geometry [`run_view::pane_tree`] computes rather than a plain 50/50
/// split.
fn render_sessions_preview_body(frame: &mut ratatui::Frame<'_>, area: Rect, state: &mut State) {
    let (progress_lines, journey_lines) = sessions_preview_pane_lines(state);
    let data = run_view::PaneData {
        progress: Some(&progress_lines),
        journey: Some(&journey_lines),
        history: None,
        current: None,
        post_run: None,
        title: run_view::PaneTitleRow::None,
    };
    // Merged into `state.last_pane_layout` (never replacing it — the list
    // pane's own rect, set by the generic per-leaf loop just above, must
    // survive) so directional focus movement and scroll never read stale
    // rects for these two ids.
    let tree = run_view::pane_tree(&SESSIONS_PREVIEW_PANE_IDS, area, &data);
    let layout = tree.resolve(area);
    if let Some(rect) = layout.rect(PANE_SESSIONS_PROGRESS) {
        state.last_pane_layout.set(PANE_SESSIONS_PROGRESS, rect);
    }
    if let Some(rect) = layout.rect(PANE_SESSIONS_JOURNEY) {
        state.last_pane_layout.set(PANE_SESSIONS_JOURNEY, rect);
    }
    // P552 review `live-run-pane-contract-absent`: paging keys queued while
    // the list is visible (see `queue_sessions_pane_key`) target the journey
    // pane directly — `state.focus` stays on the sessions list itself here
    // (never reconciled into this tree), so `render_pane_body` cannot derive
    // a scroll target from `focus.current()` the way the attached body does.
    run_view::render_pane_body(
        frame,
        area,
        &SESSIONS_PREVIEW_PANE_IDS,
        &data,
        &[],
        None,
        run_view::PaneRenderState {
            scrolls: &mut state.pane_scrolls,
            follow: run_view::PaneFollow {
                progress: &mut state.session_progress_follow,
                journey: &mut state.session_journey_follow,
                history: &mut state.session_history_follow,
                current: &mut state.session_current_follow,
            },
            focus: &mut state.focus,
            pending_keys: &mut state.pending_keys,
            key_target: Some(PANE_SESSIONS_JOURNEY),
        },
    );
}

/// P552 review `live-run-pane-contract-absent`: the REAL focusable leaf ids
/// backing the ordinary (list-visible) SESSIONS preview — `PANE_SESSIONS_LIST`
/// plus whatever [`run_view::pane_tree`] itself would resolve for the
/// preview region (mirroring [`render_sessions_preview_body`]'s own call),
/// rather than the outer tree's placeholder id. Used only for focus
/// reconciliation, before any drawing happens.
fn sessions_focus_leaf_ids(state: &State, resolved: &PaneLayoutResult) -> Vec<PaneId> {
    let mut ids = vec![PANE_SESSIONS_LIST];
    if let Some(region) = resolved.rect(PANE_SESSIONS_PREVIEW_REGION) {
        let (progress_lines, journey_lines) = sessions_preview_pane_lines(state);
        let data = run_view::PaneData {
            progress: Some(&progress_lines),
            journey: Some(&journey_lines),
            history: None,
            current: None,
            post_run: None,
            title: run_view::PaneTitleRow::None,
        };
        ids.extend(run_view::pane_tree(&SESSIONS_PREVIEW_PANE_IDS, region, &data).leaf_ids());
    }
    ids
}

fn build_sessions_tree(state: &State, width: u16) -> PaneTree {
    // A permanent, operator-useful regression signal — invisible in
    // a healthy steady state (below `RELOAD_WARN_THRESHOLD`), present the
    // moment a reload genuinely slows down.
    let list_title = match state.reload_duration {
        Some(duration) if duration >= RELOAD_WARN_THRESHOLD => {
            format!("sessions (reload {}ms)", duration.as_millis())
        }
        _ => "sessions".to_string(),
    };
    let list_leaf = PaneTree::Leaf {
        id: PANE_SESSIONS_LIST,
        title: list_title,
    };
    if width < SESSIONS_LEFT_MIN + SESSIONS_RIGHT_MIN {
        return list_leaf;
    }
    // P552 review `live-run-pane-contract-absent`: this tree only ever backs
    // the ordinary (list-visible) preview — an attached session bypasses it
    // entirely for `render_attached_session_body`'s full four-pane body.
    // The whole right-hand region is ONE placeholder leaf
    // (`PANE_SESSIONS_PREVIEW_REGION`) — `render_sessions_preview_body`
    // resolves its own real `PANE_SESSIONS_PROGRESS`/`PANE_SESSIONS_JOURNEY`
    // leaves from that leaf's rect via `run_view::pane_tree`, the only code
    // that creates and sizes them, rather than this function pre-splitting
    // them 50/50 and having that geometry immediately discarded.
    PaneTree::Split {
        dir: Direction::Horizontal,
        children: vec![
            (Constraint::Min(SESSIONS_LEFT_MIN), list_leaf),
            (
                Constraint::Percentage(50),
                PaneTree::Leaf {
                    id: PANE_SESSIONS_PREVIEW_REGION,
                    title: "preview".to_string(),
                },
            ),
        ],
    }
}

/// The list+single-preview shape TRAITS/MERGES/TRUST each build (P506 §3.2)
/// — one genuine extraction, since all three screens are otherwise identical
/// two-pane trees differing only in ids/titles/width floors.
fn build_two_pane_tree(
    list_id: PaneId,
    list_title: &str,
    preview_id: PaneId,
    preview_title: String,
    left_min: u16,
    right_min: u16,
    width: u16,
) -> PaneTree {
    let list_leaf = PaneTree::Leaf {
        id: list_id,
        title: list_title.to_string(),
    };
    if width < left_min + right_min {
        return list_leaf;
    }
    PaneTree::Split {
        dir: Direction::Horizontal,
        children: vec![
            (Constraint::Min(left_min), list_leaf),
            (
                Constraint::Percentage(50),
                PaneTree::Leaf {
                    id: preview_id,
                    title: preview_title,
                },
            ),
        ],
    }
}

fn traits_preview_title(state: &State) -> String {
    state
        .trait_preview
        .as_ref()
        .map(|preview| format!("preview: {}", preview.trait_id))
        .unwrap_or_else(|| "preview".to_string())
}

/// Dispatches a single leaf's content render by pane id — the one place a
/// new pane's content gets wired up, mirroring `tui_demo::draw`'s own
/// id-dispatch shape.
fn render_pane_content(frame: &mut ratatui::Frame<'_>, id: PaneId, inner: Rect, state: &State) {
    if id == PANE_SESSIONS_LIST {
        render_sessions_list_pane(frame, inner, state);
    } else if id == PANE_TRAITS_LIST {
        render_traits_list_pane(frame, inner, state);
    } else if id == PANE_TRAITS_PREVIEW {
        render_lines_preview_pane(
            frame,
            inner,
            state.trait_preview.as_ref().map(|p| p.lines.as_slice()),
            "(no trait selected)",
            PANE_TRAITS_PREVIEW,
            state,
        );
    } else if id == PANE_MERGES_LIST {
        render_merges_list_pane(frame, inner, state);
    } else if id == PANE_MERGES_PREVIEW {
        render_lines_preview_pane(
            frame,
            inner,
            state.merge_preview.as_ref().map(|p| p.lines.as_slice()),
            "(no run selected)",
            PANE_MERGES_PREVIEW,
            state,
        );
    } else if id == PANE_TRUST_LIST {
        render_trust_list_pane(frame, inner, state);
    } else if id == PANE_TRUST_PREVIEW {
        render_lines_preview_pane(
            frame,
            inner,
            state.trust_preview.as_ref().map(|p| p.lines.as_slice()),
            "(no trait selected)",
            PANE_TRUST_PREVIEW,
            state,
        );
    } else if id == PANE_TASKS_LIST {
        render_tasks_list_pane(frame, inner, state);
    } else if id == PANE_TASKS_PREVIEW {
        render_lines_preview_pane(
            frame,
            inner,
            state.task_preview.as_ref().map(|p| p.lines.as_slice()),
            "(no task selected)",
            PANE_TASKS_PREVIEW,
            state,
        );
    }
}

/// The generic `ViewportScroll`-backed preview-pane renderer shared by
/// TRAITS/MERGES/TRUST (P506 §3.2): `None` content renders `empty_message`
/// instead — never an empty pane with no explanation.
fn render_lines_preview_pane(
    frame: &mut ratatui::Frame<'_>,
    inner: Rect,
    lines: Option<&[RLine<'static>]>,
    empty_message: &'static str,
    pane_id: PaneId,
    state: &State,
) {
    let scroll = state.pane_scrolls.get(pane_id);
    match lines {
        Some(lines) => {
            tui_panes::render_lines_pane(frame, inner, lines, scroll);
        }
        None => {
            tui_panes::render_lines_pane(frame, inner, &[RLine::from(empty_message)], scroll);
        }
    }
}

fn render_sessions_list_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &State) {
    // In `v` ALL mode `state.sessions` spans every indexed repository rather
    // than each row's own store alone, so this is a strict superset of the
    // per-repo id set short-id uniqueness is actually judged against
    // elsewhere (P506 §3.4) — it can only ever lengthen a short id, never
    // make one ambiguous, so computing it globally here is safe as-is.
    let all_ids: Vec<String> = state
        .sessions
        .iter()
        .map(|row| row.session_id.clone())
        .collect();
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.sessions_visible,
        &state.list_sessions,
        |row| session_visible_row_label(row, &state.sessions, &all_ids),
        |_| false,
    );
}

/// The list's content budget in an 80-column terminal: two border columns and
/// the kit's two-column selection marker leave 76 display columns.
const LIST_LABEL_WIDTH: usize = 76;
const SESSION_CLOCK_WIDTH: usize = 8;

fn short_session(session_id: &str, all_ids: &[String]) -> String {
    ctx_traits_io::run_session::short_session_display(session_id, all_ids)
}

fn state_short_session(state: &State, session_id: &str) -> String {
    let all_ids = state
        .sessions
        .iter()
        .map(|row| row.session_id.clone())
        .chain(state.merges.iter().map(|row| row.session_id.clone()))
        .collect::<Vec<_>>();
    short_session(session_id, &all_ids)
}

fn list_field(text: &str, width: usize) -> String {
    let text = tui::truncate_display_width_end(text, width);
    let padding = width.saturating_sub(tui::display_width(&text));
    format!("{text}{}", " ".repeat(padding))
}

fn session_row_label(row: &SessionRow, all_ids: &[String]) -> String {
    let short_id = short_session(&row.session_id, all_ids);
    let id_width = tui::display_width(&short_id);
    // Session identity is never clipped: steal cells from descriptive columns.
    let remaining = LIST_LABEL_WIDTH.saturating_sub(id_width + 5);
    let mut phase_width = remaining.saturating_sub(32).min(19);
    let repo_width = remaining.saturating_sub(19 + phase_width).min(12);
    let state_width = remaining
        .saturating_sub(repo_width + phase_width + 10)
        .min(9);
    let elapsed_width = remaining
        .saturating_sub(repo_width + state_width + phase_width + 3)
        .min(SESSION_CLOCK_WIDTH);
    let base_tokens_width =
        remaining.saturating_sub(repo_width + state_width + phase_width + elapsed_width);
    // A complete compact usage triplet is `W:1k N:1k G:1k` (14 cells).
    // Keep its labels visible on ordinary rows by borrowing descriptive phase
    // cells before truncating the accounting column.
    let token_target: usize = if row.tokens_text.contains("G:") {
        14
    } else {
        0
    };
    let borrowed = token_target
        .saturating_sub(base_tokens_width)
        .min(phase_width);
    phase_width = phase_width.saturating_sub(borrowed);
    let tokens_width = base_tokens_width.saturating_add(borrowed);
    // State and detail share the existing state/phase budget. This keeps the
    // additive separator visible without widening the list.
    //
    // P552's persisted session title is the DETAIL half of that column:
    // `session_state_label` already prefers `row.title` over the phase text,
    // so a resolved title shows without a column of its own, and the short
    // session id stays separate for identity/disambiguation regardless.
    // `phase_width` here is post-borrow, so the token triplet's borrowing
    // still narrows this column rather than overflowing the row.
    let detail_width = state_width + 1 + phase_width;
    let state_and_detail = session_state_label(row);
    let label = format!(
        "{} {} {} {} {}",
        list_field(row.repo_key.as_deref().unwrap_or(""), repo_width),
        short_id,
        list_field(&state_and_detail, detail_width),
        list_field(&row.elapsed_text, elapsed_width),
        list_field(&row.tokens_text, tokens_width),
    );
    debug_assert!(tui::display_width(&label) <= LIST_LABEL_WIDTH);
    label
}

/// Dashboard-local state/detail presentation. `phase_text` repeats the
/// persisted status, while this list already renders the current display
/// state; remove only that exact known prefix before adding useful detail.
fn session_state_label(row: &SessionRow) -> String {
    let phase = normalized_session_phase(row);
    let detail = row.title.as_deref().unwrap_or(phase);
    if detail.is_empty() || detail == row.state_text {
        row.state_text.clone()
    } else {
        format!("{} · {detail}", row.state_text)
    }
}

fn normalized_session_phase(row: &SessionRow) -> &str {
    let Some(status) = row.status.as_ref() else {
        return &row.phase;
    };
    let prefix = run_view::session_status(status);
    if row.phase == prefix {
        ""
    } else {
        row.phase
            .strip_prefix(&format!("{prefix} · "))
            .or_else(|| row.phase.strip_prefix(&format!("{prefix}; ")))
            .unwrap_or(&row.phase)
    }
}

/// One SESSIONS visible row's list label (P506 §3.1/§3.4): a group header
/// (`▾`/`▸` plus its owner-facing label and count) or a real session row,
/// with the session id shortened per [`short_session_display`] against the
/// whole store's id set — `all_ids` is computed once by the caller, not
/// per-row.
fn session_visible_row_label(
    row: &VisibleRow,
    sessions: &[SessionRow],
    all_ids: &[String],
) -> String {
    match row {
        VisibleRow::GroupHeader {
            group,
            count,
            collapsed,
        } => {
            let marker = if *collapsed { "\u{25b8}" } else { "\u{25be}" };
            format!("{marker} {} ({count})", group.label())
        }
        VisibleRow::Session(idx) => {
            let Some(row) = sessions.get(*idx) else {
                return String::new();
            };
            session_row_label(row, all_ids)
        }
    }
}

fn sessions_journey_lines(state: &State) -> Vec<RLine<'static>> {
    match &state.session_preview {
        Some(preview) if !preview.journey_lines.is_empty() => (0..preview.journey_lines.len())
            .map(|_| RLine::default())
            .collect(),
        Some(_) => vec![RLine::from(Span::styled(
            "(no journey recorded for this session)",
            Style::default().add_modifier(Modifier::DIM),
        ))],
        None => Vec::new(),
    }
}

fn render_traits_list_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &State) {
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.traits,
        &state.list_traits,
        trait_row_label,
        |_| false,
    );
}

fn trait_row_label(row: &TraitRow) -> String {
    let label = format!(
        "{} {} {} {}",
        list_field(&row.id, 24),
        list_field(&row.version, 10),
        list_field(&row.status, 18),
        list_field(&row.trust, 21),
    );
    debug_assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);
    label
}

fn render_merges_list_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &State) {
    let all_ids: Vec<String> = state
        .sessions
        .iter()
        .map(|row| row.session_id.clone())
        .collect();
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.merges,
        &state.list_merges,
        |row| merge_row_label(row, &all_ids),
        |_| false,
    );
}

/// The MERGES list row: `short session-id · class · stage · headline` (the
/// translated short label — never the raw reason, per §3.3), the short id
/// applied per §3.4.
fn merge_row_label(row: &MergeRow, all_ids: &[String]) -> String {
    // A landed row's stage is always `cleanup` (the frame the `Merged`
    // status lands on) — truthful but redundant next to the `landed` class
    // column, so it's blanked here rather than shown.
    let stage = if row.class == MergeClass::Landed {
        "-".to_string()
    } else {
        row.stage
            .map(merge_story::stage_text)
            .unwrap_or("-")
            .to_string()
    };
    let short_id = short_session(&row.session_id, all_ids);
    let remaining = LIST_LABEL_WIDTH.saturating_sub(tui::display_width(&short_id) + 3);
    let class_width = remaining.min(10);
    let stage_width = remaining.saturating_sub(class_width).min(12);
    let headline_width = remaining.saturating_sub(class_width + stage_width);
    let label = format!(
        "{} {} {} {}",
        short_id,
        list_field(row.class.label(), class_width),
        list_field(&stage, stage_width),
        list_field(&row.headline, headline_width),
    );
    debug_assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);
    label
}

fn render_trust_list_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &State) {
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.trust_visible,
        &state.list_trust,
        |index| trust_row_label(&state.trust[*index]),
        |index| {
            let row = &state.trust[*index];
            row.trait_id
                .as_deref()
                .is_some_and(|id| state.trust_marks.contains(&id.to_string()))
        },
    );
}

/// The TRUST list row: `trait-id · state · family/variant · short digest`
/// (§4.8). An orphan leads with its recorded digest; the orphan-only list
/// context already supplies its class, so it is not printed redundantly.
fn trust_row_label(row: &TrustRow) -> String {
    if row.class == trust_story::TrustClass::Orphaned {
        let label = format!(
            "{} {} {} {}",
            list_field(
                &short_digest(row.recorded_digest.as_deref().unwrap_or("")),
                20
            ),
            list_field("", 14),
            list_field("", 20),
            list_field("", 19),
        );
        debug_assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);
        return label;
    }
    let family_variant = match (&row.family, &row.variant) {
        (Some(family), Some(variant)) => format!("{family}/{variant}"),
        (Some(family), None) => family.clone(),
        (None, _) => String::new(),
    };
    let digest = short_digest(
        row.recorded_digest
            .as_deref()
            .unwrap_or(&row.current_digest),
    );
    let label = format!(
        "{} {} {} {}",
        list_field(row.trait_id.as_deref().unwrap_or("(orphaned)"), 20),
        list_field(row.class.label(), 14),
        list_field(&family_variant, 20),
        list_field(&digest, 19),
    );
    debug_assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);
    label
}

fn render_tasks_list_pane(frame: &mut ratatui::Frame<'_>, inner: Rect, state: &State) {
    let summaries: &[TaskSummary] = state
        .tasks_board
        .as_ref()
        .map(|board| board.summaries.as_slice())
        .unwrap_or_default();
    tui_panes::render_list_pane(
        frame,
        inner,
        &state.tasks_visible,
        &state.list_tasks,
        |row| task_visible_row_label(row, summaries, &state.task_proposals),
        |_| false,
    );
}

fn task_visible_row_label(
    row: &TaskVisibleRow,
    summaries: &[TaskSummary],
    proposals: &HashMap<String, super::task_proposals::DoneProposal>,
) -> String {
    match row {
        TaskVisibleRow::GroupHeader {
            group,
            count,
            collapsed,
        } => {
            let marker = if *collapsed { "\u{25b8}" } else { "\u{25be}" };
            format!("{marker} {} ({count})", group.label())
        }
        TaskVisibleRow::Task(key) => {
            let Some(summary) = summaries.iter().find(|s| &s.key == key) else {
                return String::new();
            };
            task_row_label(summary, proposals.contains_key(key))
        }
    }
}

/// The TASKS list row: `key · derived status · pending-proposal marker ·
/// title`, padded to [`LIST_LABEL_WIDTH`] like every other list in this
/// dashboard (0063.8: the marker column is fixed-width, so the title field
/// is trimmed to compensate rather than growing the row).
fn task_row_label(summary: &TaskSummary, has_proposal: bool) -> String {
    let label = format!(
        "{} {} {} {}",
        list_field(&summary.key, 8),
        list_field(super::tasks::status_text(summary.derived_status), 10),
        list_field(if has_proposal { "!" } else { "" }, 1),
        list_field(&summary.title, 54),
    );
    debug_assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);
    label
}

fn footer_line(state: &State) -> Paragraph<'static> {
    if state.story_view.is_some() {
        return tui_kit::keymap_footer(
            "↑↓/jk scroll  PgUp/PgDn page  q/Esc/S close",
            state.message.as_deref(),
        );
    }
    let scope_suffix = if state.all_repos { " [ALL]" } else { "" };
    let navigation = "↑↓/jk list  PgUp/PgDn preview  Enter focus  Esc list";
    let hint = match state.screen {
        Screen::Sessions => {
            format!(
                "{navigation}  Enter attach/expand  space expand  a answer  s resume  S story  x stop  d delete  n spawn  v {}  r reload  Tab/1-5 screens  q quit{scope_suffix}",
                if state.all_repos {
                    "this repo only"
                } else {
                    "all repos"
                }
            )
        }
        Screen::Traits => {
            format!(
                "{navigation}  a approve  b block  e edit source  x explain  r reload  Tab/1-5 screens  q quit"
            )
        }
        Screen::Merges => format!(
            "{navigation}  m merge  d deep-merge  p print path  x drop  v {}  r reload  Tab/1-5 screens  q quit{scope_suffix}",
            if state.all_repos {
                "this repo only"
            } else {
                "all repos"
            }
        ),
        Screen::Trust => format!(
            "{navigation}  a approve  b block  space mark  A approve family/marked  o {} {} orphaned  r reload  Tab/1-5 screens  q quit  [{} marked]",
            if state.show_trust_orphans {
                "hide"
            } else {
                "show"
            },
            state
                .trust
                .iter()
                .filter(|row| row.class == trust_story::TrustClass::Orphaned)
                .count(),
            state.trust_marks.len(),
        ),
        Screen::Tasks => format!(
            "{navigation}  space expand  s sync  S split  R reconcile  a archive  e edit  y mark done  d dispatch  r reload  Tab/1-5 screens  q quit"
        ),
    };
    let hint = format!("{hint}  [{}]", state.current_list().position_text());
    let generated_task = state
        .trait_explanation
        .as_ref()
        .and_then(|(_, _, result, started)| {
            result
                .as_ref()
                .err()
                .map(|message| explanation_task_text(message, started.elapsed()))
        });
    let task = if state.loading && !state.has_snapshot {
        Some("loading...".to_string())
    } else if let Some(error) = state.refresh_error.as_deref() {
        Some(format!("stale: {error}"))
    } else if let Some(task) = generated_task.as_deref() {
        Some(task.to_string())
    } else {
        state.message.clone()
    };
    tui_kit::keymap_footer(hint, task.as_deref())
}

fn explanation_task_text(message: &str, elapsed: Duration) -> String {
    if message == "working..." {
        format!("working... {}", tui::elapsed_text(elapsed))
    } else {
        format!("generated explanation unavailable: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // P081: guide-chat routing while attached moved into
    // `run_view::RunPanel`'s own observer (`install_guide_handle`/the
    // ask-refusal branch in `poll_and_apply_keys`) — see run_view.rs's own
    // tests for that coverage. This dashboard no longer routes ask keys or
    // renders a guide chat pane itself.

    // 0143: `session_driver_live` is the ONE probe title-generation gating
    // (`run_view::title_row_line`) and `session_is_terminal` both rely on —
    // a ledger path with no driver-lock sibling at all (never driven, or a
    // driver that has since exited and released the flock) must read as not
    // live, never fail-open into "generation still possible".
    #[test]
    fn session_driver_live_is_false_with_no_lock_held() {
        let directory = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir")
            .join(format!(
                "ctx-dashboard-driver-live-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(directory.as_std_path());
        std::fs::create_dir_all(directory.as_std_path()).expect("create scratch directory");
        let ledger_path = directory.join("ledger.json");
        assert!(!session_driver_live(&ledger_path));
    }

    fn row(class: SessionClass) -> SessionRow {
        row_with_id("s1", class)
    }

    fn row_with_id(id: &str, class: SessionClass) -> SessionRow {
        use ctx_traits_core::procedure::session::Status;
        let status = match class {
            SessionClass::Live | SessionClass::Resumable => Some(Status::AwaitingAgentOutput),
            SessionClass::Terminal => Some(Status::Completed),
            SessionClass::Unreadable => None,
        };
        SessionRow {
            session_id: id.to_string(),
            ledger_path: camino::Utf8PathBuf::from(format!("/tmp/{id}.json")),
            run_id: format!("r-{id}"),
            state_text: "x".to_string(),
            phase: "x".to_string(),
            elapsed_text: "-".to_string(),
            tokens_text: "-".to_string(),
            repo_key: None,
            repo_path: None,
            class,
            status,
            outcome: None,
            title: None,
            task_key: None,
            merged_landed: None,
        }
    }

    fn snapshot_with_sessions(sessions: Vec<SessionRow>) -> DashboardSnapshot {
        let mut state = State::new_without_worker();
        state.sessions = sessions;
        DashboardSnapshot::from_state(&state)
    }

    #[test]
    fn dashboard_guide_tokens_standard_session_row_keeps_all_labels() {
        let mut session = row_with_id("session-1234", SessionClass::Terminal);
        session.tokens_text = "W:1k N:1k G:1k".to_string();
        let label = session_row_label(&session, std::slice::from_ref(&session.session_id));
        assert!(label.contains("W:1k"));
        assert!(label.contains("N:1k"));
        assert!(label.contains("G:1k"));

        session.tokens_text = "W:1m N:1m G:1m".to_string();
        let label = session_row_label(&session, std::slice::from_ref(&session.session_id));
        assert!(label.contains("W:1m"));
        assert!(label.contains("N:1m"));
        assert!(label.contains("G:1m"));
    }

    #[test]
    fn explanation_activity_timer_uses_the_shared_clock() {
        assert_eq!(
            explanation_task_text("working...", Duration::from_secs(59)),
            "working... 00:00:59"
        );
        assert_eq!(
            explanation_task_text("working...", Duration::from_secs(60)),
            "working... 00:01:00"
        );
    }

    #[test]
    fn preview_trust_prefers_current_digest_over_later_identity_history() {
        let document = ctx_traits_io::trust::Document {
            digests: vec![
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:a".to_string(),
                    state: ctx_traits_io::trust::TrustState::Blocked,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: Some("A is blocked".to_string()),
                    seq: Some(1),
                },
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:b".to_string(),
                    state: ctx_traits_io::trust::TrustState::Verified,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: None,
                    seq: Some(2),
                },
            ],
        };
        assert_eq!(
            trust_record_facts(&document, "fixture", "sha256:a"),
            (
                "blocked".to_string(),
                "A is blocked".to_string(),
                false,
                true
            )
        );
    }

    #[test]
    fn trust_facts_keep_exact_and_raw_current_evidence_ahead_of_history() {
        let mut document = ctx_traits_io::trust::Document {
            digests: vec![
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:a".to_string(),
                    state: ctx_traits_io::trust::TrustState::Verified,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: None,
                    seq: Some(1),
                },
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:b".to_string(),
                    state: ctx_traits_io::trust::TrustState::Verified,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: None,
                    seq: Some(2),
                },
            ],
        };
        assert_eq!(
            trust_record_facts(&document, "fixture", "sha256:a").0,
            "verified"
        );

        document.digests.push(ctx_traits_io::trust::TrustRecord {
            digest: "sha256:a".to_string(),
            state: ctx_traits_io::trust::TrustState::Blocked,
            trait_id: Some("fixture".to_string()),
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(3),
        });
        assert_eq!(
            trust_record_facts(&document, "fixture", "sha256:a").0,
            "blocked"
        );

        document.digests.push(ctx_traits_io::trust::TrustRecord {
            digest: "sha256:a".to_string(),
            state: ctx_traits_io::trust::TrustState::Verified,
            trait_id: None,
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(4),
        });
        assert_eq!(
            trust_record_facts(&document, "fixture", "sha256:a").0,
            "verified"
        );
        let unseen = trust_record_facts(&document, "fixture", "sha256:c");
        assert_eq!(unseen.0, "blocked");
        assert!(
            unseen.2,
            "unseen current bytes must be marked stale history"
        );
    }

    #[test]
    fn trust_rows_keep_exact_and_raw_current_evidence_ahead_of_history() {
        let all = [dashboard_trait_row("fixture", "sha256:a", None, None)];
        let mut document = ctx_traits_io::trust::Document {
            digests: vec![
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:a".to_string(),
                    state: ctx_traits_io::trust::TrustState::Verified,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: None,
                    seq: Some(1),
                },
                ctx_traits_io::trust::TrustRecord {
                    digest: "sha256:b".to_string(),
                    state: ctx_traits_io::trust::TrustState::Verified,
                    trait_id: Some("fixture".to_string()),
                    act: None,
                    updated_at: None,
                    reason: None,
                    seq: Some(2),
                },
            ],
        };
        let class = |document: &ctx_traits_io::trust::Document| {
            build_trust_rows_from(document, &all)[0].class
        };
        assert_eq!(class(&document), trust_story::TrustClass::Verified);

        document.digests.push(ctx_traits_io::trust::TrustRecord {
            digest: "sha256:a".to_string(),
            state: ctx_traits_io::trust::TrustState::Blocked,
            trait_id: Some("fixture".to_string()),
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(3),
        });
        assert_eq!(class(&document), trust_story::TrustClass::Blocked);

        document.digests.push(ctx_traits_io::trust::TrustRecord {
            digest: "sha256:a".to_string(),
            state: ctx_traits_io::trust::TrustState::Verified,
            trait_id: None,
            act: None,
            updated_at: None,
            reason: None,
            seq: Some(4),
        });
        assert_eq!(class(&document), trust_story::TrustClass::Verified);

        let unseen = [dashboard_trait_row("fixture", "sha256:c", None, None)];
        assert_eq!(
            build_trust_rows_from(&document, &unseen)[0].class,
            trust_story::TrustClass::MovedBlock
        );
    }

    #[test]
    fn footer_shows_loading_only_before_the_first_snapshot() {
        let mut state = State::new_without_worker();
        state.loading = true;
        assert!(format!("{:?}", footer_line(&state)).contains("loading..."));

        state.has_snapshot = true;
        assert!(!format!("{:?}", footer_line(&state)).contains("loading..."));
    }

    #[test]
    fn failed_refresh_preserves_snapshot_and_marks_it_stale_until_recovery() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("retained", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        let snapshot = DashboardSnapshot::from_state(&state);
        state.apply_snapshot(&snapshot);
        state.loading = true;

        state.loading = false;
        state.refresh_error = Some("inventory unavailable".to_string());
        assert_eq!(state.sessions[0].session_id, "retained");
        assert!(format!("{:?}", footer_line(&state)).contains("stale: inventory unavailable"));

        state.apply_snapshot(&snapshot);
        assert!(state.refresh_error.is_none());
        assert!(!format!("{:?}", footer_line(&state)).contains("stale:"));
    }

    #[test]
    fn queued_success_before_failure_retains_success_and_marks_stale() {
        let mut state = State::new_without_worker();
        state.loading = true;

        let mut refreshed = State::new_without_worker();
        refreshed.sessions = vec![row_with_id("newest", SessionClass::Live)];
        rebuild_visible_sessions(&mut refreshed);
        let snapshot = DashboardSnapshot::from_state(&refreshed);

        state.apply_refresh_results([
            Ok(std::sync::Arc::new(snapshot)),
            Err("inventory unavailable".to_string()),
        ]);

        assert_eq!(state.sessions[0].session_id, "newest");
        assert!(state.has_snapshot);
        assert!(!state.loading);
        assert!(format!("{:?}", footer_line(&state)).contains("stale: inventory unavailable"));
    }

    #[test]
    fn queued_obsolete_success_does_not_clamp_selection_before_latest_snapshot() {
        let mut state = State::new_without_worker();
        state.sessions = vec![
            row_with_id("initial-a", SessionClass::Live),
            row_with_id("initial-b", SessionClass::Live),
            row_with_id("initial-c", SessionClass::Live),
            row_with_id("initial-d", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(3);

        let mut obsolete = State::new_without_worker();
        obsolete.sessions = vec![row_with_id("obsolete", SessionClass::Live)];
        rebuild_visible_sessions(&mut obsolete);

        let mut latest = State::new_without_worker();
        latest.sessions = vec![
            row_with_id("latest-a", SessionClass::Live),
            row_with_id("latest-b", SessionClass::Live),
            row_with_id("latest-c", SessionClass::Live),
            row_with_id("latest-d", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut latest);

        state.apply_refresh_results([
            Ok(std::sync::Arc::new(DashboardSnapshot::from_state(
                &obsolete,
            ))),
            Ok(std::sync::Arc::new(DashboardSnapshot::from_state(&latest))),
        ]);

        assert!(matches!(
            state.sessions_visible.get(state.list_sessions.selected()),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Live,
                ..
            })
        ));
        assert_eq!(state.sessions[0].session_id, "latest-a");
    }

    #[test]
    fn snapshot_restores_selected_session_after_same_group_reorder() {
        let mut state = State::new_without_worker();
        state.sessions = vec![
            row_with_id("A", SessionClass::Live),
            row_with_id("B", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(2);

        state.apply_snapshot(&snapshot_with_sessions(vec![
            row_with_id("B", SessionClass::Live),
            row_with_id("A", SessionClass::Live),
        ]));

        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("B")
        );
    }

    #[test]
    fn snapshot_restores_selected_header_after_preceding_count_changes() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("live", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        let pending_header = state
            .sessions_visible
            .iter()
            .position(|row| {
                matches!(
                    row,
                    VisibleRow::GroupHeader {
                        group: SessionGroup::Pending,
                        ..
                    }
                )
            })
            .expect("pending group header");
        state.list_sessions.set_selected(pending_header);
        assert!(matches!(
            state.sessions_visible.get(state.list_sessions.selected()),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Pending,
                ..
            })
        ));

        state.apply_snapshot(&snapshot_with_sessions(vec![
            row_with_id("live-a", SessionClass::Live),
            row_with_id("live-b", SessionClass::Live),
        ]));

        assert!(matches!(
            state.sessions_visible.get(state.list_sessions.selected()),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Pending,
                ..
            })
        ));
    }

    #[test]
    fn snapshot_moving_selection_into_collapsed_group_selects_its_header() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("target", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(1);

        state.apply_snapshot(&snapshot_with_sessions(vec![
            row_with_id("other", SessionClass::Live),
            row_with_id("target", SessionClass::Terminal),
        ]));

        assert!(state.collapsed_groups.contains(&SessionGroup::Completed));
        assert!(selected_session(&state).is_none());
        assert!(matches!(
            state.sessions_visible.get(state.list_sessions.selected()),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Completed,
                ..
            })
        ));
    }

    #[test]
    fn snapshot_with_missing_selected_session_uses_a_header() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("gone", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(1);

        state.apply_snapshot(&snapshot_with_sessions(vec![row_with_id(
            "other",
            SessionClass::Live,
        )]));

        assert!(selected_session(&state).is_none());
        assert!(matches!(
            state.sessions_visible.get(state.list_sessions.selected()),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Live,
                ..
            })
        ));
    }

    fn attached_view_for(id: &str) -> AttachedView {
        AttachedView {
            session_id: id.to_string(),
            ledger_path: camino::Utf8PathBuf::from(format!("/tmp/{id}.json")),
            run_id: format!("r-{id}"),
            state_digest: String::new(),
            progress_lines: vec![labeled_dim_line("stub")],
            journey_lines: vec![run_view::journey_line(labeled_dim_line("stub"))],
            post_run: Vec::new(),
            history: Vec::new(),
            current: Vec::new(),
            title: None,
            title_state: None,
            trait_name: Some("stub-trait".to_string()),
            started_at_epoch: Some(0),
            trait_degraded: None,
            activity_degraded: None,
            activity_available: true,
            history_available: false,
            terminal: false,
        }
    }

    #[test]
    fn unreadable_view_clears_digest_for_same_digest_recovery() {
        let mut view = attached_view_for("s1");
        view.state_digest = "unchanged-after-recovery".to_string();
        view.post_run = vec![labeled_dim_line("post-run row")];

        mark_view_unreadable(&mut view, "temporary read failure".to_string());

        assert!(view.state_digest.is_empty());
        assert!(view.journey_lines.is_empty());
        assert!(view.history.is_empty());
        assert!(view.current.is_empty());
        assert!(view.post_run.is_empty());
    }

    #[test]
    fn delayed_worker_preview_is_ignored_after_selection_moves() {
        let mut state = State::new_without_worker();
        state.sessions = vec![
            row_with_id("A", SessionClass::Live),
            row_with_id("B", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(2);

        assert!(!session_preview_matches_current(&state, "A"));
        assert!(session_preview_matches_current(&state, "B"));
    }

    #[test]
    fn initial_live_session_identity_selects_its_row() {
        let mut state = State::new_without_worker_for_session(Some("target".to_string()));
        state.sessions = vec![
            row_with_id("neighbor", SessionClass::Live),
            row_with_id("target", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);

        resolve_initial_session(&mut state);

        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("target")
        );
        assert_eq!(state.initial_session_id.as_deref(), Some("target"));
    }

    #[test]
    fn initial_terminal_session_identity_expands_and_selects_its_row() {
        let mut state = State::new_without_worker_for_session(Some("target".to_string()));
        state.sessions = vec![
            row_with_id("neighbor", SessionClass::Terminal),
            row_with_id("target", SessionClass::Terminal),
        ];
        rebuild_visible_sessions(&mut state);
        assert!(state.collapsed_groups.contains(&SessionGroup::Completed));

        resolve_initial_session(&mut state);

        assert!(!state.collapsed_groups.contains(&SessionGroup::Completed));
        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("target")
        );
        assert!(state.initial_session_id.is_none());
    }

    #[test]
    fn initial_session_selection_survives_live_to_terminal_snapshot() {
        let mut state = State::new_without_worker_for_session(Some("target".to_string()));
        state.sessions = vec![
            row_with_id("live-neighbor", SessionClass::Live),
            row_with_id("target", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        resolve_initial_session(&mut state);
        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("target")
        );
        assert_eq!(state.initial_session_id.as_deref(), Some("target"));

        // A later snapshot can classify the same identity as terminal and add
        // neighboring rows. Its completed group starts collapsed.
        state.sessions = vec![
            row_with_id("live-neighbor", SessionClass::Live),
            row_with_id("terminal-neighbor", SessionClass::Terminal),
            row_with_id("target", SessionClass::Terminal),
        ];
        rebuild_visible_sessions(&mut state);
        assert!(state.collapsed_groups.contains(&SessionGroup::Completed));
        resolve_initial_session(&mut state);

        assert!(!state.collapsed_groups.contains(&SessionGroup::Completed));
        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("target")
        );
        assert!(state.initial_session_id.is_none());
    }

    #[test]
    fn session_navigation_releases_initial_selection_pin() {
        let mut state = State::new_without_worker_for_session(Some("target".to_string()));
        state.sessions = vec![
            row_with_id("target", SessionClass::Live),
            row_with_id("chosen", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        resolve_initial_session(&mut state);
        state.move_selection(1);

        assert!(state.initial_session_id.is_none());
        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("chosen")
        );
    }

    #[test]
    fn selected_live_session_requests_preview_on_reload() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("selected", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(1);

        let request = state.session_preview_request().expect("selected live row");

        assert_eq!(request.session_id, "selected");
        assert_eq!(request.run_id, "r-selected");
    }

    // P081: `focus_enter_and_leave_keep_the_attached_identity_in_sync` and
    // `attached_up_down_queues_for_the_shared_renderer_instead_of_the_hidden_list`
    // tested `update_session_attachment_for_focus`/attached-mode pane-key
    // queueing, both removed — attach is now a synchronous handoff (see
    // `attach_selected`/`run_attached_observer`), never a persisted focus
    // state this dashboard's own key routing tracks.

    /// P552 review `live-run-pane-contract-absent`, `done-when`: the
    /// ordinary (list-visible) SESSIONS preview's own PageDown must reach
    /// the journey pane through the shared renderer's `pending_keys`/
    /// `key_target` path (not the deleted progress-hardcoded direct scroll),
    /// while the list itself stays visible and its selection untouched.
    #[test]
    fn list_visible_preview_page_down_scrolls_journey_not_the_list() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut state = State::new_without_worker();
        state.screen = Screen::Sessions;
        state.sessions = vec![
            row_with_id("A", SessionClass::Live),
            row_with_id("B", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(0);
        let mut preview = attached_view_for("A");
        preview.progress_lines = vec![labeled_dim_line("progress-1")];
        preview.journey_lines = (0..12)
            .map(|n| run_view::journey_line(labeled_dim_line(&format!("journey-{n}"))))
            .collect();
        state.session_preview = Some(preview);

        let area = Rect::new(0, 0, 120, 12);
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("test terminal");
        terminal
            .draw(|frame| render_sessions_preview_body(frame, area, &mut state))
            .expect("draw");
        assert_eq!(
            state.pane_scrolls.get(PANE_SESSIONS_JOURNEY).window(4),
            0..4
        );

        let key = crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
        assert!(queue_sessions_pane_key(&mut state, &key));
        assert_eq!(state.pending_keys, vec![key]);

        terminal
            .draw(|frame| render_sessions_preview_body(frame, area, &mut state))
            .expect("draw");

        assert!(state.pending_keys.is_empty());
        assert!(
            state
                .pane_scrolls
                .get(PANE_SESSIONS_JOURNEY)
                .window(4)
                .start
                > 0
        );
        assert_eq!(state.list_sessions.selected(), 0);
    }

    /// P552 review `live-run-pane-contract-absent`, `done-when`: `Tab` must
    /// still switch dashboard screens while the SESSIONS list is visible —
    /// `queue_sessions_pane_key` deliberately leaves `Tab`/`BackTab` unqueued
    /// in that state so screen-switching survives the shared-renderer input
    /// routing added for PageUp/PageDown.
    #[test]
    fn tab_still_switches_dashboard_screens_while_sessions_list_is_visible() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Sessions;
        state.sessions = vec![row_with_id("A", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);

        let key = crossterm::event::KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(!queue_sessions_pane_key(&mut state, &key));

        let screens = Screen::all();
        let idx = screens.iter().position(|s| *s == state.screen).unwrap_or(0);
        state.switch_screen(screens[(idx + 1) % screens.len()]);
        assert_ne!(state.screen, Screen::Sessions);
    }

    fn scratch_ledger_path(name: &str) -> camino::Utf8PathBuf {
        let dir = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("temp dir is UTF-8")
            .join(format!(
                "ctx-dashboard-activity-test-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
        let _ = std::fs::remove_dir_all(dir.as_std_path());
        std::fs::create_dir_all(dir.as_std_path()).expect("create scratch dir");
        dir.join("session-fixture.json")
    }

    /// Minimal-but-valid [`ctx_traits_core::procedure::session::Session`]
    /// whose `provenance.trait_source` is `None` — the deterministic,
    /// filesystem-free failure `ctx_traits_io::run::load_trait_for_session`
    /// takes when neither `--file`/`--trait` nor a persisted trait source is
    /// available, without needing an actually-unresolvable-on-disk trait id.
    fn unresolvable_trait_session_fixture(
        run_id: &str,
        started_at_epoch: Option<u64>,
    ) -> ctx_traits_core::procedure::session::Session {
        use ctx_traits_core::digest::Digest;
        use ctx_traits_core::procedure::runtime::FinalState;
        ctx_traits_core::procedure::session::Session {
            schema_version: "1".to_string(),
            session_id: ctx_traits_core::procedure::session::SessionId::new(format!(
                "session-{run_id}"
            ))
            .expect("session id"),
            run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string()).expect("run id"),
            trait_id: "test-trait".to_string(),
            source_digest: None,
            canonical_digest: None,
            current_run_index: 0,
            current_source_index: None,
            current_sequence_item_id: None,
            current_sequence_title: None,
            current_agent: None,
            status: ctx_traits_core::procedure::session::Status::AwaitingAgentOutput,
            warnings: Vec::new(),
            accepted_port_values: Vec::new(),
            accepted_slot_values: Vec::new(),
            accepted_output_port_values: Vec::new(),
            slot_revisions: Vec::new(),
            emitted_signals: Vec::new(),
            rejected_submissions: Vec::new(),
            unresolved_inputs: Vec::new(),
            resource_evidence: Vec::new(),
            provider_capability_reports: Vec::new(),
            output_ports: Vec::new(),
            active_path: Vec::new(),
            control_stack: Vec::new(),
            stop_reason: None,
            final_output_summary: Vec::new(),
            next_frame: None,
            last_validation_report: None,
            completion: None,
            last_drive_outcome: None,
            provenance: ctx_traits_core::procedure::session::Provenance {
                started_by: ctx_traits_core::procedure::session::CallerProvenance {
                    surface: "test".to_string(),
                    caller: "dashboard-activity-test".to_string(),
                    agent: None,
                    harness: None,
                },
                state_source: "test".to_string(),
                agent_assignments: None,
                harness_probes: Vec::new(),
                warnings: Vec::new(),
                trait_source: None,
                query_selection: None,
                worktree: None,
                merge_frames: Vec::new(),
                merge_intent: None,
                out_of_tree_mutations: Vec::new(),
                started_at_epoch,
                trust_approval: None,
                session_title: None,
                task_digest: None,
                task_key: None,
                dependency_override: None,
            },
            ledger: ctx_traits_core::procedure::runtime::State {
                run_id: ctx_traits_core::procedure::run::Id::new(run_id.to_string())
                    .expect("run id"),
                trait_id: "test-trait".to_string(),
                strict_loops: false,
                source_digest: None,
                canonical_digest: None,
                current_run_index: 0,
                sequence_statuses: Vec::new(),
                accepted_port_values: Vec::new(),
                accepted_slot_values: Vec::new(),
                accepted_output_port_values: Vec::new(),
                slot_revisions: Vec::new(),
                resource_evidence: Vec::new(),
                emitted_signals: Vec::new(),
                rejected_attempts: Vec::new(),
                provider_capability_reports: Vec::new(),
                output_ports: Vec::new(),
                active_path: Vec::new(),
                control_stack: Vec::new(),
                branch_decisions: Vec::new(),
                conditional_input_decisions: Vec::new(),
                ask_decisions: Vec::new(),
                failure_routes: Vec::new(),
                guard_evaluations: Vec::new(),
                parallel_panel_records: Vec::new(),
                stop_reason: None,
                elapsed_seconds: 0,
                final_state: FinalState::Running,
            },
            state_digest: Digest::source("test"),
        }
    }

    fn append_activity_event(ledger_path: &camino::Utf8Path, frame_id: &str, text: &str) {
        ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(ledger_path).append_activity(
            ctx_traits_core::procedure::activity::ActivityEvent {
                sequence: 0,
                frame_id: frame_id.to_string(),
                kind: ctx_traits_core::procedure::activity::ActivityKind::RunningTool,
                text: Some(text.to_string()),
                tool: None,
                tokens: None,
                rate_limit: None,
            },
        );
    }

    fn append_step_summary_event(ledger_path: &camino::Utf8Path, key: &str, text: &str) {
        ctx_traits_io::activity_sidecar::ActivitySidecarWriter::open(ledger_path)
            .append_step_summary(key.to_string(), "worker".to_string(), text.to_string());
    }

    fn append_corrupt_sidecar_line(ledger_path: &camino::Utf8Path) {
        use std::io::Write;
        let path = ctx_traits_io::activity_sidecar::activity_path(ledger_path);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path.as_std_path())
            .expect("open sidecar for corrupt append");
        writeln!(file, "{{not valid json").expect("append corrupt line");
    }

    /// P552 review `dashboard-attach-contract-absent`, blocker
    /// `dashboard-attach-contract-absent`: an unchanged digest must select
    /// the sidecar-only refresh purely by digest equality — never by
    /// `journey_lines` emptiness, which a failed trait reconstruction leaves
    /// permanently empty and would otherwise force a full re-reconstruction
    /// attempt on every single reload despite nothing on the ledger having
    /// changed. Asynchronously landing narrator step summaries and activity
    /// events (which never touch the ledger's own `state_digest`) must still
    /// reach `history`/`current`, and a partially-corrupt sidecar's
    /// degradation must remain visible alongside (not instead of) the
    /// trait-resolution failure already recorded.
    #[test]
    fn unresolvable_trait_unchanged_digest_refresh_updates_history_and_current_from_sidecar() {
        let ledger_path = scratch_ledger_path("unresolvable-unchanged-digest");
        let session = unresolvable_trait_session_fixture("run-unresolvable-unchanged", Some(0));
        ctx_traits_io::run_session::write_run_session(&ledger_path, &session)
            .expect("write session");

        let mut view = build_attached_view(
            "unresolvable-unchanged-digest",
            &ledger_path,
            "run-unresolvable-unchanged",
        );
        assert!(
            view.journey_lines.is_empty(),
            "trait resolution failed, so journey stays empty"
        );
        assert!(view.trait_degraded.is_some());
        assert!(view.history.is_empty());
        assert!(view.current.is_empty());
        let unchanged_digest = view.state_digest.clone();

        append_activity_event(&ledger_path, "frame-a", "now active");
        append_step_summary_event(&ledger_path, "step-a", "finished the thing");
        append_corrupt_sidecar_line(&ledger_path);

        refresh_attached_view(&mut view);

        // Digest is unchanged, so no trait reconstruction ran — the
        // trait-failure notice from the original build is still there and
        // `journey`/`progress` were never touched by this refresh.
        assert_eq!(view.state_digest, unchanged_digest);
        assert!(view.journey_lines.is_empty());
        assert!(view.trait_degraded.is_some());

        // But history/current both updated from the sidecar, and its own
        // (distinct) degradation reason is visible alongside the
        // trait-resolution failure rather than replacing it.
        assert_eq!(view.history.len(), 1);
        assert!(view.history[0].tail().contains("finished the thing"));
        assert_eq!(view.current.len(), 1);
        assert!(view.current[0].tail().contains("now active"));
        assert!(view.activity_available);
        let activity_reason = view
            .activity_degraded
            .as_deref()
            .expect("corrupt line degrades the sidecar");
        assert!(activity_reason.contains("unparseable"));
        assert_ne!(view.trait_degraded, view.activity_degraded);
    }

    #[test]
    fn unchanged_digest_refresh_observes_post_run_frames_without_trait_reconstruction() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus, Status};

        let ledger_path = scratch_ledger_path("post-run-unchanged-digest");
        let mut session = unresolvable_trait_session_fixture("run-post-run-refresh", Some(0));
        session.status = Status::Completed;
        ctx_traits_io::run_session::write_run_session(&ledger_path, &session)
            .expect("write initial session");
        let mut view = build_attached_view(
            "post-run-unchanged-digest",
            &ledger_path,
            "run-post-run-refresh",
        );
        assert!(view.post_run.is_empty());
        let digest = view.state_digest.clone();

        session.provenance.merge_frames.push(MergeFrame {
            stage: MergeStage::Gates,
            status: MergeStatus::Parked,
            reason: Some("post-run gate failed".to_string()),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        });
        ctx_traits_io::run_session::write_run_session(&ledger_path, &session)
            .expect("append merge frame");
        refresh_attached_view(&mut view);

        assert_eq!(
            view.state_digest, digest,
            "merge frames do not affect digest"
        );
        assert_eq!(view.post_run.len(), 1);
        let text: String = view.post_run[0].segments().map(|(text, _)| text).collect();
        assert!(text.starts_with("× gates"));
        assert!(
            view.trait_degraded.is_some(),
            "refresh did not reconstruct trait"
        );
    }

    #[test]
    fn trait_reconstruction_failure_keeps_persisted_post_run_frames() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus, Status};

        let mut session = unresolvable_trait_session_fixture("run-post-run-fallback", Some(0));
        session.status = Status::Completed;
        session.provenance.merge_frames.push(MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Merged,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        });
        let ledger_path = scratch_ledger_path("post-run-fallback");
        let reconstruction = reconstruct_panes(&session, &ledger_path);
        assert!(reconstruction.trait_degraded.is_some());
        assert_eq!(reconstruction.post_run.len(), 1);
    }

    /// P552 review `terminal-attach-story-identity-lost`: two ledgers can
    /// legitimately share the SAME run-id (a foreign-repository attachment
    /// plus an unrelated same-run-id ledger in the current repository's own
    /// session store) — [`build_story_view_from_ledger`] must resolve the
    /// exact attached ledger's own session and activity sidecar regardless,
    /// never fall back to a `run_id`-only lookup that could silently pick
    /// the OTHER one up.
    #[test]
    fn build_story_view_from_ledger_resolves_the_exact_ledger_not_a_same_run_id_collision() {
        let shared_run_id = "run-shared-id";
        let attached_ledger = scratch_ledger_path("terminal-attach-foreign");
        let colliding_ledger = scratch_ledger_path("terminal-attach-colliding");

        let mut attached_session = unresolvable_trait_session_fixture(shared_run_id, Some(0));
        attached_session.session_id =
            ctx_traits_core::procedure::session::SessionId::new("session-attached".to_string())
                .expect("session id");
        ctx_traits_io::run_session::write_run_session(&attached_ledger, &attached_session)
            .expect("write attached session");
        append_activity_event(
            &attached_ledger,
            "frame-attached",
            "attached ledger's own event",
        );

        let mut colliding_session = unresolvable_trait_session_fixture(shared_run_id, Some(0));
        colliding_session.session_id =
            ctx_traits_core::procedure::session::SessionId::new("session-colliding".to_string())
                .expect("session id");
        ctx_traits_io::run_session::write_run_session(&colliding_ledger, &colliding_session)
            .expect("write colliding session");
        append_activity_event(
            &colliding_ledger,
            "frame-colliding",
            "the OTHER ledger's event",
        );

        let view = build_story_view_from_ledger(&attached_ledger).expect("story from ledger");

        assert_eq!(view.session.session_id.as_str(), "session-attached");
        assert!(view.title.contains(shared_run_id));
        assert_eq!(view.report.detailed_timeline.len(), 1);
        assert_eq!(
            view.report.detailed_timeline[0].event.text.as_deref(),
            Some("attached ledger's own event")
        );
    }

    /// P552 review `dashboard-attach-contract-absent`, `done-when`: a
    /// trait-reconstruction failure must not suppress an existing activity
    /// sidecar — `activity_available` stays `true` and `current` still
    /// renders from it, while `progress` retains the trait-failure notice
    /// (`degraded` — the honest reason `reconstruct_panes` never claims the
    /// sidecar itself is unavailable just because trait loading failed).
    #[test]
    fn trait_reconstruction_failure_still_surfaces_an_existing_sidecars_current_pane() {
        let ledger_path = scratch_ledger_path("unresolvable-trait");
        append_activity_event(&ledger_path, "frame-a", "running the thing");
        let session = unresolvable_trait_session_fixture("run-unresolvable", Some(0));

        let reconstruction = reconstruct_panes(&session, &ledger_path);

        assert!(reconstruction.activity_available);
        assert_eq!(reconstruction.current.len(), 1);
        assert!(
            reconstruction.current[0]
                .tail()
                .contains("running the thing")
        );
        assert!(reconstruction.history.is_empty());
        reconstruction.trait_degraded.expect("degraded reason");
        let progress_text: String = reconstruction.progress[0]
            .segments()
            .map(|(text, _)| text)
            .collect();
        assert!(progress_text.contains("live view unavailable"));
    }

    /// P552 review `dashboard-attach-contract-absent`, `done-when`: the
    /// unchanged-digest refresh path must observe a sidecar that appears
    /// AFTER the initial (successful) reconstruction without re-resolving
    /// the trait/plan — `journey_lines`/`progress_lines` stay exactly what
    /// they were seeded to, proving no reconstruction ran, while `current`
    /// still picks up the newly written sidecar.
    #[test]
    fn unchanged_digest_refresh_observes_a_newly_appearing_sidecar_without_reconstruction() {
        let ledger_path = scratch_ledger_path("unchanged-digest");
        let session = unresolvable_trait_session_fixture("run-unchanged-digest", Some(0));
        ctx_traits_io::run_session::write_run_session(&ledger_path, &session)
            .expect("write session");

        let mut view = attached_view_for("run-unchanged-digest");
        view.ledger_path = ledger_path.clone();
        view.state_digest = session.state_digest.to_string();
        view.progress_lines = vec![labeled_dim_line("seeded-progress")];
        view.journey_lines = vec![run_view::journey_line(labeled_dim_line("seeded-journey"))];
        view.activity_available = false;

        refresh_attached_view(&mut view);
        assert!(!view.activity_available, "no sidecar exists yet");
        assert_eq!(
            view.progress_lines,
            vec![labeled_dim_line("seeded-progress")]
        );
        assert_eq!(view.journey_lines.len(), 1);

        append_activity_event(&ledger_path, "frame-a", "now active");
        refresh_attached_view(&mut view);

        assert!(view.activity_available);
        assert_eq!(view.current.len(), 1);
        assert!(view.current[0].tail().contains("now active"));
        // Never re-reconstructed: still the seeded stand-ins, not fallback
        // trait-failure text.
        assert_eq!(
            view.progress_lines,
            vec![labeled_dim_line("seeded-progress")]
        );
        assert_eq!(view.journey_lines.len(), 1);
    }

    #[test]
    fn live_trace_follow_keeps_new_tail_visible_until_manual_scroll() {
        let mut scroll = tui_kit::ViewportScroll::new();
        follow_session_preview(3, &mut scroll, false, 8);
        assert_eq!(scroll.window(3), 0..3);

        follow_session_preview(3, &mut scroll, true, 8);
        assert_eq!(scroll.window(3), 5..8);

        scroll.apply(ScrollDelta::Up(1), 3);
        follow_session_preview(3, &mut scroll, false, 10);
        assert_eq!(scroll.window(3), 4..7);
    }

    #[test]
    fn attach_selected_records_a_request_for_the_selected_live_row() {
        let mut state = State::new_without_worker();
        state.sessions = vec![row_with_id("A", SessionClass::Live)];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(1);

        attach_selected(&mut state);

        let request = state.attach_request.expect("attach request recorded");
        assert_eq!(request.session_id, "A");
    }

    #[test]
    fn describe_attach_outcome_finished_and_detach_report_on_the_footer() {
        assert_eq!(
            describe_attach_outcome(
                &Ok(Some("the run finished while attached".to_string())),
                "A"
            ),
            Ok("the run finished while attached".to_string())
        );
        assert_eq!(
            describe_attach_outcome(&Ok(None), "A"),
            Ok("detached from A".to_string())
        );
    }

    #[test]
    fn describe_attach_outcome_error_reports_a_modal_body_not_a_footer_message() {
        let outcome: crate::Result<Option<String>> = Err(crate::Error::Command {
            message: "ledger unreadable".to_string(),
        });
        assert_eq!(
            describe_attach_outcome(&outcome, "A"),
            Err("attach failed: ledger unreadable".to_string())
        );
    }

    // Classification table (draft test 1), now covering the real mapping
    // site rather than only the `can_*` verb table it feeds.
    #[test]
    fn classify_session_maps_live_and_terminal_and_resumable() {
        use ctx_traits_core::procedure::session::Status;
        assert_eq!(
            classify_session(true, &Status::Completed, None),
            SessionClass::Live
        );
        assert_eq!(
            classify_session(true, &Status::Failed, None),
            SessionClass::Live
        );
        assert_eq!(
            classify_session(false, &Status::Completed, None),
            SessionClass::Terminal
        );
        assert_eq!(
            classify_session(false, &Status::Failed, None),
            SessionClass::Terminal
        );
        assert_eq!(
            classify_session(false, &Status::AwaitingInput, None),
            SessionClass::Resumable
        );
        assert_eq!(
            classify_session(false, &Status::AwaitingAgentOutput, None),
            SessionClass::Resumable
        );
    }

    // Blocker 2 (P509): a parked ask cancelled via STOP still carries
    // `status == WaitingOnHuman` (record_interrupted_outcome never rewrites
    // status) — only the outcome distinguishes it from a still-open ask.
    #[test]
    fn classify_session_maps_cancelled_parked_ask_to_terminal() {
        use ctx_traits_core::procedure::session::{DriveOutcomeKind, Status};
        assert_eq!(
            classify_session(
                false,
                &Status::WaitingOnHuman,
                Some(&DriveOutcomeKind::Interrupted)
            ),
            SessionClass::Terminal
        );
        assert_eq!(
            classify_session(false, &Status::WaitingOnHuman, None),
            SessionClass::Resumable
        );
    }

    // Blocker 3 (P509): a relative trait path joins against the row's
    // ALL-mode `repo_path`; an absolute path or the default (no ALL-mode
    // tag) single-repo scope both pass through unchanged so the existing
    // cwd-relative resolution keeps working.
    #[test]
    fn resolve_answer_trait_file_path_joins_relative_against_repo_path() {
        assert_eq!(
            resolve_answer_trait_file_path(".ctx/traits/demo/trait.toml", Some("/repos/foo")),
            Some("/repos/foo/.ctx/traits/demo/trait.toml".to_string())
        );
    }

    #[test]
    fn resolve_answer_trait_file_path_passes_absolute_path_through_unchanged() {
        assert_eq!(
            resolve_answer_trait_file_path("/abs/trait.toml", Some("/repos/foo")),
            None
        );
    }

    #[test]
    fn resolve_answer_trait_file_path_defers_to_default_resolution_without_repo_path() {
        assert_eq!(
            resolve_answer_trait_file_path(".ctx/traits/demo/trait.toml", None),
            None
        );
    }

    #[test]
    fn session_group_maps_all_dashboard_states() {
        use ctx_traits_core::procedure::session::{DriveOutcomeKind, Status};
        let cases = [
            (
                "held",
                SessionClass::Live,
                Some(Status::Failed),
                None,
                SessionGroup::Live,
            ),
            (
                // Resumable == the driver-lock probe said nobody is driving
                // this. A stale `awaiting-agent-output` remains resumable;
                // only a held driver is live.
                "awaiting agent",
                SessionClass::Resumable,
                Some(Status::AwaitingAgentOutput),
                None,
                SessionGroup::Resumable,
            ),
            (
                "awaiting input",
                SessionClass::Resumable,
                Some(Status::AwaitingInput),
                None,
                SessionGroup::Pending,
            ),
            (
                "waiting on human",
                SessionClass::Resumable,
                Some(Status::WaitingOnHuman),
                None,
                SessionGroup::Pending,
            ),
            (
                "rejected",
                SessionClass::Resumable,
                Some(Status::Rejected),
                None,
                SessionGroup::Failed,
            ),
            (
                "permission blocked",
                SessionClass::Resumable,
                Some(Status::BlockedCommandPermissionRequired),
                None,
                SessionGroup::Failed,
            ),
            (
                "agent blocked",
                SessionClass::Resumable,
                Some(Status::BlockedAgentUnassigned),
                None,
                SessionGroup::Failed,
            ),
            (
                "blocked",
                SessionClass::Resumable,
                Some(Status::Blocked),
                None,
                SessionGroup::Failed,
            ),
            (
                "failed",
                SessionClass::Terminal,
                Some(Status::Failed),
                None,
                SessionGroup::Failed,
            ),
            (
                "completed",
                SessionClass::Terminal,
                Some(Status::Completed),
                None,
                SessionGroup::Completed,
            ),
            (
                "unreadable",
                SessionClass::Unreadable,
                None,
                None,
                SessionGroup::Failed,
            ),
            (
                "interrupted",
                SessionClass::Terminal,
                Some(Status::WaitingOnHuman),
                Some(DriveOutcomeKind::Interrupted),
                SessionGroup::Failed,
            ),
            (
                "killed",
                SessionClass::Terminal,
                Some(Status::AwaitingAgentOutput),
                Some(DriveOutcomeKind::Killed),
                SessionGroup::Failed,
            ),
        ];
        for (name, class, status, outcome, expected) in cases {
            assert_eq!(
                session_group(class, status.as_ref(), outcome.as_ref()),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn session_groups_have_fixed_display_order_and_labels() {
        assert_eq!(
            SessionGroup::order().map(SessionGroup::label),
            ["live", "resumable", "pending", "failed", "completed"]
        );
    }

    #[test]
    fn a_stopped_mid_frame_run_is_resumable_unless_a_driver_is_held() {
        use ctx_traits_core::procedure::session::Status;
        assert_eq!(
            session_group(
                SessionClass::Resumable,
                Some(&Status::AwaitingAgentOutput),
                None
            ),
            SessionGroup::Resumable
        );
        // ...and a genuinely driven one still is, whatever its status says.
        assert_eq!(
            session_group(SessionClass::Live, Some(&Status::AwaitingAgentOutput), None),
            SessionGroup::Live
        );
    }

    #[test]
    fn session_group_projection_defaults_and_toggles_survive_reload() {
        use ctx_traits_core::procedure::session::Status;
        let mut state = State::new_without_worker();
        let live = row_with_id("live", SessionClass::Live);
        let mut pending = row_with_id("pending", SessionClass::Resumable);
        pending.status = Some(Status::AwaitingInput);
        let mut failed = row_with_id("failed", SessionClass::Resumable);
        failed.status = Some(Status::Failed);
        let mut completed = row_with_id("completed", SessionClass::Terminal);
        completed.status = Some(Status::Completed);
        // Resumable + `awaiting-agent-output`: a run whose driver died while a
        // frame was out. It belongs in the collapsed resumable section.
        let mut stopped = row_with_id("stopped", SessionClass::Resumable);
        stopped.status = Some(Status::AwaitingAgentOutput);
        state.sessions = vec![
            live.clone(),
            pending.clone(),
            stopped.clone(),
            failed.clone(),
            completed.clone(),
        ];
        rebuild_visible_sessions(&mut state);

        assert!(matches!(
            state.sessions_visible[0],
            VisibleRow::GroupHeader {
                group: SessionGroup::Live,
                count: 1,
                collapsed: false
            }
        ));
        assert!(matches!(state.sessions_visible[1], VisibleRow::Session(0)));
        assert!(matches!(
            state.sessions_visible[2],
            VisibleRow::GroupHeader {
                group: SessionGroup::Resumable,
                count: 1,
                collapsed: true
            }
        ));
        assert!(matches!(
            state.sessions_visible[3],
            VisibleRow::GroupHeader {
                group: SessionGroup::Pending,
                count: 1,
                collapsed: true
            }
        ));
        assert!(matches!(
            state.sessions_visible[4],
            VisibleRow::GroupHeader {
                group: SessionGroup::Failed,
                count: 1,
                collapsed: true
            }
        ));
        assert!(matches!(
            state.sessions_visible[5],
            VisibleRow::GroupHeader {
                group: SessionGroup::Completed,
                count: 1,
                collapsed: true
            }
        ));

        for group in SessionGroup::order() {
            let index = state.sessions_visible.iter().position(|row| matches!(row, VisibleRow::GroupHeader { group: current, .. } if *current == group)).expect("group header");
            state.list_sessions.set_selected(index);
            toggle_selected_group(&mut state);
        }
        assert_eq!(state.collapsed_groups, HashSet::from([SessionGroup::Live]));

        let snapshot = DashboardSnapshot::from_state(&state);
        state.apply_snapshot(&snapshot);
        assert_eq!(state.collapsed_groups, HashSet::from([SessionGroup::Live]));
        assert!(matches!(
            state.sessions_visible[0],
            VisibleRow::GroupHeader {
                group: SessionGroup::Live,
                collapsed: true,
                ..
            }
        ));
        assert!(
            state
                .sessions_visible
                .iter()
                .any(|row| matches!(row, VisibleRow::Session(2)))
        );
        assert!(
            state
                .sessions_visible
                .iter()
                .any(|row| matches!(row, VisibleRow::Session(3)))
        );
    }

    #[test]
    fn group_headers_are_not_session_action_targets() {
        let mut state = State::new_without_worker();
        rebuild_visible_sessions(&mut state);

        assert!(matches!(
            state.sessions_visible.first(),
            Some(VisibleRow::GroupHeader {
                group: SessionGroup::Live,
                count: 0,
                collapsed: false,
            })
        ));
        assert!(selected_session(&state).is_none());

        attach_selected(&mut state);
        open_kill_modal(&mut state);

        assert!(state.attach_request.is_none());
        assert!(!state.modal_host.is_open());
    }

    // MERGES row classification (P472 §3.3): a `merges_from_inventory`-level
    // table over terminal-frame/drive-completed combinations, mirroring
    // `classify_session_maps_live_and_terminal_and_resumable`'s own shape.
    // `PostMergeCleanupFailure`/`RecoveryFailure` must never map to
    // `Parked` — see `session.rs`'s own doc comment on `MergeStatus`.
    #[test]
    fn classify_merge_maps_terminal_frames_and_mergeable() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus};

        let frame = |status: MergeStatus| MergeFrame {
            stage: MergeStage::Gates,
            status,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };

        assert_eq!(
            classify_merge(Some(&frame(MergeStatus::Merged)), false),
            Some(MergeClass::Landed)
        );
        assert_eq!(
            classify_merge(Some(&frame(MergeStatus::Parked)), false),
            Some(MergeClass::Parked)
        );
        assert_eq!(
            classify_merge(Some(&frame(MergeStatus::PostMergeCleanupFailure)), false),
            Some(MergeClass::Failed)
        );
        assert_eq!(
            classify_merge(Some(&frame(MergeStatus::RecoveryFailure)), false),
            Some(MergeClass::Failed)
        );
        assert_eq!(classify_merge(None, true), Some(MergeClass::Mergeable));
        assert_eq!(classify_merge(None, false), None);
    }

    #[test]
    fn merge_class_retry_and_drop_eligibility() {
        assert!(MergeClass::Mergeable.can_retry());
        assert!(MergeClass::Parked.can_retry());
        assert!(MergeClass::Failed.can_retry());
        assert!(!MergeClass::Landed.can_retry());

        assert!(!MergeClass::Mergeable.can_drop());
        assert!(MergeClass::Parked.can_drop());
        assert!(MergeClass::Failed.can_drop());
        assert!(MergeClass::Landed.can_drop());
    }

    // `MergeRow::headline` must honor its own documented contract (empty for
    // `Landed`/`Mergeable`) rather than showing a translated headline the
    // list row has no use for — guards the recurrence of
    // `merged-frames-still-routed-through-the-park-classifier` at the
    // dashboard layer, not just inside `merge_story`.
    #[test]
    fn merge_row_headline_is_blank_for_landed_and_never_reads_unrecognized() {
        use ctx_traits_core::procedure::session::{MergeFrame, MergeStage, MergeStatus};

        let merged_frame = MergeFrame {
            stage: MergeStage::Landing,
            status: MergeStatus::Merged,
            reason: None,
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        assert_eq!(
            merge_row_headline(MergeClass::Landed, Some(&merged_frame)),
            ""
        );
        assert_eq!(merge_row_headline(MergeClass::Mergeable, None), "");

        let parked_frame = MergeFrame {
            stage: MergeStage::Gates,
            status: MergeStatus::Parked,
            reason: Some("post-run gate just-test failed: exit=Some(1)".to_string()),
            evidence: Vec::new(),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        let headline = merge_row_headline(MergeClass::Parked, Some(&parked_frame));
        assert!(!headline.is_empty());
        assert!(!headline.contains("unrecognized"));
    }

    // P081: `attached_session_survives_reorder_and_never_adopts_another_rows_identity`
    // and `attached_session_reports_gone_when_it_leaves_the_inventory` tested
    // `refresh_attached_session`, which is removed — attach no longer
    // persists a pane this reload path has to re-point at (see
    // `AttachRequest`/`run_attached_observer`); the list-visible preview's
    // own identity handling (`apply_snapshot`'s `session_preview` branch) is
    // covered elsewhere.

    // Session classes remain presentation-only; attach/resume display policy
    // must not grow back into stop/delete authorization.
    #[test]
    fn session_class_only_controls_presentation_verbs() {
        assert!(!SessionClass::Live.can_resume());
        assert!(SessionClass::Live.can_attach());

        assert!(SessionClass::Resumable.can_resume());
        assert!(SessionClass::Resumable.can_attach());

        assert!(!SessionClass::Terminal.can_resume());
        assert!(SessionClass::Terminal.can_attach());

        assert!(!SessionClass::Unreadable.can_attach());
        assert!(!SessionClass::Unreadable.can_resume());
    }

    // Test 8 (resume argv): exact argv, no `--worktree`.
    #[test]
    fn resume_argv_is_exact_and_never_carries_worktree() {
        let argv = resume_argv("abc123");
        assert_eq!(
            argv,
            vec![
                "traits",
                "drive",
                "--session",
                "abc123",
                "--progress",
                "none"
            ]
        );
        assert!(!argv.iter().any(|arg| arg == "--worktree"));
    }

    // Test 7 (delete plan enumeration): the pure planner's three cases.
    #[test]
    fn delete_plan_ledger_only_without_worktree_provenance() {
        let plan = plan_delete(
            camino::Utf8Path::new("/runs/s1.json"),
            None,
            None,
            None,
            true,
            None,
        );
        assert_eq!(plan.artifact_lines(), vec!["ledger: /runs/s1.json"]);
    }

    #[test]
    fn delete_plan_includes_worktree_when_registration_verified() {
        let plan = plan_delete(
            camino::Utf8Path::new("/runs/s1.json"),
            None,
            None,
            Some(("wt1", "ctx/run/wt1")),
            true,
            Some((
                camino::Utf8PathBuf::from("/repo"),
                camino::Utf8PathBuf::from("/worktrees/wt1"),
            )),
        );
        assert_eq!(
            plan.artifact_lines(),
            vec![
                "ledger: /runs/s1.json",
                "worktree: /worktrees/wt1",
                "branch: ctx/run/wt1",
            ]
        );
    }

    #[test]
    fn delete_plan_notes_unregistered_worktree_and_leaves_it_out() {
        let plan = plan_delete(
            camino::Utf8Path::new("/runs/s1.json"),
            None,
            None,
            Some(("wt1", "ctx/run/wt1")),
            true,
            None,
        );
        assert!(plan.worktree.is_none());
        let lines = plan.artifact_lines();
        assert_eq!(lines[0], "ledger: /runs/s1.json");
        assert!(lines[1].contains("not registered"));
    }

    #[test]
    fn delete_plan_notes_foreign_repository_without_probing_git() {
        let plan = plan_delete(
            camino::Utf8Path::new("/runs/s1.json"),
            None,
            None,
            Some(("wt1", "ctx/run/wt1")),
            false,
            None,
        );
        assert!(plan.worktree.is_none());
        assert!(plan.worktree_note.unwrap().contains("different repository"));
    }

    #[test]
    fn list_labels_fit_an_80_column_pane_and_clip_each_dynamic_field() {
        let id = format!("session-{}", "a".repeat(64));
        let other = format!("session-{}", "a".repeat(12) + &"b".repeat(52));
        let mut session = row_with_id(&id, SessionClass::Live);
        session.repo_key = Some("repository-key-that-is-far-too-wide".to_string());
        session.state_text = "state-that-is-far-too-wide".to_string();
        session.phase = "phase-that-is-far-too-wide-for-a-session-list-row".to_string();
        session.elapsed_text = "1234567890s".to_string();
        session.tokens_text = "123456 tokens".to_string();
        let session_label = session_visible_row_label(
            &VisibleRow::Session(0),
            std::slice::from_ref(&session),
            &[id.clone(), other.clone()],
        );
        assert_eq!(tui::display_width(&session_label), LIST_LABEL_WIDTH);
        assert!(!session_label.contains(&id));
        assert!(session_label.contains(&short_session(&id, &[id.clone(), other])));

        session.repo_key = Some("界\u{0301}".repeat(20));
        session.phase = "界\u{0301}".repeat(40);
        let wide_label = session_row_label(&session, std::slice::from_ref(&id));
        assert!(tui::display_width(&wide_label) <= LIST_LABEL_WIDTH);

        let trait_row = TraitRow {
            id: "trait-id-that-is-far-too-wide-for-this-column".to_string(),
            version: "version-that-is-too-wide".to_string(),
            status: "status-that-is-too-wide".to_string(),
            trust: "trust-state-that-is-too-wide".to_string(),
            canonical_digest: String::new(),
            source_path: String::new(),
            error: None,
        };
        let trait_label = trait_row_label(&trait_row);
        assert_eq!(tui::display_width(&trait_label), LIST_LABEL_WIDTH);

        let merge = MergeRow {
            session_id: id.clone(),
            run_id: String::new(),
            ledger_path: camino::Utf8PathBuf::new(),
            class: MergeClass::Failed,
            stage: None,
            headline: "headline-that-is-far-too-wide-for-the-merge-list-row".to_string(),
            phase: None,
            trait_id: String::new(),
            last_frame: None,
            worktree: None,
            repo_path: None,
        };
        assert_eq!(
            tui::display_width(&merge_row_label(&merge, std::slice::from_ref(&id))),
            LIST_LABEL_WIDTH
        );

        let trust = TrustRow {
            trait_id: Some("trait-id-that-is-far-too-wide-for-this-column".to_string()),
            origin: String::new(),
            family: Some("family-that-is-far-too-wide".to_string()),
            variant: Some("variant-that-is-far-too-wide".to_string()),
            current_digest: "d".repeat(64),
            recorded_digest: None,
            class: trust_story::TrustClass::MovedApproval,
            updated_at: None,
            reason: None,
        };
        assert_eq!(
            tui::display_width(&trust_row_label(&trust)),
            LIST_LABEL_WIDTH
        );
    }

    #[test]
    fn session_row_label_omits_repeated_state_and_keeps_additive_detail() {
        use ctx_traits_core::procedure::session::Status;
        let mut session = row_with_id("session-1", SessionClass::Resumable);
        session.state_text = "in-progress".to_string();
        session.status = Some(Status::AwaitingAgentOutput);

        session.phase = "in-progress".to_string();
        assert_eq!(session_state_label(&session), "in-progress");

        session.phase = "in-progress · checking output".to_string();
        assert_eq!(
            session_state_label(&session),
            "in-progress · checking output"
        );

        session.phase = "in-progress; trait source warning".to_string();
        assert_eq!(
            session_state_label(&session),
            "in-progress · trait source warning"
        );

        session.phase = "in-progress · ignored phase".to_string();
        session.title = Some("persisted title".to_string());
        assert_eq!(
            session_state_label(&session),
            "in-progress · persisted title"
        );
        assert!(
            tui::display_width(&session_row_label(&session, &[session.session_id.clone()]))
                <= LIST_LABEL_WIDTH
        );
    }

    #[test]
    fn relative_age_keeps_human_unit_prose() {
        assert_eq!(format_elapsed_ago(Duration::from_secs(128)), "2m 8s ago");
    }

    #[test]
    fn session_clock_column_is_fixed_across_duration_boundaries() {
        let durations = ["00:00:59", "00:01:00", "00:59:59", "01:00:00"];
        let labels = durations.map(|elapsed_text| {
            let mut session = row_with_id("session-1", SessionClass::Live);
            session.phase = "building".to_string();
            session.elapsed_text = elapsed_text.to_string();
            session.tokens_text = "12 tok".to_string();
            session_row_label(&session, &["session-1".to_string()])
        });

        let clock_start = labels[0].find(durations[0]).expect("clock duration");
        let tokens_start = labels[0].find("12 tok").expect("tokens");
        assert_eq!(tokens_start - clock_start, SESSION_CLOCK_WIDTH + 1);
        for (label, duration) in labels.iter().zip(durations) {
            assert_eq!(tui::display_width(label), LIST_LABEL_WIDTH);
            assert_eq!(label.find(duration), Some(clock_start));
            assert_eq!(label.find("12 tok"), Some(tokens_start));
        }
    }

    // Test 6 (target identity survives a reload): an action tag addresses a
    // session_id, not an index; a re-lookup after the backing rows changed
    // must find the same session or report it gone — never whatever now
    // occupies the old index.
    #[test]
    fn action_tag_reresolves_by_session_id_not_index() {
        let rows = [row(SessionClass::Live)];
        let found = rows.iter().find(|row| row.session_id == "s1");
        assert!(found.is_some());
        let reloaded: Vec<SessionRow> = Vec::new();
        let found_after_reload = reloaded.iter().find(|row| row.session_id == "s1");
        assert!(found_after_reload.is_none());
    }

    // Test 3/4/5-supporting: the kit's `ModalHost` (exercised directly here
    // for SessionAction, mirroring `tui_kit`'s own coverage) never yields a
    // tag without a resolved outcome, so no action key can mutate state
    // without its modal resolving first — see `tui_kit::tests` for the
    // exhaustive state-machine coverage this type reuses unchanged.
    #[test]
    fn modal_host_gates_every_session_action() {
        let mut host: ModalHost<SessionAction> = ModalHost::new();
        assert!(!host.is_open());
        host.open(
            SessionAction::Kill("s1".to_string()),
            Modal::confirm("stop session", "body"),
        );
        assert!(host.is_open());
        let pending = host.handle_key(&crossterm::event::KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::NONE,
        ));
        assert!(pending.is_none());
        assert!(host.is_open());
        let resolved = host.handle_key(&crossterm::event::KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ));
        assert!(matches!(
            resolved,
            Some((SessionAction::Kill(id), ModalOutcome::Confirmed)) if id == "s1"
        ));
        assert!(!host.is_open());
    }

    // P472: an `m`/`d`/`x` keypress on MERGES opens a modal (the `ModalHost`
    // tag) rather than performing the side effect directly — mirrors
    // `modal_host_gates_every_session_action` for `MergeAction`.
    #[test]
    fn modal_host_gates_every_merge_action() {
        let mut host: ModalHost<MergeAction> = ModalHost::new();
        host.open(
            MergeAction::Retry {
                run_id: "r1".to_string(),
                deep: false,
            },
            Modal::confirm("merge", "body"),
        );
        assert!(host.is_open());
        let resolved = host.handle_key(&crossterm::event::KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        ));
        assert!(matches!(
            resolved,
            Some((MergeAction::Retry { run_id, deep: false }, ModalOutcome::Confirmed))
                if run_id == "r1"
        ));
        assert!(!host.is_open());
    }

    // ------------------------------------------------------------------
    // P471: TRAITS master-detail preview/trust/edit
    // ------------------------------------------------------------------

    fn trait_row(id: &str, digest: &str) -> TraitRow {
        TraitRow {
            id: id.to_string(),
            version: "1.0.0".to_string(),
            status: "active".to_string(),
            trust: "pending".to_string(),
            canonical_digest: digest.to_string(),
            source_path: "/traits/x/index.toml".to_string(),
            error: None,
        }
    }

    fn trust_row(id: &str, digest: &str, class: trust_story::TrustClass) -> TrustRow {
        TrustRow {
            trait_id: Some(id.to_string()),
            origin: "repo".to_string(),
            family: None,
            variant: None,
            current_digest: digest.to_string(),
            recorded_digest: None,
            class,
            updated_at: None,
            reason: None,
        }
    }

    fn orphan_trust_row(recorded_digest: &str) -> TrustRow {
        TrustRow {
            trait_id: None,
            origin: "orphaned".to_string(),
            family: None,
            variant: None,
            current_digest: String::new(),
            recorded_digest: Some(recorded_digest.to_string()),
            class: trust_story::TrustClass::Orphaned,
            updated_at: None,
            reason: None,
        }
    }

    // An orphan-heavy trust store (the exact shape observed live: 207
    // orphaned records ahead of 9 current ones) must never leave the TRUST
    // list opening on a row `a`/`b`/`A` all refuse — actionable rows sort
    // first regardless of how many orphan rows exist.
    #[test]
    fn sort_trust_rows_puts_orphan_rows_after_actionable_rows() {
        let mut rows = vec![
            orphan_trust_row("sha256:orphan-1"),
            orphan_trust_row("sha256:orphan-2"),
            trust_row("t1", "sha256:aaa", trust_story::TrustClass::Unreviewed),
        ];
        sort_trust_rows(&mut rows);
        assert_eq!(rows[0].trait_id.as_deref(), Some("t1"));
        assert!(rows[1].trait_id.is_none());
        assert!(rows[2].trait_id.is_none());
    }

    #[test]
    fn trust_orphan_projection_toggles_the_measured_store_shape() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Trust;
        state.trust.extend((0..8).map(|index| {
            trust_row(
                &format!("live-{index}"),
                &format!("sha256:live-{index}"),
                trust_story::TrustClass::Verified,
            )
        }));
        state
            .trust
            .extend((0..208).map(|index| orphan_trust_row(&format!("sha256:orphan-{index}"))));
        rebuild_visible_trust(&mut state);

        assert_eq!(state.trust_visible.len(), 8);
        assert!(
            state
                .trust_visible
                .iter()
                .all(|index| state.trust[*index].trait_id.is_some())
        );

        toggle_trust_orphans(&mut state);
        assert_eq!(state.trust_visible.len(), 216);

        toggle_trust_orphans(&mut state);
        assert_eq!(state.trust_visible.len(), 8);
    }

    #[test]
    fn selected_trust_uses_visible_indices_and_hidden_orphans_cannot_act() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Trust;
        state.trust = vec![
            trust_row("live", "sha256:live", trust_story::TrustClass::Verified),
            orphan_trust_row("sha256:orphan"),
        ];
        rebuild_visible_trust(&mut state);
        assert_eq!(
            selected_trust(&state).and_then(|row| row.trait_id.as_deref()),
            Some("live")
        );

        // The backing orphan is inaccessible while hidden; selection remains
        // on the live record rather than accidentally addressing index one.
        state.list_trust.move_by(1, usize::MAX);
        assert_eq!(
            selected_trust(&state).and_then(|row| row.trait_id.as_deref()),
            Some("live")
        );
        assert!(state.trust_marks.is_empty());

        toggle_trust_orphans(&mut state);
        state.list_trust.move_by(1, usize::MAX);
        assert!(selected_trust(&state).is_some_and(|row| row.trait_id.is_none()));
        toggle_trust_orphans(&mut state);
        assert_eq!(
            selected_trust(&state).and_then(|row| row.trait_id.as_deref()),
            Some("live")
        );
    }

    #[test]
    fn trust_footer_always_exposes_the_orphan_toggle_and_count() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Trust;
        state.trust = vec![
            trust_row("live", "sha256:live", trust_story::TrustClass::Verified),
            orphan_trust_row("sha256:orphan"),
        ];
        rebuild_visible_trust(&mut state);
        assert!(format!("{:?}", footer_line(&state)).contains("o show 1 orphaned"));
        toggle_trust_orphans(&mut state);
        assert!(format!("{:?}", footer_line(&state)).contains("o hide 1 orphaned"));
    }

    #[test]
    fn orphan_trust_presentation_uses_the_digest_without_repeating_its_class() {
        let mut orphan = orphan_trust_row("sha256:0123456789abcdef");
        orphan.reason = Some("retired trait".to_string());
        let label = trust_row_label(&orphan);
        assert!(label.starts_with(&short_digest("sha256:0123456789abcdef")));
        assert!(!label.contains("orphaned"));
        assert_eq!(tui::display_width(&label), LIST_LABEL_WIDTH);

        let normal_label = trust_row_label(&trust_row(
            "live",
            "sha256:live",
            trust_story::TrustClass::Verified,
        ));
        assert!(normal_label.contains("live"));
        assert!(normal_label.contains("verified"));

        let facts = TrustPreviewFacts {
            trait_id: None,
            origin: "orphaned".to_string(),
            family: None,
            variant: None,
            current_digest: String::new(),
            recorded_digest: orphan.recorded_digest.clone(),
            class: orphan.class,
            updated_at: Some("now".to_string()),
            reason: orphan.reason.clone(),
            sighting: None,
            family_members: Vec::new(),
        };
        let rendered: Vec<String> = trust_preview_lines(&facts).iter().map(text_of).collect();
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("sha256:0123456789abcdef"))
        );
        assert!(rendered.iter().any(|line| line.contains("retired trait")));
        assert!(
            !rendered
                .iter()
                .any(|line| line.contains("origin: orphaned"))
        );

        let normal = TrustPreviewFacts {
            trait_id: Some("live".to_string()),
            origin: "repo".to_string(),
            family: None,
            variant: None,
            current_digest: "sha256:live".to_string(),
            recorded_digest: None,
            class: trust_story::TrustClass::Verified,
            updated_at: None,
            reason: None,
            sighting: None,
            family_members: Vec::new(),
        };
        assert!(
            trust_preview_lines(&normal)
                .iter()
                .map(text_of)
                .any(|line| line.contains("origin: repo"))
        );
    }

    #[test]
    fn orphan_preview_rebuilds_for_each_selected_orphan_record() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Trust;
        let mut first = orphan_trust_row("sha256:first-orphan");
        first.reason = Some("first reason".to_string());
        let mut second = orphan_trust_row("sha256:second-orphan");
        second.reason = Some("second reason".to_string());
        state.trust = vec![first, second];
        state.show_trust_orphans = true;
        rebuild_visible_trust(&mut state);

        refresh_trust_preview_for_selection(&mut state);
        let first_preview = state.trust_preview.as_ref().expect("first orphan preview");
        let first_rendered: String = first_preview
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(first_rendered.contains("sha256:first-orphan"));
        assert!(first_rendered.contains("first reason"));

        state.list_trust.move_by(1, usize::MAX);
        refresh_trust_preview_for_selection(&mut state);
        let second_preview = state.trust_preview.as_ref().expect("second orphan preview");
        let rendered: String = second_preview
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(rendered.contains("sha256:second-orphan"));
        assert!(rendered.contains("second reason"));
        assert!(!rendered.contains("sha256:first-orphan"));
        assert!(!rendered.contains("first reason"));
    }

    #[test]
    fn orphan_preview_rebuilds_when_records_share_a_digest() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Trust;
        let mut first = orphan_trust_row("sha256:shared");
        first.updated_at = Some("first timestamp".to_string());
        first.reason = Some("first reason".to_string());
        let mut second = orphan_trust_row("sha256:shared");
        second.updated_at = Some("second timestamp".to_string());
        second.reason = Some("second reason".to_string());
        state.trust = vec![first, second];
        state.show_trust_orphans = true;
        rebuild_visible_trust(&mut state);

        refresh_trust_preview_for_selection(&mut state);
        state.list_trust.move_by(1, usize::MAX);
        refresh_trust_preview_for_selection(&mut state);
        let rendered: String = state
            .trust_preview
            .as_ref()
            .expect("second orphan preview")
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(rendered.contains("second timestamp"));
        assert!(rendered.contains("second reason"));
        assert!(!rendered.contains("first timestamp"));
        assert!(!rendered.contains("first reason"));
    }

    fn text_of(line: &tui::Line) -> String {
        line.segments().map(|(text, _)| text).collect()
    }

    fn facts_stub() -> TraitPreviewFacts {
        TraitPreviewFacts {
            id: "my-trait".to_string(),
            version: "1.0.0".to_string(),
            status: "active".to_string(),
            canonical_digest: "sha256:abc".to_string(),
            trust_state: "pending".to_string(),
            trust_reason: String::new(),
            trust_stale: false,
            has_trust_record: false,
            drift: "clean".to_string(),
            source_drift_checked: false,
            procedure: ProcedureShape::Sequence(vec![
                ("step-one".to_string(), "prompt".to_string()),
                ("step-two".to_string(), "command".to_string()),
            ]),
            source_path: "/traits/my-trait/index.ts".to_string(),
            source_excerpt: vec!["export const meta = {}".to_string()],
            error: None,
        }
    }

    // Test 1: `trait_preview_lines` is pure and complete — the digest, the
    // trust state, and one procedure-shape row per sequence item are all
    // present.
    #[test]
    fn trait_preview_lines_covers_digest_trust_and_procedure_shape() {
        let facts = facts_stub();
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(rendered.iter().any(|l| l.contains("sha256:abc")));
        assert!(rendered.iter().any(|l| l.contains("pending")));
        assert!(rendered.iter().any(|l| l.contains("step-one")));
        assert!(rendered.iter().any(|l| l.contains("step-two")));
    }

    // Test 1 (guidance-only case): no `[procedure]` produces the explicit
    // no-procedure row, never an empty section.
    #[test]
    fn trait_preview_lines_guidance_only_trait_is_explicit() {
        let mut facts = facts_stub();
        facts.procedure = ProcedureShape::GuidanceOnly;
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("no procedure — guidance-only trait"))
        );
    }

    // Blocker `preview-mislabels-unloadable-trait-as-guidance-only`: a trait
    // that could not be read/checked must never render the positive
    // guidance-only claim — it renders the distinct "unknown" row instead.
    #[test]
    fn trait_preview_lines_unknown_procedure_never_says_guidance_only() {
        let mut facts = facts_stub();
        facts.procedure = ProcedureShape::Unknown;
        facts.error = Some("parse failed".to_string());
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(!rendered.iter().any(|l| l.contains("guidance-only")));
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("procedure unknown — trait could not be read"))
        );
    }

    // Blocker `trait-preview-drift-omits-authored-source`: the drift row
    // must never present an unqualified all-clear when the authored source
    // was not part of the comparison — the qualifier is required and
    // always present in that case, present or not depending on the fact.
    #[test]
    fn trait_preview_lines_flags_unchecked_authored_source() {
        let mut facts = facts_stub();
        facts.source_drift_checked = false;
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("authored source not re-checked"))
        );
    }

    #[test]
    fn trait_preview_lines_omits_qualifier_when_authored_source_was_checked() {
        let mut facts = facts_stub();
        facts.source_drift_checked = true;
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(
            !rendered
                .iter()
                .any(|l| l.contains("authored source not re-checked"))
        );
    }

    // Test 2: stale-trust rendering — a trust record digest that no longer
    // matches the canonical digest produces the re-approval-required row.
    #[test]
    fn trait_preview_lines_flags_stale_trust_record() {
        let mut facts = facts_stub();
        facts.has_trust_record = true;
        facts.trust_stale = true;
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(rendered.iter().any(|l| l.contains("re-approval required")));
    }

    #[test]
    fn trait_preview_lines_current_trust_record_has_no_stale_warning() {
        let mut facts = facts_stub();
        facts.has_trust_record = true;
        facts.trust_stale = false;
        let lines = trait_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(!rendered.iter().any(|l| l.contains("re-approval required")));
    }

    // Test 3: digest-movement refusal — the pure decision function, no IO,
    // no trust store. Now keyed against `state.trust` (§4.7's shared
    // re-lookup source for both screens) rather than `state.traits`.
    #[test]
    fn decide_member_apply_proceeds_when_digest_unchanged() {
        let rows = vec![trust_row(
            "t1",
            "sha256:aaa",
            trust_story::TrustClass::Unreviewed,
        )];
        assert_eq!(
            decide_member_apply(&rows, "t1", "sha256:aaa"),
            TrustApplyDecision::Proceed
        );
    }

    #[test]
    fn decide_member_apply_refuses_when_digest_moved() {
        let rows = vec![trust_row(
            "t1",
            "sha256:bbb",
            trust_story::TrustClass::Unreviewed,
        )];
        assert_eq!(
            decide_member_apply(&rows, "t1", "sha256:aaa"),
            TrustApplyDecision::DigestMoved {
                captured: "sha256:aaa".to_string(),
                current: "sha256:bbb".to_string(),
            }
        );
    }

    #[test]
    fn decide_member_apply_reports_gone_when_row_missing() {
        let rows: Vec<TrustRow> = Vec::new();
        assert_eq!(
            decide_member_apply(&rows, "t1", "sha256:aaa"),
            TrustApplyDecision::RowGone
        );
    }

    // Test 4: an unreadable row (empty canonical digest) refuses before the
    // modal opens.
    #[test]
    fn open_trait_trust_modal_refuses_unreadable_row_before_opening() {
        let mut state = State::new();
        state.screen = Screen::Traits;
        state.traits = vec![trait_row("t1", "")];
        state.list_traits.set_len(state.traits.len());
        open_trait_trust_modal(&mut state, ctx_traits_io::trust::TrustState::Verified);
        assert!(!state.modal_host.is_open());
        assert!(state.message.unwrap().contains("unreadable"));
    }

    #[test]
    fn open_trait_trust_modal_opens_for_readable_row() {
        let mut state = State::new();
        state.screen = Screen::Traits;
        state.traits = vec![trait_row("t1", "sha256:aaa")];
        state.list_traits.set_len(state.traits.len());
        open_trait_trust_modal(&mut state, ctx_traits_io::trust::TrustState::Verified);
        assert!(state.modal_host.is_open());
    }

    // Test 5: cancelling a trust modal never writes.
    #[test]
    fn apply_trait_action_cancelled_never_writes() {
        let mut state = State::new();
        state.traits = vec![trait_row("t1", "sha256:aaa")];
        let action = TraitAction::Trust {
            label: "t1".to_string(),
            members: vec![("t1".to_string(), "sha256:aaa".to_string())],
            verdict: ctx_traits_io::trust::TrustState::Verified,
        };
        let result = apply_trait_action(&mut state, action, ModalOutcome::Cancelled);
        assert!(result.is_ok());
        assert!(state.message.is_none());
    }

    // A whole family write aborts naming the offender when any one member's
    // digest moved since the modal opened — never a partial apply.
    #[test]
    fn decide_member_apply_covers_a_multi_member_set() {
        let rows = vec![
            trust_row("t1", "sha256:aaa", trust_story::TrustClass::Unreviewed),
            trust_row("t2", "sha256:bbb", trust_story::TrustClass::Unreviewed),
        ];
        assert_eq!(
            decide_member_apply(&rows, "t1", "sha256:aaa"),
            TrustApplyDecision::Proceed
        );
        assert_eq!(
            decide_member_apply(&rows, "t2", "sha256:old"),
            TrustApplyDecision::DigestMoved {
                captured: "sha256:old".to_string(),
                current: "sha256:bbb".to_string(),
            }
        );
    }

    // Test 6: identity-addressed position restore — re-locates the edited
    // trait by id after a row-set change moves its index, and reports it
    // gone (never a neighbor) when it left the inventory.
    #[test]
    fn reposition_trait_selection_finds_moved_row_by_id() {
        let traits = vec![trait_row("b", "sha256:b"), trait_row("a", "sha256:a")];
        assert_eq!(reposition_trait_selection(&traits, "a"), Some(1));
    }

    #[test]
    fn reposition_trait_selection_reports_none_when_gone() {
        let traits = vec![trait_row("b", "sha256:b")];
        assert_eq!(reposition_trait_selection(&traits, "a"), None);
    }

    // Test 7: the preview cache gate — an unchanged (trait_id,
    // canonical_digest) pair does not trigger a rebuild; a moved digest
    // does.
    #[test]
    fn trait_preview_needs_rebuild_gate() {
        assert!(!trait_preview_needs_rebuild(
            Some(("t1", "sha256:aaa")),
            "t1",
            "sha256:aaa"
        ));
        assert!(trait_preview_needs_rebuild(
            Some(("t1", "sha256:aaa")),
            "t1",
            "sha256:bbb"
        ));
        assert!(trait_preview_needs_rebuild(
            Some(("t1", "sha256:aaa")),
            "t2",
            "sha256:aaa"
        ));
        assert!(trait_preview_needs_rebuild(None, "t1", "sha256:aaa"));
    }

    // P506 §3.3: `focus_pane` moves the ring to a target leaf (bounded by
    // the ring's own small fixed leaf count) and is a no-op once already
    // there.
    #[test]
    fn focus_pane_moves_ring_to_target_and_is_idempotent() {
        let mut ring = FocusRing::new(vec![PANE_SESSIONS_LIST, PANE_SESSIONS_PROGRESS]);
        assert_eq!(ring.current(), Some(PANE_SESSIONS_LIST));
        focus_pane(&mut ring, PANE_SESSIONS_PROGRESS);
        assert_eq!(ring.current(), Some(PANE_SESSIONS_PROGRESS));
        focus_pane(&mut ring, PANE_SESSIONS_PROGRESS);
        assert_eq!(ring.current(), Some(PANE_SESSIONS_PROGRESS));
        focus_pane(&mut ring, PANE_SESSIONS_LIST);
        assert_eq!(ring.current(), Some(PANE_SESSIONS_LIST));
    }

    #[test]
    fn enter_focuses_preview_and_esc_restores_the_list() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Traits;
        state.focus = FocusRing::new(vec![PANE_TRAITS_LIST, PANE_TRAITS_PREVIEW]);

        assert!(handle_focus_key(
            &mut state,
            &crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        ));
        assert_eq!(state.focus.current(), Some(PANE_TRAITS_PREVIEW));

        assert!(handle_focus_key(
            &mut state,
            &crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        ));
        assert_eq!(state.focus.current(), Some(PANE_TRAITS_LIST));
    }

    // List movement is independent of preview focus and returns visible focus
    // to the list — as long as SESSIONS is not genuinely attached.
    #[test]
    fn list_navigation_works_from_preview_focus_when_not_attached() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Sessions;
        state.sessions = vec![
            row_with_id("A", SessionClass::Live),
            row_with_id("B", SessionClass::Live),
        ];
        rebuild_visible_sessions(&mut state);
        state.list_sessions.set_selected(1);
        state.focus = FocusRing::new(vec![PANE_SESSIONS_LIST, PANE_SESSIONS_PROGRESS]);
        focus_pane(&mut state.focus, PANE_SESSIONS_PROGRESS);

        assert!(handle_navigation_key(
            &mut state,
            &crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        ));

        assert_eq!(state.focus.current(), Some(PANE_SESSIONS_LIST));
        assert_eq!(
            selected_session(&state).map(|row| row.session_id.as_str()),
            Some("B")
        );
    }

    // P081: `list_navigation_never_detaches_a_genuinely_attached_session`
    // tested `queue_sessions_pane_key`'s deleted attached-mode branch — Enter
    // is a synchronous handoff now, so there is no longer a persisted
    // attached focus state for list navigation to disturb.

    // P506 review blocker `focus-ring-includes-undrawn-panes`: below a
    // screen's narrow-terminal floor the tree degrades to the list leaf
    // alone, so a focus previously moved to the preview/progress pane must
    // never survive into that drawn frame — `draw_screen` reconciles the
    // ring against the tree it actually resolves, every time, rather than a
    // hypothetical maximum-width one. Below-floor width for every screen:
    // SESSIONS 60+30=90, TRAITS/MERGES/TRUST 50+40=90; 80 columns (the
    // classic terminal default) is below all four.
    #[test]
    fn focus_reconciles_to_the_drawn_tree_below_every_screens_narrow_floor() {
        const NARROW_WIDTH: u16 = 80;
        for screen in Screen::all() {
            let mut state = State::new();
            state.screen = screen;
            // Simulate focus having moved to the preview/progress pane at a
            // wide layout on a prior frame, before the terminal narrowed.
            state.focus = FocusRing::new(vec![list_pane_id(screen), preview_pane_id(screen)]);
            focus_pane(&mut state.focus, preview_pane_id(screen));
            assert_eq!(state.focus.current(), Some(preview_pane_id(screen)));

            let tree = build_tree_for_screen(&state, NARROW_WIDTH);
            state.focus.reconcile(tree.leaf_ids(), list_pane_id(screen));

            let leaves = tree.leaf_ids();
            assert!(
                state.focus.current().is_some_and(|id| leaves.contains(&id)),
                "{screen:?}: focused pane must be a leaf of the drawn (narrow) tree"
            );
            assert_eq!(
                state.focus.current(),
                Some(list_pane_id(screen)),
                "{screen:?}: narrow layouts restore list focus"
            );
        }
    }

    // `apply_pane_scroll` clamps against the pane's own content length
    // rather than an arbitrary/persisted one.
    #[test]
    fn apply_pane_scroll_clamps_to_pane_content_len() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Traits;
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 3));
        state.trait_preview = Some(TraitPreview {
            trait_id: "t1".to_string(),
            canonical_digest: "sha256:aaa".to_string(),
            lines: vec![RLine::from("a"), RLine::from("b"), RLine::from("c")],
        });
        apply_pane_scroll(&mut state, PANE_TRAITS_PREVIEW, ScrollDelta::Down(100));
        let scroll = state.pane_scrolls.get(PANE_TRAITS_PREVIEW);
        assert_eq!(scroll.window(1), 2..3);
    }

    #[test]
    fn paging_from_list_focus_moves_only_the_preview_and_saturates() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Traits;
        state.focus = FocusRing::new(vec![PANE_TRAITS_LIST, PANE_TRAITS_PREVIEW]);
        state.traits = vec![
            trait_row("one", "sha256:one"),
            trait_row("two", "sha256:two"),
        ];
        state.list_traits.set_len(state.traits.len());
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 10));
        state.trait_preview = Some(TraitPreview {
            trait_id: "one".to_string(),
            canonical_digest: "sha256:one".to_string(),
            lines: (0..30).map(|n| RLine::from(n.to_string())).collect(),
        });

        for _ in 0..10 {
            assert!(handle_navigation_key(
                &mut state,
                &crossterm::event::KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            ));
        }

        assert_eq!(state.list_traits.selected(), 0);
        assert_eq!(state.focus.current(), Some(PANE_TRAITS_LIST));
        assert_eq!(
            state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(8),
            22..30
        );
    }

    #[test]
    fn paging_an_absent_preview_is_a_no_op_and_draw_clamping_tracks_resize() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Traits;
        state.trait_preview = Some(TraitPreview {
            trait_id: "one".to_string(),
            canonical_digest: "sha256:one".to_string(),
            lines: (0..20).map(|n| RLine::from(n.to_string())).collect(),
        });
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 10));
        apply_pane_scroll(&mut state, PANE_TRAITS_PREVIEW, ScrollDelta::Down(100));
        assert_eq!(
            state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(8),
            12..20
        );

        state.last_pane_layout = PaneLayoutResult::default();
        apply_pane_scroll(&mut state, PANE_TRAITS_PREVIEW, ScrollDelta::Up(10));
        assert_eq!(
            state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(8),
            12..20
        );

        state.trait_preview.as_mut().unwrap().lines.truncate(5);
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 12));
        clamp_visible_pane_scroll(&mut state, PANE_TRAITS_PREVIEW);
        assert_eq!(state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(10), 0..5);
    }

    #[test]
    fn screen_switch_preserves_list_and_preview_scroll_state() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Traits;
        state.list_traits.set_len(5);
        state.list_traits.set_selected(3);
        state.trait_preview = Some(TraitPreview {
            trait_id: "one".to_string(),
            canonical_digest: "sha256:one".to_string(),
            lines: (0..20).map(|n| RLine::from(n.to_string())).collect(),
        });
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 10));
        apply_pane_scroll(&mut state, PANE_TRAITS_PREVIEW, ScrollDelta::Down(100));
        let preview_window = state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(8);

        state.switch_screen(Screen::Merges);
        state.switch_screen(Screen::Traits);

        assert_eq!(state.list_traits.selected(), 3);
        assert_eq!(
            state.pane_scrolls.get(PANE_TRAITS_PREVIEW).window(8),
            preview_window
        );
    }

    #[test]
    fn every_footer_starts_with_navigation_and_return_hints() {
        for screen in Screen::all() {
            let mut state = State::new_without_worker();
            state.screen = screen;
            let rendered = format!("{:?}", footer_line(&state));
            assert!(rendered.contains("↑↓/jk list"));
            assert!(rendered.contains("PgUp/PgDn preview"));
            assert!(rendered.contains("Enter focus"));
            assert!(rendered.contains("Esc list"));
        }
    }

    // Unreadable-row facts (row.error is Some): the preview degrades rather
    // than attempting a trait load, surfacing the error inline at the top
    // of the rendered lines rather than crashing or rendering an empty pane.
    #[test]
    fn build_trait_preview_degrades_for_unreadable_row() {
        let mut row = trait_row("broken", "");
        row.error = Some("parse failed".to_string());
        let preview = build_trait_preview(&row, &ctx_traits_io::trust::Document::default());
        assert!(!preview.lines.is_empty());
        let rendered: String = preview.lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(rendered.contains("parse failed"));
    }

    // ------------------------------------------------------------------
    // P473: TRUST trait-centric master-detail
    // ------------------------------------------------------------------

    fn dashboard_trait_row(
        id: &str,
        digest: &str,
        origin: Option<&str>,
        family: Option<&str>,
    ) -> DashboardTraitRow {
        DashboardTraitRow {
            id: id.to_string(),
            version: "1.0.0".to_string(),
            status: "active".to_string(),
            trust: "pending".to_string(),
            canonical_digest: digest.to_string(),
            source_path: format!("/traits/{id}/index.toml"),
            error: None,
            origin: origin.map(str::to_string),
            family: family.map(str::to_string),
            variant: None,
        }
    }

    // `load_traits_and_trust`'s TRAITS-side projection (§4.1): filtering
    // `origin != Some("built-in")` from the shared inventory scan excludes a
    // built-in trait from TRAITS while leaving it in the unfiltered `all`
    // set TRUST's own `build_trust_rows` projects from — exercised as a pure
    // filter here (no trust-store IO); `build_trust_rows` itself reads the
    // real machine-local trust store, so it is exercised indirectly through
    // `classify_records`-level tests instead of directly in this module.
    #[test]
    fn traits_filter_excludes_built_ins_leaving_them_for_trust() {
        let all = [
            dashboard_trait_row("repo-trait", "sha256:aaa", None, None),
            dashboard_trait_row("builtin-trait", "sha256:bbb", Some("built-in"), None),
        ];
        let traits: Vec<&str> = all
            .iter()
            .filter(|row| row.origin.as_deref() != Some("built-in"))
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(traits, vec!["repo-trait"]);
        let trust_ids: Vec<&str> = all.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(trust_ids, vec!["repo-trait", "builtin-trait"]);
    }

    // A recorded decision naming a trait id no longer visible anywhere
    // classifies `Orphaned` (§4.2) — exercised directly against
    // `ctx_traits_io::trust::classify_records`, the same join
    // `build_trust_rows` delegates its orphan bucket to, so this is a
    // structural assertion on the shared classifier rather than a test that
    // touches the real machine-local trust store.
    #[test]
    fn classify_records_marks_a_vanished_trait_id_as_orphaned() {
        let document = ctx_traits_io::trust::Document {
            digests: vec![ctx_traits_io::trust::TrustRecord {
                digest: "sha256:ccc".to_string(),
                state: ctx_traits_io::trust::TrustState::Verified,
                trait_id: Some("gone-trait".to_string()),
                act: None,
                updated_at: None,
                reason: None,
                seq: Some(1),
            }],
        };
        let current = vec![("repo-trait".to_string(), "sha256:aaa".to_string())];
        let rows = ctx_traits_io::trust::classify_records(&document, &current);
        let orphan = rows
            .iter()
            .find(|row| row.trait_id.as_deref() == Some("gone-trait"))
            .expect("expected the gone-trait record");
        assert_eq!(
            orphan.freshness,
            ctx_traits_io::trust::TrustFreshness::Orphaned
        );
        assert_eq!(
            trust_story::classify_trust(Some(orphan)),
            trust_story::TrustClass::Orphaned
        );
    }

    // Orphan rows and rows with no current digest refuse `a`/`b` before any
    // modal opens (§4.6) — the same "refuse honestly" precedent as
    // `open_trait_trust_modal_refuses_unreadable_row_before_opening`.
    #[test]
    fn open_trust_modal_refuses_orphan_row_before_opening() {
        let mut state = State::new();
        state.screen = Screen::Trust;
        state.trust = vec![TrustRow {
            trait_id: None,
            origin: "orphaned".to_string(),
            family: None,
            variant: None,
            current_digest: String::new(),
            recorded_digest: Some("sha256:ccc".to_string()),
            class: trust_story::TrustClass::Orphaned,
            updated_at: None,
            reason: None,
        }];
        rebuild_visible_trust(&mut state);
        open_trust_modal(&mut state, ctx_traits_io::trust::TrustState::Verified);
        assert!(!state.modal_host.is_open());
    }

    // `A` gathers exactly the selected row's family members — never a
    // neighboring family, never a family-less row.
    #[test]
    fn open_trust_family_modal_gathers_exact_family_members() {
        let mut state = State::new();
        state.screen = Screen::Trust;
        state.trust = vec![
            trust_row_with_family("a1", "sha256:a1", "widgets"),
            trust_row_with_family("a2", "sha256:a2", "widgets"),
            trust_row_with_family("b1", "sha256:b1", "gadgets"),
        ];
        rebuild_visible_trust(&mut state);
        open_trust_family_modal(&mut state, ctx_traits_io::trust::TrustState::Verified);
        assert!(state.modal_host.is_open());
        // Resolve the modal (single-line input, Enter submits) to read back
        // the tag `ModalHost` was opened with — the only way to observe it
        // without a second, kit-only accessor.
        let enter = crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let (tag, _) = state
            .modal_host
            .handle_key(&enter)
            .expect("modal resolves on enter");
        let Action::Trait(TraitAction::Trust { members, label, .. }) = tag else {
            panic!("expected a trust action");
        };
        assert_eq!(label, "widgets");
        let ids: Vec<&str> = members.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["a1", "a2"]);
    }

    // A family write aborts the whole set, naming the offender, if one
    // member's digest moved after the modal opened — never a partial apply.
    #[test]
    fn apply_trait_action_aborts_whole_family_set_when_a_member_moved() {
        let mut state = State::new();
        state.trust = vec![
            trust_row_with_family("a1", "sha256:a1", "widgets"),
            trust_row_with_family("a2", "sha256:MOVED", "widgets"),
        ];
        let action = TraitAction::Trust {
            label: "widgets".to_string(),
            members: vec![
                ("a1".to_string(), "sha256:a1".to_string()),
                ("a2".to_string(), "sha256:a2".to_string()),
            ],
            verdict: ctx_traits_io::trust::TrustState::Verified,
        };
        let result = apply_trait_action(
            &mut state,
            action,
            ModalOutcome::Submitted("looks good".to_string()),
        );
        assert!(result.is_ok());
        let message = state.message.unwrap();
        assert!(message.contains("a2"));
        assert!(message.contains("moved"));
    }

    // The cancel path on any trust modal (TRAITS' or TRUST's) writes
    // nothing and says so in these exact words (§1 note 1); every other
    // screen keeps the generic "cancelled".
    #[test]
    fn cancel_message_names_trust_specifically() {
        let trust_tag = Action::Trait(TraitAction::Trust {
            label: "t1".to_string(),
            members: vec![("t1".to_string(), "sha256:aaa".to_string())],
            verdict: ctx_traits_io::trust::TrustState::Verified,
        });
        assert_eq!(cancel_message(&trust_tag), "no trust change recorded");

        let exit_tag = Action::Exit;
        assert_eq!(cancel_message(&exit_tag), "cancelled");
    }

    // `trust_preview_lines` degrades to an explicit "(none)" on a
    // digest-less orphan row rather than panicking or rendering blank, and
    // always includes the fixed approval-meaning block.
    #[test]
    fn trust_preview_lines_degrades_on_digest_less_row_and_covers_approval_meaning() {
        let facts = TrustPreviewFacts {
            trait_id: None,
            origin: "orphaned".to_string(),
            family: None,
            variant: None,
            current_digest: String::new(),
            recorded_digest: Some("sha256:ccc".to_string()),
            class: trust_story::TrustClass::Orphaned,
            updated_at: None,
            reason: None,
            sighting: None,
            family_members: Vec::new(),
        };
        let lines = trust_preview_lines(&facts);
        let rendered: Vec<String> = lines.iter().map(text_of).collect();
        assert!(rendered.iter().any(|l| l.contains("(none)")));
        assert!(rendered.iter().any(|l| l.contains("what approving means")));
        assert!(
            rendered
                .iter()
                .any(|l| l.contains("no run ledger on this machine recorded these bytes"))
        );
    }

    fn trust_row_with_family(id: &str, digest: &str, family: &str) -> TrustRow {
        TrustRow {
            trait_id: Some(id.to_string()),
            origin: "repo".to_string(),
            family: Some(family.to_string()),
            variant: None,
            current_digest: digest.to_string(),
            recorded_digest: None,
            class: trust_story::TrustClass::Unreviewed,
            updated_at: None,
            reason: None,
        }
    }

    // -----------------------------------------------------------------
    // TASKS (0063)
    // -----------------------------------------------------------------

    fn task_summary(key: &str, derived: DerivedStatus) -> TaskSummary {
        TaskSummary {
            key: key.to_string(),
            title: format!("title {key}"),
            stored_status: None,
            derived_status: derived,
            archived: false,
        }
    }

    fn resolved_task(key: &str, derived: DerivedStatus) -> ResolvedTask {
        ResolvedTask {
            digest: format!("sha256:{key}"),
            document: ctx_traits_core::task::TaskDocument {
                schema_version: ctx_traits_core::task::SCHEMA_VERSION.to_string(),
                key: key.to_string(),
                title: format!("title {key}"),
                status: None,
                raised: None,
                closed: None,
                wall: None,
                origin: None,
                content: String::new(),
                scope: String::new(),
                validation: String::new(),
                relations: ctx_traits_core::task::Relations::default(),
                steps: Vec::new(),
                checks: Vec::new(),
                auto_close: None,
                closure: None,
            },
            derived_status: derived,
            relations: ctx_traits_core::task::graph::ResolvedRelations::default(),
            archived: false,
            open_steps: Vec::new(),
        }
    }

    fn board_with(
        summaries: Vec<TaskSummary>,
        resolved: BTreeMap<String, ResolvedTask>,
    ) -> TasksBoardSnapshot {
        TasksBoardSnapshot {
            summaries,
            resolved,
            sync_report: SyncReport::default(),
            captured_at: wall_clock_now_secs(),
            fingerprint: BoardFingerprint::default(),
        }
    }

    #[test]
    fn task_group_live_run_beats_a_blocked_derived_status() {
        let live = row_with_id("s1", SessionClass::Live);
        assert_eq!(
            task_group(DerivedStatus::Blocked, &[&live]),
            TaskGroup::InFlight
        );
    }

    #[test]
    fn task_group_parked_run_beats_a_ready_derived_status() {
        use ctx_traits_core::procedure::session::Status;
        let mut pending = row_with_id("s1", SessionClass::Resumable);
        pending.status = Some(Status::AwaitingInput);
        assert_eq!(
            task_group(DerivedStatus::Ready, &[&pending]),
            TaskGroup::Parked
        );
    }

    #[test]
    fn task_group_done_with_a_live_joined_run_is_still_in_flight() {
        let live = row_with_id("s1", SessionClass::Live);
        assert_eq!(
            task_group(DerivedStatus::Done, &[&live]),
            TaskGroup::InFlight
        );
    }

    #[test]
    fn task_group_falls_back_to_derived_status_with_no_joined_runs() {
        assert_eq!(task_group(DerivedStatus::Blocked, &[]), TaskGroup::Blocked);
        assert_eq!(task_group(DerivedStatus::Ready, &[]), TaskGroup::Ready);
        assert_eq!(task_group(DerivedStatus::Done, &[]), TaskGroup::Done);
        assert_eq!(task_group(DerivedStatus::Cancelled, &[]), TaskGroup::Done);
    }

    #[test]
    fn task_session_join_is_many_to_many_and_a_parent_keyed_run_leaves_the_child_idle() {
        let mut state = State::new_without_worker();
        let mut run_a = row_with_id("run-a", SessionClass::Live);
        run_a.task_key = Some("0010".to_string());
        let mut run_b = row_with_id("run-b", SessionClass::Live);
        run_b.task_key = Some("0010".to_string());
        let mut run_c = row_with_id("run-c", SessionClass::Live);
        // Keyed to the parent while doing the child's work (0062 permits
        // this) — the child must have no joined runs of its own.
        run_c.task_key = Some("0010".to_string());
        state.sessions = vec![run_a, run_b, run_c];
        let join = task_session_join(&state);
        assert_eq!(join.get("0010").map(Vec::len), Some(3));
        assert!(!join.contains_key("0010.1"));
    }

    #[test]
    fn rebuild_visible_tasks_emits_five_headers_and_collapse_toggles_persist_across_a_resync() {
        let mut state = State::new_without_worker();
        state.tasks_board = Some(board_with(
            vec![
                task_summary("0001", DerivedStatus::Ready),
                task_summary("0002", DerivedStatus::Blocked),
            ],
            BTreeMap::new(),
        ));
        rebuild_visible_tasks(&mut state);
        // Five groups always render a header, including empty ones; nothing
        // starts collapsed.
        assert_eq!(state.tasks_visible.len(), 5 + 2);
        for group in TaskGroup::order() {
            let index = state
                .tasks_visible
                .iter()
                .position(
                    |row| matches!(row, TaskVisibleRow::GroupHeader { group: candidate, .. } if *candidate == group),
                )
                .expect("group header");
            state.list_tasks.set_selected(index);
            toggle_selected_task_group(&mut state);
        }
        assert_eq!(state.collapsed_task_groups.len(), 5);
        assert_eq!(state.tasks_visible.len(), 5);

        // A resync (new summaries, same collapse set) keeps every group
        // collapsed rather than resetting it.
        state.tasks_board = Some(board_with(
            vec![
                task_summary("0001", DerivedStatus::Ready),
                task_summary("0002", DerivedStatus::Blocked),
                task_summary("0003", DerivedStatus::Done),
            ],
            BTreeMap::new(),
        ));
        rebuild_visible_tasks(&mut state);
        assert_eq!(state.tasks_visible.len(), 5);
        assert!(state.tasks_visible.iter().all(|row| matches!(
            row,
            TaskVisibleRow::GroupHeader {
                collapsed: true,
                ..
            }
        )));
    }

    /// 0063.8: a task-bound run whose last terminal merge frame landed
    /// derives a proposal on the exact same rebuild that produces
    /// `tasks_visible` — never a second pass, never persisted.
    #[test]
    fn rebuild_visible_tasks_derives_a_proposal_for_a_merged_bound_run() {
        let mut state = State::new_without_worker();
        let mut run = row_with_id("run-a", SessionClass::Terminal);
        run.task_key = Some("0100".to_string());
        run.merged_landed = Some("abc123".to_string());
        state.sessions = vec![run];
        state.tasks_board = Some(board_with(
            vec![task_summary("0100", DerivedStatus::Ready)],
            BTreeMap::new(),
        ));
        rebuild_visible_tasks(&mut state);
        let proposal = state
            .task_proposals
            .get("0100")
            .expect("proposal derived for the merged bound run");
        assert_eq!(proposal.evidence.len(), 1);
        assert_eq!(proposal.evidence[0].run_id, "r-run-a");
        assert_eq!(proposal.evidence[0].sha, "abc123");
    }

    /// A run that has not landed (`merged_landed: None`) never derives a
    /// proposal — the safe-absence direction 0064's Watch requires.
    #[test]
    fn rebuild_visible_tasks_derives_nothing_for_an_unmerged_bound_run() {
        let mut state = State::new_without_worker();
        let mut run = row_with_id("run-a", SessionClass::Live);
        run.task_key = Some("0100".to_string());
        state.sessions = vec![run];
        state.tasks_board = Some(board_with(
            vec![task_summary("0100", DerivedStatus::Ready)],
            BTreeMap::new(),
        ));
        rebuild_visible_tasks(&mut state);
        assert!(state.task_proposals.is_empty());
    }

    #[test]
    fn task_row_label_reserves_a_fixed_width_marker_for_a_pending_proposal() {
        let summary = task_summary("0100", DerivedStatus::Ready);
        let without = task_row_label(&summary, false);
        let with = task_row_label(&summary, true);
        assert_eq!(tui::display_width(&without), LIST_LABEL_WIDTH);
        assert_eq!(tui::display_width(&with), LIST_LABEL_WIDTH);
        assert_ne!(without, with);
        assert!(with.contains('!'));
    }

    #[test]
    fn dispatch_selected_task_refusal_names_the_reason_and_never_opens_the_spawn_modal() {
        let mut state = State::new_without_worker();
        let mut dependent = resolved_task("0002", DerivedStatus::Blocked);
        dependent.relations.depends_on = vec![ctx_traits_core::task::graph::ResolvedEdge {
            key: "0001".to_string(),
            title: "title 0001".to_string(),
            status: DerivedStatus::Ready,
        }];
        let mut resolved = BTreeMap::new();
        resolved.insert("0002".to_string(), dependent);
        state.tasks_board = Some(board_with(
            vec![task_summary("0002", DerivedStatus::Blocked)],
            resolved,
        ));
        rebuild_visible_tasks(&mut state);
        let index = state
            .tasks_visible
            .iter()
            .position(|row| matches!(row, TaskVisibleRow::Task(key) if key == "0002"))
            .expect("task row");
        state.list_tasks.set_selected(index);

        dispatch_selected_task(&mut state);

        assert!(!state.modal_host.is_open());
        let message = state.message.expect("refusal message set");
        assert!(message.contains("0002 depends on 0001 (ready)"));
        assert!(message.contains("--override-dependencies"));
    }

    #[test]
    fn dispatch_selected_task_refuses_before_any_sync_has_run() {
        let mut state = State::new_without_worker();
        state.tasks_board = None;
        // With no board cache there is no row to select; simulate the
        // "cursor sits on nothing yet" state directly.
        dispatch_selected_task(&mut state);
        assert!(!state.modal_host.is_open());
        assert_eq!(state.message.as_deref(), Some("no task selected"));
    }

    #[test]
    fn spawn_modal_seed_leads_with_the_configured_dispatch_trait() {
        let seed = spawn_modal_seed(Some("implement-quick"), "0063");
        let mut lines = seed.lines();
        assert_eq!(lines.next(), Some("implement-quick"));
        assert_eq!(lines.next(), Some("--task-dispatch"));
        assert_eq!(lines.next(), Some("--set"));
        assert_eq!(lines.next(), Some("task=0063"));
    }

    /// 0063.4's diagnose-first mandate: the seed's two-line `--set` /
    /// `task=<key>` form is three separate argv tokens once
    /// [`apply_spawn_request`]'s `.lines()` split runs — clap parses that
    /// identically to a single space-separated invocation, so the two-line
    /// form is not a distinct seam.
    #[test]
    fn spawn_modal_seed_two_line_set_form_parses_as_traits_run() {
        let seed = spawn_modal_seed(Some("implement-quick"), "0063");
        let user_args: Vec<String> = seed
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect();
        assert_eq!(
            user_args,
            vec!["implement-quick", "--task-dispatch", "--set", "task=0063"]
        );
        // See `every_visible_traits_command_has_a_registry_entry`
        // (presentation.rs): building the derived Clap tree overflows
        // `cargo test`'s default per-test thread stack in a debug build.
        std::thread::Builder::new()
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let mut full_argv: Vec<std::ffi::OsString> =
                    vec!["ctx".into(), "traits".into(), "run".into()];
                full_argv.extend(user_args.iter().map(std::ffi::OsString::from));
                match super::super::surface::cli::parse(full_argv) {
                    Ok(Some(super::super::surface::cli::Command::Traits {
                        subcommand: Some(super::super::surface::cli::TraitsCommand::Run { .. }),
                        ..
                    })) => {}
                    other => panic!(
                        "expected `ctx traits run implement-quick --task-dispatch --set task=0063` to parse, got {other:?}"
                    ),
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn spawn_modal_seed_names_the_missing_config_key_absent_a_default() {
        let seed = spawn_modal_seed(None, "0063");
        let mut lines = seed.lines();
        let comment = lines.next().expect("comment line");
        assert!(comment.starts_with('#'));
        assert!(comment.contains("dispatch-trait"));
        assert_eq!(lines.next(), Some(""));
        assert_eq!(lines.next(), Some("--task-dispatch"));
        assert_eq!(lines.next(), Some("--set"));
        assert_eq!(lines.next(), Some("task=0063"));
    }

    fn rline_text(line: &RLine<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn build_task_preview_renders_prose_between_status_and_relations_wrapped_to_width() {
        let mut resolved = resolved_task("0001", DerivedStatus::Ready);
        resolved.document.content = "\n\nfirst content line\nsecond content line\n\n".to_string();
        resolved.document.scope = "in scope".to_string();
        resolved.document.validation = "done when green".to_string();
        resolved.relations.depends_on = vec![ctx_traits_core::task::graph::ResolvedEdge {
            key: "0000".to_string(),
            title: "blocker".to_string(),
            status: DerivedStatus::Ready,
        }];
        let mut board_resolved = BTreeMap::new();
        board_resolved.insert("0001".to_string(), resolved);
        let board = board_with(
            vec![task_summary("0001", DerivedStatus::Ready)],
            board_resolved,
        );

        let preview = build_task_preview(
            &task_summary("0001", DerivedStatus::Ready),
            &board,
            &[],
            None,
            40,
        );
        let texts: Vec<String> = preview.lines.iter().map(rline_text).collect();

        let status_index = texts
            .iter()
            .position(|t| t.contains("status:"))
            .expect("status line");
        let content_index = texts
            .iter()
            .position(|t| t.contains("first content line"))
            .expect("content line");
        let scope_index = texts
            .iter()
            .position(|t| t.contains("in scope"))
            .expect("scope line");
        let validation_index = texts
            .iter()
            .position(|t| t.contains("done when green"))
            .expect("validation line");
        let relation_index = texts
            .iter()
            .position(|t| t.contains("blocked by"))
            .expect("relation line");

        assert!(status_index < content_index);
        assert!(content_index < scope_index);
        assert!(scope_index < validation_index);
        assert!(validation_index < relation_index);
        for line in &preview.lines {
            assert!(line.width() <= 40, "line exceeded wrap width: {line:?}");
        }
    }

    #[test]
    fn build_task_preview_with_empty_prose_matches_the_pre_prose_shape() {
        let resolved = resolved_task("0001", DerivedStatus::Ready);
        let mut board_resolved = BTreeMap::new();
        board_resolved.insert("0001".to_string(), resolved);
        let board = board_with(
            vec![task_summary("0001", DerivedStatus::Ready)],
            board_resolved,
        );

        let preview = build_task_preview(
            &task_summary("0001", DerivedStatus::Ready),
            &board,
            &[],
            None,
            200,
        );
        let texts: Vec<String> = preview.lines.iter().map(rline_text).collect();
        assert!(!texts.iter().any(|t| t.contains("content:")));
        assert!(!texts.iter().any(|t| t.contains("scope:")));
        assert!(!texts.iter().any(|t| t.contains("validation:")));
        let status_index = texts
            .iter()
            .position(|t| t.contains("status:"))
            .expect("status line");
        let steps_index = texts
            .iter()
            .position(|t| t.contains("open steps:"))
            .expect("open steps header");
        // No blank slab inserted between the (empty) status block and the
        // (also empty) relations/open-steps block: exactly the two blank
        // separators that existed before prose sections did (one before the
        // resolve guard, one before "open steps:").
        assert_eq!(steps_index - status_index, 3);
    }

    #[test]
    fn apply_pane_scroll_clamps_to_task_preview_content_len() {
        let mut state = State::new_without_worker();
        state.screen = Screen::Tasks;
        state.last_pane_layout =
            build_tree_for_screen(&state, 100).resolve(Rect::new(0, 0, 100, 3));
        state.task_preview = Some(TaskPreview {
            key: "0001".to_string(),
            lines: vec![RLine::from("a"), RLine::from("b"), RLine::from("c")],
        });
        apply_pane_scroll(&mut state, PANE_TASKS_PREVIEW, ScrollDelta::Down(100));
        let scroll = state.pane_scrolls.get(PANE_TASKS_PREVIEW);
        assert_eq!(scroll.window(1), 2..3);
    }

    #[test]
    fn parse_task_edit_input_status_form() {
        let update = parse_task_edit_input("status ready").unwrap();
        assert_eq!(update.status, Some(TaskDocStatus::Ready));
        let update = parse_task_edit_input("status done").unwrap();
        assert_eq!(update.status, Some(TaskDocStatus::Done));
        let update = parse_task_edit_input("status cancelled").unwrap();
        assert_eq!(update.status, Some(TaskDocStatus::Cancelled));
        assert!(parse_task_edit_input("status bogus").is_err());
        assert!(parse_task_edit_input("status").is_err());
    }

    #[test]
    fn parse_task_edit_input_dep_add_and_remove_forms() {
        let update = parse_task_edit_input("dep +0010").unwrap();
        assert_eq!(update.add_depends_on, vec!["0010".to_string()]);
        assert!(update.remove_depends_on.is_empty());

        let update = parse_task_edit_input("dep -0010").unwrap();
        assert_eq!(update.remove_depends_on, vec!["0010".to_string()]);
        assert!(update.add_depends_on.is_empty());

        assert!(parse_task_edit_input("dep +").is_err());
        assert!(parse_task_edit_input("dep -").is_err());
    }

    #[test]
    fn parse_task_edit_input_dep_repoint_form_is_one_remove_plus_one_add() {
        let update = parse_task_edit_input("dep 0010 0011").unwrap();
        assert_eq!(update.remove_depends_on, vec!["0010".to_string()]);
        assert_eq!(update.add_depends_on, vec!["0011".to_string()]);
        assert!(parse_task_edit_input("dep 0010").is_err());
    }

    #[test]
    fn parse_task_archive_input_recognizes_the_release_token() {
        let (status, release) = parse_task_archive_input("done").unwrap();
        assert_eq!(status, TaskDocStatus::Done);
        assert!(!release);

        let (status, release) = parse_task_archive_input("done release").unwrap();
        assert_eq!(status, TaskDocStatus::Done);
        assert!(release);

        let (status, release) = parse_task_archive_input("cancelled release").unwrap();
        assert_eq!(status, TaskDocStatus::Cancelled);
        assert!(release);

        let (status, release) = parse_task_archive_input("canceled").unwrap();
        assert_eq!(status, TaskDocStatus::Cancelled);
        assert!(!release);

        // Anything after `release` (or a typo instead of it) is ignored —
        // only its presence as the second token opts in.
        let (_, release) = parse_task_archive_input("done nope").unwrap();
        assert!(!release);

        assert!(parse_task_archive_input("bogus").is_err());
        assert!(parse_task_archive_input("").is_err());
    }

    #[test]
    fn parse_task_edit_input_rejects_unknown_forms_and_empty_input() {
        assert!(parse_task_edit_input("").is_err());
        assert!(parse_task_edit_input("bogus").is_err());
    }

    // -----------------------------------------------------------------
    // TASKS board freshness (0063.7)
    // -----------------------------------------------------------------

    fn tasks_board_tempdir() -> camino::Utf8PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dashboard-tasks-board-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        camino::Utf8PathBuf::from_path_buf(dir).unwrap()
    }

    /// A `State` with [`State::tasks_cache_root`] pointed at a scratch temp
    /// directory, so persistence in these tests never touches the real
    /// `~/.config/ctx/cache`.
    fn state_with_scratch_cache() -> State {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dashboard-tasks-cache-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = State::new_without_worker();
        state.tasks_cache_root = Some(camino::Utf8PathBuf::from_path_buf(dir).unwrap());
        state
    }

    fn write_task_toml(dir: &camino::Utf8Path, file_name: &str, key: &str) {
        std::fs::write(
            dir.join(file_name).as_std_path(),
            format!(
                "schema-version = \"0.2\"\nkey = \"{key}\"\ntitle = \"title {key}\"\nstatus = \"ready\"\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn tick_refresh_applies_a_changed_board() {
        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);
        assert_eq!(state.tasks_board.as_ref().unwrap().summaries.len(), 1);

        write_task_toml(&dir, "0002-second.toml", "0002");
        refresh_tasks_board_if_stale_in(&mut state, &dir);
        assert_eq!(state.tasks_board.as_ref().unwrap().summaries.len(), 2);
        assert!(state.tasks_refresh_error.is_none());
    }

    #[test]
    fn tick_refresh_is_a_no_op_when_the_fingerprint_is_unchanged() {
        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);
        let captured_at = state.tasks_board.as_ref().unwrap().captured_at;

        // A second sweep with nothing changed on disk must not disturb the
        // already-captured snapshot (same fingerprint => no re-read).
        refresh_tasks_board_if_stale_in(&mut state, &dir);
        assert_eq!(state.tasks_board.as_ref().unwrap().captured_at, captured_at);
    }

    #[test]
    fn failed_re_read_keeps_the_prior_snapshot_and_sets_the_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);
        assert_eq!(state.tasks_board.as_ref().unwrap().summaries.len(), 1);

        let archived = dir.join("archived");
        std::fs::create_dir_all(archived.as_std_path()).unwrap();
        std::fs::set_permissions(
            archived.as_std_path(),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();

        refresh_tasks_board_if_stale_in(&mut state, &dir);

        // Restore permissions before any assertion can panic and leak an
        // unreadable directory into the temp root's cleanup.
        std::fs::set_permissions(
            archived.as_std_path(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();

        assert_eq!(state.tasks_board.as_ref().unwrap().summaries.len(), 1);
        assert!(state.tasks_refresh_error.is_some());
    }

    // -----------------------------------------------------------------
    // 0149: `y`'s mark-done write must not have its own confirmation
    // clobbered by the resync that follows it.
    // -----------------------------------------------------------------

    #[test]
    fn mark_done_reports_the_confirmation_and_applies_status_and_archive() {
        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);

        let provider = FilesTaskBoard::open_read_write(dir.clone());
        let digest = provider.get("0001").unwrap().expect("task resolves").digest;

        apply_task_mark_done_in(
            &mut state,
            &dir,
            "0001".to_string(),
            digest,
            vec![super::super::task_proposals::MergedRunEvidence {
                run_id: "run-1".to_string(),
                sha: "deadbeef".to_string(),
            }],
        )
        .unwrap();

        // The write's own confirmation must survive the resync that follows
        // it — not be overwritten with "synced" (the bug this test guards).
        let message = state.message.as_deref().unwrap_or("");
        assert!(
            message.contains("0001 marked done"),
            "expected a mark-done confirmation, got {message:?}"
        );

        let resolved = provider.get("0001").unwrap().expect("task still resolves");
        assert_eq!(resolved.document.status, Some(TaskDocStatus::Done));
        assert!(
            dir.join("archived").join("0001-first.toml").exists(),
            "archive placement must move the file under archived/"
        );
        assert!(!dir.join("0001-first.toml").exists());
    }

    #[test]
    fn mark_done_refuses_and_reports_a_stale_digest_without_moving_the_file() {
        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);

        apply_task_mark_done_in(
            &mut state,
            &dir,
            "0001".to_string(),
            "sha256:stale-digest-that-never-matches".to_string(),
            vec![super::super::task_proposals::MergedRunEvidence {
                run_id: "run-1".to_string(),
                sha: "deadbeef".to_string(),
            }],
        )
        .unwrap();

        let message = state.message.as_deref().unwrap_or("");
        assert!(
            message.contains("mark done refused"),
            "expected a visible refusal, got {message:?}"
        );
        assert!(dir.join("0001-first.toml").exists());
        assert!(!dir.join("archived").join("0001-first.toml").exists());
    }

    #[test]
    fn title_renders_age_from_captured_at_and_notes_a_refresh_failure() {
        let mut state = State::new_without_worker();
        assert_eq!(tasks_list_title(&state), "tasks (no board read yet)");

        state.tasks_board = Some(board_with(
            vec![task_summary("0001", DerivedStatus::Ready)],
            BTreeMap::new(),
        ));
        assert!(tasks_list_title(&state).starts_with("tasks (as of "));
        assert!(!tasks_list_title(&state).contains("refresh failed"));

        state.tasks_refresh_error = Some("boom".to_string());
        assert!(tasks_list_title(&state).ends_with("— refresh failed"));
    }

    #[test]
    fn selected_task_key_survives_a_re_read_that_reshuffles_groups() {
        let dir = tasks_board_tempdir();
        write_task_toml(&dir, "0001-first.toml", "0001");
        write_task_toml(&dir, "0002-second.toml", "0002");
        let mut state = state_with_scratch_cache();
        sync_tasks_board_in(&mut state, &dir);

        let index = state
            .tasks_visible
            .iter()
            .position(|row| matches!(row, TaskVisibleRow::Task(key) if key == "0002"))
            .expect("0002 row present");
        state.list_tasks.set_selected(index);

        // Close 0001 so it moves to the Done group ahead of 0002 in the
        // fixed group order, reshuffling row indices.
        let doc_0001 =
            "schema-version = \"0.2\"\nkey = \"0001\"\ntitle = \"title 0001\"\nstatus = \"done\"\n";
        std::fs::write(dir.join("0001-first.toml").as_std_path(), doc_0001).unwrap();
        refresh_tasks_board_if_stale_in(&mut state, &dir);

        match state.tasks_visible.get(state.list_tasks.selected()) {
            Some(TaskVisibleRow::Task(key)) => assert_eq!(key, "0002"),
            Some(TaskVisibleRow::GroupHeader { .. }) => panic!("selection landed on a header"),
            None => panic!("selection landed out of bounds"),
        }
    }

    // --- 0064 split-from-park-report mapping ---------------------------

    fn park_blocker(id: &str, what: &str, open: bool) -> ctx_traits_io::run_session::ParkBlocker {
        ctx_traits_io::run_session::ParkBlocker {
            id: id.to_string(),
            location: "modules/x.rs".to_string(),
            what: what.to_string(),
            root_cause: "missing invariant".to_string(),
            required_fix: "establish the invariant".to_string(),
            steps: vec![ctx_traits_io::run_session::ParkBlockerStep {
                step: "fix it".to_string(),
                status: if open { "open" } else { "done" }.to_string(),
                evidence: String::new(),
            }],
            done_when: "the fix is verified".to_string(),
        }
    }

    #[test]
    fn split_children_from_park_skips_already_closed_blockers() {
        let report = ctx_traits_io::run_session::ParkReportEntry {
            status: "revise".to_string(),
            blockers: vec![
                park_blocker("open-one", "an open defect", true),
                park_blocker("closed-one", "a resolved defect", false),
            ],
            wall_id: String::new(),
        };
        let children = split_children_from_park(&report);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].title, "an open defect");
        assert!(children[0].content.contains("missing invariant"));
        assert_eq!(children[0].validation, "the fix is verified");
        assert_eq!(children[0].steps.len(), 1);
        assert_eq!(children[0].steps[0].title, "fix it");
        assert!(!children[0].steps[0].done);
    }

    #[test]
    fn split_children_from_feasibility_uses_missing_entries() {
        let verdict = ctx_traits_io::run_session::FeasibilityVerdict {
            verdict: "oversized".to_string(),
            evidence: "checked every reference".to_string(),
            missing: vec!["a shared cache abstraction".to_string()],
            owner_action: "split the task".to_string(),
        };
        let children = split_children_from_feasibility(&verdict);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].title, "a shared cache abstraction");
        assert!(children[0].content.contains("checked every reference"));
        assert!(children[0].validation.is_empty());
    }

    #[test]
    fn split_child_title_truncates_long_blocker_text() {
        let long = "x".repeat(200);
        let title = split_child_title(&long);
        assert!(title.chars().count() <= 97);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn split_child_title_leaves_short_text_untouched() {
        assert_eq!(split_child_title("  a short title  "), "a short title");
    }

    #[test]
    fn reconcile_completion_message_names_every_ambiguous_finding() {
        let mut state = State::new();
        state.reconcile_ambiguous = vec![super::super::task_proposals::AmbiguousFinding {
            task_key: "0100".to_string(),
            reason: "no ancestry evidence".to_string(),
        }];
        let message = reconcile_completion_message(&state);
        assert!(message.contains("0100"));
        assert!(message.contains("no ancestry evidence"));
    }

    #[test]
    fn reconcile_completion_message_is_clean_when_nothing_ambiguous() {
        let state = State::new();
        assert_eq!(
            reconcile_completion_message(&state),
            "reconcile: no ambiguous findings"
        );
    }

    #[test]
    fn declared_checks_notice_lists_every_command_before_confirm() {
        let mut body = "existing body".to_string();
        let checks = vec![ctx_traits_core::task::Check {
            name: "unit tests".to_string(),
            command: "cargo test -p ctx-traits-core".to_string(),
            timeout_ms: None,
            expect: None,
        }];
        append_declared_checks_notice(&mut body, &checks);
        assert!(body.contains("declared checks (run on confirm):"));
        assert!(body.contains("unit tests: cargo test -p ctx-traits-core"));
    }

    #[test]
    fn declared_checks_notice_is_a_no_op_with_no_declared_checks() {
        let mut body = "existing body".to_string();
        append_declared_checks_notice(&mut body, &[]);
        assert_eq!(body, "existing body");
    }
}
#[test]
fn dashboard_guide_tokens_compact_labels_survive_token_column_width() {
    let mut usage = ctx_traits_core::procedure::session::TokenUsageEvidence::default();
    assert_eq!(dashboard_tokens_text(Some(&usage)), "-");
    usage.guide_tokens = Some(3);
    assert_eq!(dashboard_tokens_text(Some(&usage)), "W:- N:- G:3");
    usage.work_tokens = Some(10);
    usage.narrator_tokens = Some(2);
    assert_eq!(dashboard_tokens_text(Some(&usage)), "W:10 N:2 G:3");
    assert_eq!(
        list_field(&dashboard_tokens_text(Some(&usage)), 15),
        "W:10 N:2 G:3   "
    );
    usage.work_tokens = Some(1_000);
    usage.narrator_tokens = Some(1_000);
    usage.guide_tokens = Some(1_000);
    assert_eq!(
        list_field(&dashboard_tokens_text(Some(&usage)), 15),
        "W:1k N:1k G:1k "
    );
}
