//! `ctx traits merge <run-id>`: land a completed `--worktree` run.
//!
//! A true clean fast-forward (`main` is the run branch's merge base) lands
//! deterministically: no rebase, no merger resolution/probing/dispatch. Every
//! other case (a divergent history, or `--force-merger`) rebases the run's
//! `ctx/run/<id>` branch onto clean `main` and invokes the standing merger
//! agent for mechanical reconciliation (never redesign or re-review) —
//! `--deep` (P420) instead invokes a judgment-capable merger that resolves
//! conflicts under five stable doctrines, records a typed per-hunk decision
//! receipt, and refuses a landed-fix regression rather than guessing — before
//! fast-forwarding. Both paths converge on the same landing/cleanup tail: the
//! declared `[merge] gate` (P477) — an ordered list of literal argv commands,
//! empty by default — seed harvest, a lost-fast-forward recheck, the
//! fast-forward itself, and worktree/branch removal. Any failed precondition,
//! unresolved conflict, judgment call, red gate, or lost fast-forward race
//! parks the run with its branch and worktree intact.

use std::io::IsTerminal;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

use ctx_traits_core::procedure::activity::ActivityEvent;
use ctx_traits_core::procedure::session::{
    DeepMergeDecision, DeepMergeRule, MergeFrame, MergeStage, MergeStatus, ParkReason, Session,
    WorktreeProvenance,
};
use ctx_traits_io::debug_trace::{HarnessAttemptExit, HarnessAttemptStart, HarnessAttemptWriter};

use crate::app::agent_dispatch;
use crate::app::agent_dispatch::RunOneShotObservers;
use crate::app::entry::print_json_report;
use crate::app::harness_stream;
use crate::app::merge_story::{self, MergeProgress, reasons};
use crate::app::presentation::{OutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human};
use crate::app::run::run_envelope;

/// P549: a cheap, cloneable live-activity sink threaded optionally through
/// [`MergeInputs`] — `None` for dashboard's `m` action (its own live surface
/// is P523's concern). A per-merge closure over the landing run's own state
/// — no global/thread-local state, so a concurrent merge under the shared
/// `merge_lock` can never write into another process's view. `sequence` is
/// assigned here, once, so every call site only supplies a [`MergeProgress`]
/// point and never hand-rolls a sequence number.
#[derive(Clone)]
pub(crate) struct MergeLive {
    sequence: Arc<std::sync::atomic::AtomicU64>,
    sink: Arc<dyn Fn(ActivityEvent) + Send + Sync>,
}

impl MergeLive {
    /// Builds a live sink around any per-event consumer — a handed-off
    /// `RunPanel::merge_event`, a plain stage-boundary stderr line, or a
    /// TTY-only themed one. The one place `sequence` starts at 0 for a given
    /// merge attempt.
    pub(crate) fn new(sink: impl Fn(ActivityEvent) + Send + Sync + 'static) -> Self {
        Self {
            sequence: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            sink: Arc::new(sink),
        }
    }

    fn emit(&self, progress: &MergeProgress<'_>) {
        let draft = merge_story::activity_event(progress);
        (self.sink)(ActivityEvent {
            sequence: self
                .sequence
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            frame_id: draft.frame_id,
            kind: draft.kind,
            text: draft.text,
            tool: draft.tool,
            tokens: None,
        });
    }

    /// P549: forward an already-typed [`ActivityEvent`] (produced by a
    /// [`ctx_traits_io::harness_activity::HarnessActivityAdapter`] parsing
    /// the merger's own stream, the same P504 vocabulary a driven frame's
    /// stream speaks) — re-sequenced under this merge attempt's own counter
    /// rather than the adapter's, so interleaving with `emit`'s own events
    /// never produces a non-monotonic sequence.
    fn emit_activity_event(&self, mut event: ActivityEvent) {
        event.sequence = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (self.sink)(event);
    }
}

/// P549 non-pane live surface for a status-mode (or TUI-degraded) run/drive
/// completion: one plain stderr line per event that carries text — the
/// replacement for the deleted dim 60s tick line off-pane. Unconditional,
/// no TTY gate, matching drive's own status-mode progress lines
/// (`emit_output_progress`'s `Status` arm, `command_started_event`) rather
/// than the TTY-gated styling `handle_merge`'s own live channel uses below —
/// a status-mode run's stderr is always plain by convention.
pub(crate) fn plain_stage_line_live() -> MergeLive {
    MergeLive::new(|event: ActivityEvent| {
        if let Some(text) = event.text {
            eprintln!("merge: {text}");
        }
    })
}

/// P549 standalone `ctx traits merge` live surface: `None` off a real
/// terminal (today's non-TTY behavior — the deleted tick line was the only
/// non-TTY live output, and its removal is this phase's explicit order); a
/// named-ANSI faint line per event on a TTY, same convention the deleted
/// dim tick line used.
pub(crate) fn tty_stage_line_live() -> Option<MergeLive> {
    if !std::io::stderr().is_terminal() {
        return None;
    }
    Some(MergeLive::new(|event: ActivityEvent| {
        let Some(text) = event.text else { return };
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "\x1b[2mmerge: {text}\x1b[0m");
    }))
}

/// P488: the single stable advisory text recorded whenever the landing
/// branch could not be discovered from `[merge] branch` config, the
/// `origin/HEAD` symref, or `init.defaultBranch`, and fell back to the
/// literal `"main"` assumption — the fallback's discoverability path.
const DEFAULT_BRANCH_FALLBACK_ADVISORY: &str = "default branch assumed \"main\": no [merge] \
     branch config, origin/HEAD, or init.defaultBranch; set [merge] branch to override";
/// The single stable advisory text for an empty (undeclared) `[merge] gate`
/// — recorded once in the `Gates` ledger frame's evidence AND propagated
/// into `MergeReport::warnings` so it is user-visible in both the plain and
/// `--json` command output, not only in persisted ledger evidence.
const EMPTY_GATE_ADVISORY: &str =
    "no [merge] gate declared; post-run without running a repository command";
/// Built-in default wall-clock budget for ONE merger harness call (standard
/// and deep alike), when neither `[agent.role.merger].budget.frame-seconds`
/// nor `[agent.role.merger-deep].budget.frame-seconds` is declared (P475).
/// 2026-07-25 (owner): raised 300s → 900s after live deep merges timed out
/// mid-judgment on wide conflict surfaces (nine overlapping files, full
/// per-hunk receipt) and the kill surfaced as the invalid-receipt
/// park. Kept BELOW frame budgets deliberately: [`merge_lock_wait_timeout_ms`]
/// multiplies the RESOLVED merger timeout by [`MAX_RECONCILIATION_ITERATIONS`],
/// so every increase to the resolved merger budget inflates the worst-case
/// dead-lock wait 25× — see `doctor --config`'s derived lock-wait row.
pub(crate) const DEFAULT_MERGER_TIMEOUT_MS: u64 = 900_000;

/// The merger seat's resolved one-shot call budget (P475, D3): its own
/// `budget.frame-seconds` if declared, else [`DEFAULT_MERGER_TIMEOUT_MS`].
/// Never joined to the run-level `[run]`/CLI-flag chain drive frames use —
/// the merger call is a one-shot dispatch outside the drive frame loop, so a
/// `[run] frame-seconds` declared for drive frames must never silently cut
/// merger calls too (the 2026-07-25 masked-timeout incident this phase
/// closes).
fn merger_timeout_ms(assignment: &ctx_traits_io::harness_config::ProfileAssignment) -> u64 {
    match assignment.budget.frame_seconds {
        Some(seconds) => seconds.saturating_mul(1_000),
        None => DEFAULT_MERGER_TIMEOUT_MS,
    }
}
/// Defense-in-depth cap on merger reconciliation rounds (each round is a paid
/// harness call): a well-behaved reconciliation lands in a handful of
/// continue passes, so this is generous headroom, not a tuned expectation.
pub(crate) const MAX_RECONCILIATION_ITERATIONS: u32 = 25;
/// Fixed overhead budget for the Git/filesystem work a merge attempt does
/// outside merger calls (rebase, fast-forward, cleanup).
pub(crate) const MERGE_LOCK_WAIT_OVERHEAD_MS: u64 = 60_000;

/// Bounded wall-clock budget for the declared `[merge] gate` (P477): every
/// command shares the same resolved `gate-seconds` per-command ceiling, so
/// the aggregate is `gate.len() * gate-seconds` converted to milliseconds —
/// named once here so [`merge_lock_wait_timeout_ms`] cannot silently drift
/// from the budget each gate command actually runs under.
pub(crate) fn landing_gate_total_timeout_ms(
    policy: &ctx_traits_io::harness_config::EffectiveMergePolicy,
) -> u64 {
    let per_command_ms = policy.gate_seconds.saturating_mul(1_000);
    (policy.gate.len() as u64).saturating_mul(per_command_ms)
}

/// Bound `ctx traits merge` is willing to wait for the cross-process merge
/// lock: it must cover the complete worst-case bounded merge lifecycle a
/// healthy holder can spend inside the lock — every permitted merger
/// reconciliation call, plus the FULL declared `[merge] gate` budget (every
/// command's ceiling, not just one) — so a queued, healthy merge is never
/// starved out by a timeout that understates the holder's own budget.
/// Derived from the same constants/config those calls use rather than an
/// independently maintained number that could silently drift from them.
/// Never infinite — a wedged holder must not deadlock every other batch.
/// `merger_timeout_ms` (P475) is the RESOLVED merger-seat budget for the rung
/// being run — a config bump here is exactly what multiplies the worst-case
/// wait 25×, which is why `doctor --config` surfaces the derived value.
pub(crate) fn merge_lock_wait_timeout_ms(
    policy: &ctx_traits_io::harness_config::EffectiveMergePolicy,
    merger_timeout_ms: u64,
) -> u64 {
    (MAX_RECONCILIATION_ITERATIONS as u64)
        .saturating_mul(merger_timeout_ms)
        .saturating_add(landing_gate_total_timeout_ms(policy))
        .saturating_add(MERGE_LOCK_WAIT_OVERHEAD_MS)
}

/// The merger seat's resolved budget for the rung about to run (`--deep` →
/// `merger-deep`, falling back to `merger` when the deep table is absent —
/// mirroring [`resolve_merger`]'s own fallback), read directly from the
/// already-scope-merged `RuntimeConfig` rather than a full
/// `ResolvedRuntimeAssignments` — the lock-wait ceiling must be derivable
/// BEFORE merger resolution (a true clean fast-forward never constructs one
/// at all, see `merge_locked`'s fast-path comment). Budgets are declared as
/// config-only in the P475 draft's stated scope, but the `--assign
/// role=json:{...}` decoder accepts the `budget` field on whatever
/// `ProfileAssignment` it decodes like any other field (validated the same
/// way a config table's budget is, via `validate_role_budget`) — `--assign`
/// never reaches this call site, which resolves the rung's budget straight
/// from the raw config table `[agent.role.<seat>]`, always the source of
/// truth for `doctor --config`'s lock-wait derivation.
pub(crate) fn resolved_merger_role_budget(
    agent: &ctx_traits_io::harness_config::AgentDefaults,
    deep: bool,
) -> ctx_traits_io::harness_config::RoleBudget {
    let primary = if deep { "merger-deep" } else { "merger" };
    let table = agent
        .role
        .get(primary)
        .and_then(|value| value.entries().first())
        .or_else(|| {
            deep.then(|| agent.role.get("merger"))
                .flatten()
                .and_then(|value| value.entries().first())
        });
    table
        .map(|assignment| assignment.budget.clone())
        .unwrap_or_default()
}

pub(crate) struct MergeInputs<'a> {
    pub(crate) run_id: &'a str,
    pub(crate) session_store: Option<&'a str>,
    /// P460: the exact ledger path already resolved by the
    /// completion-to-landing caller (`run`/`session start`/`drive` resume),
    /// read directly instead of the `run_id`/`session_store` scan below —
    /// so a session written to an explicit `--out` path outside
    /// `session_store` still resolves. `None` for the explicit `ctx traits
    /// merge <run-id>` CLI surface (and the dashboard's `m` action), which
    /// keep resolving by run-id/store scan.
    pub(crate) session_path_override: Option<&'a Utf8Path>,
    pub(crate) assignments: &'a [String],
    pub(crate) no_wait: bool,
    pub(crate) force_wait: bool,
    pub(crate) json: bool,
    /// Send this merge through the standing merger agent's confirmation path
    /// even when `main` is a true clean fast-forward for the run branch
    /// (which would otherwise skip merger resolution/probing/dispatch
    /// entirely). Has no effect on a divergent history, which always uses the
    /// merger regardless of this flag.
    pub(crate) force_merger: bool,
    /// Park on a detected stale-base overlap (the run branch and `main` both
    /// changed the same paths since the branch base) instead of the default
    /// of landing anyway with the same overlap evidence recorded.
    pub(crate) park_on_overlap: bool,
    pub(crate) force_land_on_overlap: bool,
    /// Deprecated no-op, kept for one release: landing despite a detected
    /// stale-base overlap is now the default, so this flag no longer changes
    /// behavior. Its only remaining effect is a deprecation warning on the
    /// report.
    pub(crate) allow_stale_overlap: bool,
    /// P420: reconcile a divergent history through the judgment-capable deep
    /// merger (`[agent.role.merger-deep]`, falling back to
    /// `[agent.role.merger]`) instead of the standard mechanical-only
    /// merger. No effect on a true clean fast-forward unless combined with
    /// `force_merger`.
    pub(crate) deep: bool,
    /// P549: optional live-activity sink. `None` (every existing caller
    /// today) means no live emission — no behavior change, no serialized
    /// output change; see [`MergeLive`].
    pub(crate) live: Option<MergeLive>,
    /// P549: optional raw-byte observer fed every merger dispatch's stdout,
    /// ALONGSIDE (never instead of) the debug-trace writer and the P504
    /// activity adapter this module wires unconditionally when `live` is
    /// present. Built by the completion-to-landing caller: a
    /// [`harness_stream::NarratorFeeder`] when a narrator seat resolved, or
    /// a handed-off panel's raw passthrough (`RunPanel::push_bytes`)
    /// otherwise — `None` for every caller with no live surface at all
    /// (standalone `ctx traits merge` off a TTY, the dashboard's `m`
    /// action).
    pub(crate) merger_stdout_observer: Option<ctx_traits_io::harness::OutputObserver>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct MergeReport {
    pub(crate) status: String,
    pub(crate) run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    /// Operational cross-process merge-lock evidence (wait duration, observed
    /// holder, stale-holder reclaim). Present whenever the lock outcome is
    /// known: acquired, busy (`--no-wait`), or timed out. Never canonical —
    /// see `ctx_traits_core::procedure::session::MergeStatus::LockAcquired`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lock_wait_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lock_holder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lock_stale_holder_reclaimed: Option<String>,
    /// Stable `git-lock-retry` evidence accumulated across the Git calls of
    /// this merge lifecycle. Empty (and omitted from JSON) when no Git call
    /// hit classified transient lock contention, so an uncontended merge's
    /// serialized output is unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) warnings: Vec<String>,
    /// Stable `stale-base-overlap` evidence (overlapping paths and the
    /// responsible `main` commits), recorded whenever a stale-base overlap
    /// was detected and this merge landed anyway (the default). Empty (and
    /// omitted from JSON) when no overlap was detected, or one was detected
    /// and `--park-on-overlap` parked instead (that outcome is `status:
    /// "parked"`, with its own ledger frame).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) stale_base_overlap: Vec<String>,
}

enum MergerDecision {
    Proceed {
        /// Empty for the standard mechanical-only merger; one entry per
        /// P420 `--deep` typed decision otherwise.
        deep_decisions: Vec<DeepMergeDecision>,
    },
    Park {
        reason: String,
        /// The deep merger's attempted decision log up to a rule-4 refusal
        /// (or any other deep park). Empty for the standard merger and for a
        /// harness-level park (timeout/truncation/unparseable reply).
        deep_decisions: Vec<DeepMergeDecision>,
    },
}

pub(crate) fn handle_merge(
    input: MergeInputs<'_>,
) -> crate::Result<ctx_traits_core::response::CommandOutput<()>> {
    let json = input.json;
    let report = merge(input)?;
    match OutputMode::select(json, false) {
        OutputMode::Json => {
            print_json_report(&run_envelope(report.clone(), false, false, false), "merge")?;
        }
        OutputMode::Human(_) => {
            print_report(&report)?;
        }
    }
    let disposition = crate::app::run::disposition_for_report_status(&report.status);
    if let Some(exit_code) = disposition.exit_code() {
        return Err(crate::Error::AlreadyReported {
            message: format!(
                "merge {} for run {:?}{}",
                report.status,
                report.run_id,
                report
                    .reason
                    .as_deref()
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default()
            ),
            exit_code,
        });
    }
    Ok(ctx_traits_core::response::CommandOutput::new(()))
}

/// The one plain-text rendering of a [`MergeReport`], shared by
/// `handle_merge` and P460's combined run/drive completion report (so a
/// merge attempted as part of `run --merge`/`session start --merge`/`drive`
/// resume prints identically to an explicit `ctx traits merge`).
pub(crate) fn print_report(report: &MergeReport) -> crate::Result<()> {
    let status = if report.status == "merged" {
        PanelStatus::Passed("passed".to_string())
    } else {
        PanelStatus::Blocked(report.status.clone())
    };
    let mut panel = Panel::new("ctx", "merge", status).row(PanelRow::toned(
        "run-id",
        report.run_id.as_str(),
        RowTone::Default,
    ));
    if report.status != "merged" {
        let explanation = merge_story::explain_report(report);
        panel = panel.next(PanelRow::toned(
            "reason",
            explanation.sentence.clone(),
            RowTone::Default,
        ));
        panel = panel.row(PanelRow::toned(
            "next",
            explanation.next_action.clone(),
            RowTone::Default,
        ));
    }
    if let Some(reason) = report.reason.as_deref() {
        panel = panel.row(PanelRow::toned("raw-reason", reason, RowTone::Default));
    }
    if let Some(wait_ms) = report.lock_wait_ms {
        panel = panel.row(PanelRow::toned(
            "lock-wait-ms",
            wait_ms.to_string(),
            RowTone::Default,
        ));
    }
    if let Some(holder) = report.lock_holder.as_deref() {
        panel = panel.row(PanelRow::toned("lock-holder", holder, RowTone::Default));
    }
    if let Some(stale) = report.lock_stale_holder_reclaimed.as_deref() {
        panel = panel.row(PanelRow::toned(
            "lock-stale-holder-reclaimed",
            stale,
            RowTone::Default,
        ));
    }
    for warning in &report.warnings {
        panel = panel.row(PanelRow::toned("warning", warning, RowTone::Warn));
    }
    for overlap in &report.stale_base_overlap {
        panel = panel.row(PanelRow::toned(
            "stale-base-overlap",
            overlap,
            RowTone::Default,
        ));
    }
    emit_human(
        false,
        &panel,
        crate::app::presentation::HumanOutputMode::Compact,
        || Ok(()),
    )
}

/// Report-returning merge operation, split out from [`handle_merge`] so the
/// P423 dashboard's MERGES-screen `m` action can invoke exactly the same
/// landing logic without going through CLI printing/exit-code conversion.
pub(crate) fn merge(input: MergeInputs<'_>) -> crate::Result<MergeReport> {
    // --- Resolve: unresolvable runs return a plain error, Git state untouched. ---
    let (session_path, session) = match input.session_path_override {
        Some(path) => (
            path.to_path_buf(),
            ctx_traits_io::run_session::read_run_session(path)?,
        ),
        None => {
            let Some(resolved) = ctx_traits_io::run_session::find_session_by_run_id(
                input.session_store,
                input.run_id,
            )?
            else {
                return Err(crate::Error::Command {
                    message: format!(
                        "merge unresolvable: no run-session ledger records run-id {:?}",
                        input.run_id
                    ),
                });
            };
            resolved
        }
    };
    if session.status != ctx_traits_core::procedure::session::Status::Completed
        || session
            .last_drive_outcome
            .as_ref()
            .is_none_or(|outcome| !outcome.outcome.is_completed())
    {
        return Err(crate::Error::Command {
            message: format!(
                "merge unresolvable: run {:?} is not a completed drive (status={:?}, last-drive-outcome={:?})",
                input.run_id,
                session.status,
                session
                    .last_drive_outcome
                    .as_ref()
                    .map(|outcome| outcome.outcome.as_str())
            ),
        });
    }
    let Some(worktree) = session.provenance.worktree.clone() else {
        return Err(crate::Error::Command {
            message: format!(
                "merge unresolvable: run {:?} has no recorded worktree provenance (started without --worktree, or a legacy ledger)",
                input.run_id
            ),
        });
    };

    let repo_root = ctx_traits_io::repository::discover_repo_root()?;
    let runtime_config = ctx_traits_io::harness_config::resolve_runtime_config(Utf8Path::new("."))?;
    let merge_policy = runtime_config.effective_merge_policy();
    // P488: resolve the landing branch exactly ONCE per merge attempt — a
    // retry after `git remote set-head` repair (or an `[merge] branch` edit)
    // picks up the fix, but within this one attempt the name is frozen.
    let mut retry_warnings = ctx_traits_io::worktree::RetryWarnings::new();
    let (default_branch, default_branch_source) = ctx_traits_io::worktree::resolve_default_branch(
        &repo_root,
        merge_policy.branch.as_deref(),
        &mut retry_warnings,
    )?;
    let mut default_branch_warnings: Vec<String> = Vec::new();
    if matches!(
        default_branch_source,
        ctx_traits_io::worktree::DefaultBranchSource::Fallback
    ) {
        default_branch_warnings.push(DEFAULT_BRANCH_FALLBACK_ADVISORY.to_string());
    }
    let no_wait = !input.force_wait && (input.no_wait || !merge_policy.wait);
    let park_on_overlap = !input.force_land_on_overlap
        && (input.park_on_overlap
            || matches!(
                merge_policy.overlap,
                ctx_traits_io::harness_config::MergeOverlap::Park
            ));

    // Runtime-assignment resolution, merger validation/probing, and the
    // worktree environment-overlay resolution all move to `merge_locked`'s
    // divergent branch (see `resolve_merger`): a true clean fast-forward
    // never needs any of it and must not be blocked by a missing or broken
    // merger configuration it will never use, so nothing here may construct
    // a `ResolvedRuntimeAssignments` before the fast-forward/divergent
    // determination happens inside `merge_locked`.

    // --- Acquire the cross-process merge lock before touching Git state:
    // serializes concurrent `ctx traits merge` invocations across batches so
    // mechanical races queue instead of parking. Held through every
    // subsequent stage via RAII (`_lock_guard` stays alive until this
    // function returns, on every exit path). ---
    let merger_role_budget = resolved_merger_role_budget(&runtime_config.agent, input.deep);
    let merger_timeout_ms = merger_role_budget
        .frame_seconds
        .map(|seconds| seconds.saturating_mul(1_000))
        .unwrap_or(DEFAULT_MERGER_TIMEOUT_MS);
    let wait = if no_wait {
        ctx_traits_io::merge_lock::Wait::NoWait
    } else {
        ctx_traits_io::merge_lock::Wait::Bounded(std::time::Duration::from_millis(
            merge_lock_wait_timeout_ms(&merge_policy, merger_timeout_ms),
        ))
    };
    if let Some(live) = input.live.as_ref() {
        live.emit(&MergeProgress::LockWaiting);
    }
    let (_lock_guard, lock_evidence) =
        match ctx_traits_io::merge_lock::acquire(&repo_root, input.run_id, wait)? {
            ctx_traits_io::merge_lock::AcquireOutcome::Acquired { guard, evidence } => {
                (guard, evidence)
            }
            ctx_traits_io::merge_lock::AcquireOutcome::Busy { holder } => {
                let holder_evidence = holder.as_ref().map(format_holder);
                return Ok(MergeReport {
                    status: "lock-unavailable".to_string(),
                    run_id: input.run_id.to_string(),
                    reason: Some(match &holder_evidence {
                        Some(holder) => format!("{} {holder}", reasons::LOCK_HELD_BY_PREFIX),
                        None => reasons::LOCK_UNAVAILABLE_PREFIX.to_string(),
                    }),
                    lock_wait_ms: Some(0),
                    lock_holder: holder_evidence,
                    lock_stale_holder_reclaimed: None,
                    ..Default::default()
                });
            }
            ctx_traits_io::merge_lock::AcquireOutcome::TimedOut { waited_ms, holder } => {
                let holder_evidence = holder.as_ref().map(format_holder);
                return Ok(MergeReport {
                    status: "lock-timeout".to_string(),
                    run_id: input.run_id.to_string(),
                    reason: Some(format!(
                        "{} {waited_ms}ms waiting for merge lock{}",
                        reasons::LOCK_TIMEOUT_PREFIX,
                        holder_evidence
                            .as_ref()
                            .map(|holder| format!(" held by {holder}"))
                            .unwrap_or_default()
                    )),
                    lock_wait_ms: Some(waited_ms),
                    lock_holder: holder_evidence,
                    lock_stale_holder_reclaimed: None,
                    ..Default::default()
                });
            }
        };
    let stale_holder_evidence = lock_evidence
        .stale_holder_reclaimed
        .as_ref()
        .map(format_holder);
    let mut lock_frame_evidence = vec![format!("wait-ms={}", lock_evidence.wait_ms)];
    if let Some(holder) = lock_evidence.queued_behind.as_ref().map(format_holder) {
        lock_frame_evidence.push(format!("queued-behind={holder}"));
    }
    let lock_frame = MergeFrame {
        stage: MergeStage::Lock,
        status: MergeStatus::LockAcquired,
        reason: stale_holder_evidence
            .as_ref()
            .map(|holder| format!("stale merge-lock holder reclaimed: {holder}")),
        evidence: lock_frame_evidence,
        park_reason: None,
        deep_decisions: Vec::new(),
    };
    if let Some(live) = input.live.as_ref() {
        live.emit(&MergeProgress::LockAcquired);
        live.emit(&MergeProgress::FrameRecorded(&lock_frame));
        live.emit(&MergeProgress::StageEntered(MergeStage::Preflight));
    }
    ctx_traits_io::run_session::append_merge_frame(&session_path, lock_frame)?;

    // --- Every outcome from here on is decorated once, at the bottom of this
    // function, with the lock evidence captured above — no individual
    // park/parked_on_error/recovery_failure call site (or the merged/cleanup-
    // failure reports below) sets its own lock fields; see
    // `decorate_with_lock`. ---
    // Written whenever a stale-base overlap was detected and landed (the
    // default) — see `MergeLockedInputs::overlap_evidence`; a park already
    // sets `MergeReport::stale_base_overlap` directly on its returned report,
    // so this must not unconditionally clobber it below.
    let mut overlap_evidence: Vec<String> = Vec::new();
    // Written whenever the declared `[merge] gate` is empty — see
    // `MergeLockedInputs::gate_warnings` / `EMPTY_GATE_ADVISORY` — so the
    // advisory reaches `MergeReport::warnings` (plain AND `--json` output),
    // not only the persisted `Gates`-frame ledger evidence.
    let mut gate_warnings: Vec<String> = Vec::new();
    let mut confinement_warnings: Vec<String> = Vec::new();
    let mut report = merge_locked(MergeLockedInputs {
        repo_root: &repo_root,
        worktree: &worktree,
        session_path: &session_path,
        session: &session,
        input: &input,
        park_on_overlap,
        gate_policy: &merge_policy,
        default_branch: &default_branch,
        default_branch_source,
        retry_warnings: &mut retry_warnings,
        overlap_evidence: &mut overlap_evidence,
        confinement_warnings: &mut confinement_warnings,
        gate_warnings: &mut gate_warnings,
    })?;
    // P462: every park class (including a standalone `ctx traits merge`
    // with no drive in flight) leaves the worktree parked for triage, but
    // the park promise never covered declared regenerable artifacts — prune
    // both retention tiers here, the single chokepoint downstream of every
    // park-returning exit inside `merge_locked`, since none of those exits
    // (`park_with_detail`, `park_with_deep_decisions`, and the standalone
    // gate/preflight parks) carries the worktree path itself.
    if report.status == "parked"
        && let Some(worktree_path) = worktree
            .path
            .as_deref()
            .map(Utf8Path::new)
            .filter(|path| path.is_dir())
    {
        let declared: Vec<String> = runtime_config
            .worktree
            .retention
            .cheap
            .iter()
            .chain(runtime_config.worktree.retention.expensive.iter())
            .cloned()
            .collect();
        let outcomes =
            ctx_traits_io::retention::prune_terminal_cheap_artifacts(worktree_path, &declared);
        let removed: Vec<_> = outcomes.iter().filter(|outcome| outcome.removed).collect();
        if !removed.is_empty() {
            report.warnings.push(format!(
                "park retention removed declared regenerable artifacts: {}",
                removed
                    .iter()
                    .map(|outcome| outcome.relative_path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for outcome in outcomes
            .into_iter()
            .filter(|outcome| outcome.error.is_some())
        {
            report.warnings.push(format!(
                "park retention cleanup failed for {}: {}",
                outcome.relative_path,
                outcome.error.unwrap_or_default()
            ));
        }
    }
    report.warnings.extend(retry_warnings.into_vec());
    report.warnings.extend(default_branch_warnings);
    report.warnings.extend(confinement_warnings);
    report.warnings.extend(gate_warnings);
    if input.allow_stale_overlap {
        // Deprecated no-op (P401): overlap landing is now the default, so
        // this flag no longer changes behavior at all — surfaced unconditionally
        // whenever it is passed, independent of whether an overlap was
        // actually detected this run.
        report.warnings.push(
            "--allow-stale-overlap is deprecated and now a no-op: landing despite a stale-base \
             overlap is the default; pass --park-on-overlap to restore the old strict behavior"
                .to_string(),
        );
    }
    if report.stale_base_overlap.is_empty() {
        report.stale_base_overlap = overlap_evidence;
    }
    Ok(decorate_with_lock(report, &lock_evidence))
}

/// Bundled arguments for [`merge_locked`] — kept as a struct rather than
/// individual parameters purely to stay under the arity lint.
struct MergeLockedInputs<'a> {
    repo_root: &'a Utf8Path,
    worktree: &'a WorktreeProvenance,
    session_path: &'a Utf8Path,
    session: &'a Session,
    input: &'a MergeInputs<'a>,
    park_on_overlap: bool,
    /// P477: the resolved `[merge] gate`/`gate-seconds` this landing path
    /// runs its pre-landing gate under — resolved once in `merge` and
    /// reused here so a paused/resumed lifecycle never re-resolves config
    /// mid-flight.
    gate_policy: &'a ctx_traits_io::harness_config::EffectiveMergePolicy,
    /// P488: the resolved landing/default branch name for this merge
    /// attempt, resolved exactly once by `merge` before the lock is
    /// acquired.
    default_branch: &'a str,
    /// P488: the source layer that produced `default_branch` (config,
    /// `origin/HEAD`, `init.defaultBranch`, or the literal fallback), named
    /// in refusal text.
    default_branch_source: ctx_traits_io::worktree::DefaultBranchSource,
    /// Single retry-evidence collector for this merge lifecycle's worktree
    /// Git calls, decorated onto the final [`MergeReport`] by [`merge`] after
    /// `merge_locked` returns — every internal return path in this function
    /// accumulates into it without threading it back out itself.
    retry_warnings: &'a mut ctx_traits_io::worktree::RetryWarnings,
    /// Set once, whenever a detected stale-base overlap was landed (the
    /// default): the same overlap evidence a park would have carried.
    /// Decorated onto the final [`MergeReport`] by [`merge`] the same way as
    /// `retry_warnings`, so it reaches the CLI report regardless of which
    /// later stage terminates the merge.
    overlap_evidence: &'a mut Vec<String>,
    /// P478: set whenever the resolved merger's harness kind has no
    /// write-confinement renderer, so a worktree merge dispatch that runs
    /// with zero write confinement is reported rather than silent — see
    /// [`ctx_traits_io::confinement::confinement_unsupported_capability`].
    /// Decorated onto the final [`MergeReport`] by [`merge`] the same way as
    /// `retry_warnings`/`overlap_evidence`.
    confinement_warnings: &'a mut Vec<String>,
    /// P477: set to [`EMPTY_GATE_ADVISORY`] whenever the declared `[merge]
    /// gate` is empty. Decorated onto the final [`MergeReport`] by [`merge`]
    /// the same way as `retry_warnings`/`overlap_evidence`, so the advisory
    /// is user-visible in plain AND `--json` output, not only in the
    /// persisted `Gates`-frame ledger evidence.
    gate_warnings: &'a mut Vec<String>,
}

/// Decorate a `merge_locked` result with the operational lock evidence
/// captured once, at acquisition — the single point every post-acquisition
/// outcome (parked, parked-on-error, recovery-failure, merged, post-merge
/// cleanup failure) is decorated, instead of threading or copying the same
/// three fields through every park helper and return site individually.
fn decorate_with_lock(
    report: MergeReport,
    evidence: &ctx_traits_io::merge_lock::LockEvidence,
) -> MergeReport {
    MergeReport {
        lock_wait_ms: Some(evidence.wait_ms),
        lock_holder: evidence.queued_behind.as_ref().map(format_holder),
        lock_stale_holder_reclaimed: evidence.stale_holder_reclaimed.as_ref().map(format_holder),
        ..report
    }
}

/// Everything from preflight through cleanup, run while the merge lock is
/// held. Split out of `merge` purely so its single `crate::Result<MergeReport>`
/// can be decorated with lock evidence in exactly one place; every internal
/// `return` in this function is unaffected and keeps its existing meaning.
fn merge_locked(args: MergeLockedInputs<'_>) -> crate::Result<MergeReport> {
    let MergeLockedInputs {
        repo_root,
        worktree,
        session_path,
        session,
        input,
        park_on_overlap,
        gate_policy,
        default_branch,
        default_branch_source,
        retry_warnings,
        overlap_evidence,
        confinement_warnings,
        gate_warnings,
    } = args;

    // --- Preflight: clean main, registered clean worktree, no rebase in progress. ---
    let worktree_path = match preflight(
        repo_root,
        worktree,
        default_branch,
        default_branch_source,
        retry_warnings,
    ) {
        Ok(path) => path,
        Err((reason, detail)) => {
            return park_with_detail(
                ParkTarget {
                    session_path,
                    run_id: input.run_id,
                },
                MergeStage::Preflight,
                reason,
                detail,
                Vec::new(),
                retry_warnings,
                input.live.as_ref(),
            );
        }
    };

    // P479: a `policy = "park"` out-of-tree-mutation finding is permanent —
    // nothing clears it — so it refuses landing here, before the branch or
    // worktree is touched, on every later merge attempt of this run-id. The
    // preflight clean-`main` check above does NOT catch this incident class,
    // since the P477 write it exists to cover landed in gitignored
    // `.ctx/config.toml`, leaving the tracked tree clean.
    if let Some(finding) = session
        .provenance
        .out_of_tree_mutations
        .iter()
        .rev()
        .find(|finding| {
            ctx_traits_io::tripwire::TripwirePolicy::Park.matches_persisted(&finding.policy)
        })
    {
        return park_out_of_tree_mutation(session_path, input.run_id, finding, input.live.as_ref());
    }

    let main_rev = ctx_traits_io::worktree::rev_parse(repo_root, default_branch, retry_warnings)?;
    let branch_rev = ctx_traits_io::worktree::rev_parse(&worktree_path, "HEAD", retry_warnings)?;
    let revision_evidence = vec![
        format!("target={default_branch}@{main_rev}"),
        format!("branch={}@{branch_rev}", worktree.branch),
    ];

    // --- Fast-path selection: resolve `main`'s merge base with the branch
    // once, before any merger-related lookup. A true clean fast-forward
    // (`main` itself is that base) needs no rebase, no stale-base overlap
    // check (main cannot have changed since a base that equals it), and no
    // merger resolution/probing/dispatch — `--force-merger` deliberately
    // routes such a case through the merger anyway (operator inspection, and
    // the deterministic differential proof against this fast path). Every
    // other case (a divergent history) always uses the merger. ---
    let base_rev = match ctx_traits_io::worktree::merge_base(
        repo_root,
        &main_rev,
        &branch_rev,
        retry_warnings,
    ) {
        Ok(rev) => rev,
        Err(error) => {
            return parked_on_error(
                &worktree_path,
                ParkTarget {
                    session_path,
                    run_id: input.run_id,
                },
                MergeStage::Rebase,
                revision_evidence,
                error.into(),
                retry_warnings,
                input.live.as_ref(),
            );
        }
    };
    let is_true_fast_forward = base_rev == main_rev;

    let (landed_rev, merger_evidence_tag) = if is_true_fast_forward && !input.force_merger {
        // --- Fast path: no assignment/merger resolution, no rebase. Nothing
        // above this point has constructed a `ResolvedRuntimeAssignments` or
        // touched `[agent.role.merger]` at all, so an invalid/unprobeable
        // merger configuration can never block a true clean fast-forward. ---
        (
            branch_rev.clone(),
            "merger=skipped-clean-fast-forward".to_string(),
        )
    } else {
        // --- Divergent path (or a forced merger confirmation): resolve the
        // runtime assignments and the merger completely — including harness
        // probing — before any overlap-related mutation or `git rebase`, so a
        // broken merger configuration fails here, with nothing yet mutated,
        // using exactly the prior no-mutation failure behavior (a plain
        // propagated error, not a park). ---
        let mut profile =
            ctx_traits_io::harness_config::resolve_runtime_assignments(input.assignments)?;
        let merger = resolve_merger(&mut profile, input.deep, &worktree_path)?;
        // P478/P480: a worktree merger spawn must not silently run with less
        // write confinement than declared — a harness kind with no renderer,
        // or a requested OS-level sandbox that is unavailable here.
        if let Some(payloads) = merger.confinement.as_ref() {
            let spawn_sandbox_applied = payloads.spawn_sandbox.is_some();
            for capability in [
                ctx_traits_io::confinement::confinement_unsupported_capability(
                    merger.harness.kind(),
                    spawn_sandbox_applied,
                ),
                ctx_traits_io::confinement::spawn_sandbox_unsupported_capability(
                    payloads.sandbox_requested,
                    payloads.spawn_sandbox.as_ref(),
                ),
            ]
            .into_iter()
            .flatten()
            {
                confinement_warnings.push(format!(
                    "{}: {}",
                    capability.capability,
                    capability.reason.unwrap_or_default()
                ));
            }
        }
        let merger_tag = format!(
            "merger={}@{}{}",
            merger.harness_id,
            merger.assignment.model.as_deref().unwrap_or("-"),
            if merger.deep { " deep=true" } else { "" }
        );
        if let Some(live) = input.live.as_ref() {
            live.emit(&MergeProgress::StageEntered(MergeStage::Rebase));
        }

        // --- Full stale-base-overlap check, rebase, and merger reconciliation
        // loop, exactly as before this fast path existed. ---
        let mut revision_evidence = revision_evidence;

        // --- Stale-base overlap detection: compare paths the run branch
        // changed since its base against paths main changed since that same
        // base, before rebasing — a successful rebase moves the branch onto
        // main and erases the base this comparison needs, so retrying after a
        // rebase could no longer detect what a retry before one still can.
        // Disjoint work is unaffected. By default an overlap lands anyway
        // (Git conflict handling, and if needed the merger, decide whether
        // reconciliation is required) with the same evidence a park would
        // have carried; `--park-on-overlap` restores the strict prior
        // behavior of parking here instead. ---
        let branch_changed = match ctx_traits_io::worktree::changed_paths(
            repo_root,
            &base_rev,
            &branch_rev,
            retry_warnings,
        ) {
            Ok(paths) => paths,
            Err(error) => {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Rebase,
                    revision_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };
        let main_changed = match ctx_traits_io::worktree::changed_paths(
            repo_root,
            &base_rev,
            &main_rev,
            retry_warnings,
        ) {
            Ok(paths) => paths,
            Err(error) => {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Rebase,
                    revision_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };
        let overlap: Vec<String> = branch_changed
            .iter()
            .filter(|path| main_changed.contains(path))
            .cloned()
            .collect();
        if !overlap.is_empty() {
            let main_commits = match ctx_traits_io::worktree::commits_touching_paths(
                repo_root,
                &base_rev,
                &main_rev,
                &overlap,
                retry_warnings,
            ) {
                Ok(commits) => commits,
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Rebase,
                        revision_evidence,
                        error.into(),
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            };
            if park_on_overlap {
                return park_stale_base_overlap(
                    session_path,
                    input.run_id,
                    &overlap,
                    &main_commits,
                    revision_evidence,
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
            *overlap_evidence = stale_base_overlap_evidence(&overlap, &main_commits);
            revision_evidence.extend(overlap_evidence.iter().cloned());
        }

        // --- Rebase the run branch onto main. ---
        let rebase_outcome = match ctx_traits_io::worktree::rebase(
            &worktree_path,
            default_branch,
            retry_warnings,
            resolve_git_long_timeout_ms(),
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Rebase,
                    revision_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };
        let mut conflicted_paths = match &rebase_outcome {
            ctx_traits_io::worktree::RebaseOutcome::Clean => Vec::new(),
            ctx_traits_io::worktree::RebaseOutcome::Conflicts { paths } => paths.clone(),
            ctx_traits_io::worktree::RebaseOutcome::Failed { message } => {
                if let Some(result) = ensure_rebase_aborted(
                    &worktree_path,
                    session_path,
                    input.run_id,
                    MergeStage::Rebase,
                    &revision_evidence,
                    retry_warnings,
                    input.live.as_ref(),
                ) {
                    return result;
                }
                return park_with_detail(
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Rebase,
                    reasons::REBASE_FAILED.to_string(),
                    Some(message.clone()),
                    revision_evidence,
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };

        // The merger (and its harness probe) was already fully resolved above,
        // before this overlap check and the rebase below — a missing or
        // invalid merger configuration never reaches this point at all.
        let mut merger_evidence = revision_evidence.clone();
        merger_evidence.push(merger_tag.clone());

        // --- Invoke the merger: confirmation on a clean rebase, mechanical
        // reconciliation on conflicts. Never redesign or re-review. A conflicted
        // rebase loops call -> `rebase --continue`, since the replay of a later
        // commit can itself conflict and need another reconciliation pass. Any
        // unexpected IO/harness failure along the way parks rather than
        // propagating past this function with the rebase left dangling. A hard
        // iteration cap and a same-commit/same-conflicted-paths no-progress check
        // (see below) both guard against a misbehaving merger that replies
        // `proceed` but stages nothing: without either, `continue_rebase` would
        // keep reporting the same conflicts forever, burning a paid harness call
        // every pass.
        // Accumulated across every reconciliation-loop iteration: the typed
        // per-hunk decision log a P420 `--deep` merger produces, empty for
        // the standard mechanical-only merger. Written to one `Reconciliation`
        // ledger frame and the amended branch tip's trailers once the loop
        // lands (see below); never touched on a park.
        let mut accumulated_deep_decisions: Vec<DeepMergeDecision> = Vec::new();
        let mut iterations: u32 = 0;
        // P463 item B: names every merger transcript for this ONE `ctx
        // traits merge` invocation — distinct from a later, separate
        // invocation retrying the same run-id, whose transcripts must not
        // collide with (and `File::create`-truncate) this one's on disk.
        let merge_item_id = format!("merge-{}", epoch_seconds());
        let mut trace_sequence: u64 = 0;
        // P463 item C: every conflicted path a declared `[[merge.generated]]`
        // entry matches, deferred (never sent to the merger) across every
        // reconciliation-loop iteration this merge makes — used once the
        // rebase has fully replayed to run each hit entry's `rebuild`
        // commands and unconditionally re-stage its declared paths.
        let mut deferred_generated_paths: Vec<String> = Vec::new();
        if let Some(live) = input.live.as_ref() {
            live.emit(&MergeProgress::StageEntered(MergeStage::Reconciliation));
        }
        loop {
            iterations += 1;
            if iterations > MAX_RECONCILIATION_ITERATIONS {
                if let Some(result) = ensure_rebase_aborted(
                    &worktree_path,
                    session_path,
                    input.run_id,
                    MergeStage::Reconciliation,
                    &merger_evidence,
                    retry_warnings,
                    input.live.as_ref(),
                ) {
                    return result;
                }
                return park(
                    session_path,
                    input.run_id,
                    MergeStage::Reconciliation,
                    format!(
                        "{} {MAX_RECONCILIATION_ITERATIONS} merger iterations without landing",
                        reasons::RECONCILIATION_ITERATION_CAP_PREFIX
                    ),
                    merger_evidence,
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
            // P463 item C: resolve every declared-generated conflicted path
            // mechanically, BEFORE the merger call — so it never sees them
            // (the deep merger's fingerprint capture inside `dispatch_merger`
            // starts from `conflicted_paths` already staged/resolved here,
            // and the standard/deep prompts below are built from the
            // narrowed source-only list).
            let mut source_conflicted_paths: Vec<String> = Vec::new();
            for path in &conflicted_paths {
                match generated_artifact_index_for_path(&gate_policy.generated, path) {
                    Some(_) => {
                        if let Err(error) = ctx_traits_io::worktree::resolve_generated_conflict(
                            &worktree_path,
                            path,
                            retry_warnings,
                        ) {
                            return parked_on_error(
                                &worktree_path,
                                ParkTarget {
                                    session_path,
                                    run_id: input.run_id,
                                },
                                MergeStage::Reconciliation,
                                merger_evidence,
                                error.into(),
                                retry_warnings,
                                input.live.as_ref(),
                            );
                        }
                        merger_evidence.push(format!("generated-deferred={path}"));
                        deferred_generated_paths.push(path.clone());
                    }
                    None => source_conflicted_paths.push(path.clone()),
                }
            }
            let (decision, call_evidence) = match dispatch_merger(MergerCall {
                harness: &merger.harness,
                harness_id: &merger.harness_id,
                cli: &merger.cli,
                assignment: &merger.assignment,
                session,
                worktree,
                worktree_path: &worktree_path,
                conflicted_paths: &source_conflicted_paths,
                deferred_generated_paths: &deferred_generated_paths,
                harvest_conflicts: None,
                revision_evidence: &revision_evidence,
                env_overlay: &merger.env_overlay,
                confinement: merger.confinement.as_ref(),
                deep: merger.deep,
                repo_root,
                base_rev: &base_rev,
                main_rev: &main_rev,
                default_branch,
                item_id: &merge_item_id,
                trace_sequence: &mut trace_sequence,
                live: input.live.as_ref(),
                merger_stdout_observer: input.merger_stdout_observer.as_ref(),
            }) {
                Ok(pair) => pair,
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        merger_evidence,
                        error,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            };
            // Every merger call's transcript/harness-session-id/retry
            // evidence is recorded on the shared `merger_evidence` — the one
            // vector every terminal frame (park or merged) in this function
            // is built from — so a park still carries this call's
            // diagnostics, not only a landed outcome.
            merger_evidence.extend(call_evidence);
            let deep_decisions = match decision {
                MergerDecision::Park {
                    reason,
                    deep_decisions,
                } => {
                    if let Some(result) = ensure_rebase_aborted(
                        &worktree_path,
                        session_path,
                        input.run_id,
                        MergeStage::Reconciliation,
                        &merger_evidence,
                        retry_warnings,
                        input.live.as_ref(),
                    ) {
                        return result;
                    }
                    return park_with_deep_decisions(
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        reason,
                        merger_evidence,
                        deep_decisions,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
                MergerDecision::Proceed { deep_decisions } => deep_decisions,
            };
            accumulated_deep_decisions.extend(deep_decisions);
            if conflicted_paths.is_empty() {
                // Confirmation call on an already clean rebase: nothing left to
                // continue.
                break;
            }
            // Progress is judged by whether the replay actually advanced to a new
            // commit (`REBASE_HEAD` changed), not merely by whether the
            // conflicted path set shrank: a later commit in the replay can
            // legitimately conflict on the same or a superset of the paths the
            // prior commit did, so a superset-of-paths check would reject valid
            // multi-commit reconciliation. Only when the replay is still on the
            // same commit AND the conflicted paths are exactly unchanged has
            // `continue_rebase` genuinely made no progress.
            let pre_continue_head = ctx_traits_io::worktree::rebase_head(&worktree_path);
            let pre_continue_paths = conflicted_paths.clone();
            match ctx_traits_io::worktree::continue_rebase(
                &worktree_path,
                retry_warnings,
                resolve_git_long_timeout_ms(),
            ) {
                Ok(ctx_traits_io::worktree::RebaseOutcome::Clean) => break,
                Ok(ctx_traits_io::worktree::RebaseOutcome::Conflicts { paths }) => {
                    let post_continue_head = ctx_traits_io::worktree::rebase_head(&worktree_path);
                    let same_commit = matches!(
                        (&pre_continue_head, &post_continue_head),
                        (Ok(pre), Ok(post)) if pre == post
                    );
                    if same_commit && paths == pre_continue_paths {
                        if let Some(result) = ensure_rebase_aborted(
                            &worktree_path,
                            session_path,
                            input.run_id,
                            MergeStage::Reconciliation,
                            &merger_evidence,
                            retry_warnings,
                            input.live.as_ref(),
                        ) {
                            return result;
                        }
                        return park(
                            session_path,
                            input.run_id,
                            MergeStage::Reconciliation,
                            format!(
                                "{}: same replayed commit and conflicted paths unchanged after a continue attempt",
                                reasons::RECONCILIATION_NO_PROGRESS_PREFIX
                            ),
                            merger_evidence,
                            retry_warnings,
                            input.live.as_ref(),
                        );
                    }
                    conflicted_paths = paths;
                }
                Ok(ctx_traits_io::worktree::RebaseOutcome::Failed { message }) => {
                    if let Some(result) = ensure_rebase_aborted(
                        &worktree_path,
                        session_path,
                        input.run_id,
                        MergeStage::Reconciliation,
                        &merger_evidence,
                        retry_warnings,
                        input.live.as_ref(),
                    ) {
                        return result;
                    }
                    return park_with_detail(
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        reasons::REBASE_CONTINUE_FAILED.to_string(),
                        Some(message),
                        merger_evidence,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        merger_evidence,
                        error.into(),
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            }
        }

        // Reconciliation completed (or was never needed): the worktree must now
        // be conflict-free, clean, and no longer mid-rebase.
        let post_state = (|| -> crate::Result<(Vec<String>, bool, bool)> {
            Ok((
                ctx_traits_io::worktree::conflicted_paths(&worktree_path)?,
                ctx_traits_io::worktree::is_clean(&worktree_path)?,
                ctx_traits_io::worktree::rebase_in_progress(&worktree_path)?,
            ))
        })();
        let (remaining_conflicts, worktree_clean, still_rebasing) = match post_state {
            Ok(values) => values,
            Err(error) => {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Reconciliation,
                    merger_evidence,
                    error,
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };
        if !remaining_conflicts.is_empty() || !worktree_clean || still_rebasing {
            if let Some(result) = ensure_rebase_aborted(
                &worktree_path,
                session_path,
                input.run_id,
                MergeStage::Reconciliation,
                &merger_evidence,
                retry_warnings,
                input.live.as_ref(),
            ) {
                return result;
            }
            return park(
                session_path,
                input.run_id,
                MergeStage::Reconciliation,
                reasons::WORKTREE_DIRTY_AFTER_RECONCILIATION.to_string(),
                merger_evidence,
                retry_warnings,
                input.live.as_ref(),
            );
        }

        // P463 item C: once the rebase has fully replayed and the worktree is
        // conflict-free, run every HIT `[[merge.generated]]` declaration's
        // `rebuild` commands and unconditionally restage its declared paths
        // — sequenced BEFORE the (separate, optional) `--deep` trailer amend
        // below, so the two collapse to one tip and `landed_rev` (read after
        // both) reflects the regenerated bytes. Unconditional restaging is
        // what makes `resolve_generated_conflict`'s upstream side choice
        // above harmless: the rebuild step's own output is what actually
        // lands, regardless of which side was mechanically checked out.
        deferred_generated_paths.sort();
        deferred_generated_paths.dedup();
        let hit_generated: Vec<usize> = gate_policy
            .generated
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.paths.iter().any(|declared| {
                    deferred_generated_paths.iter().any(|deferred| {
                        deferred == declared
                            || deferred.starts_with(&format!("{}/", declared.trim_end_matches('/')))
                    })
                })
            })
            .map(|(index, _)| index)
            .collect();
        if !hit_generated.is_empty() {
            let env_overlay = match resolve_worktree_env_for_gate(&worktree_path) {
                Ok(overlay) => overlay,
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        merger_evidence,
                        error,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            };
            let timeout_ms = gate_policy.gate_seconds.saturating_mul(1_000);
            for entry_index in hit_generated {
                let entry = &gate_policy.generated[entry_index];
                for (command_index, argv) in entry.rebuild.iter().enumerate() {
                    let label = format!(
                        "rebuild-{entry_index}-{}",
                        gate_command_label(command_index, argv)
                    );
                    let run = match run_declared_command(
                        &worktree_path,
                        &env_overlay,
                        input.run_id,
                        &label,
                        argv,
                        timeout_ms,
                    ) {
                        Ok(run) => run,
                        Err(error) => {
                            return parked_on_error(
                                &worktree_path,
                                ParkTarget {
                                    session_path,
                                    run_id: input.run_id,
                                },
                                MergeStage::Reconciliation,
                                merger_evidence,
                                error,
                                retry_warnings,
                                input.live.as_ref(),
                            );
                        }
                    };
                    merger_evidence.push(format!(
                        "rebuild={argv:?} exit={:?} timed-out={}",
                        run.outcome.exit_code, run.outcome.timed_out
                    ));
                    if !run.outcome.success
                        || run.outcome.timed_out
                        || run.outcome.stdout_truncated
                        || run.outcome.stderr_truncated
                    {
                        if let Some(capture_path) = run.capture_path {
                            merger_evidence.push(format!("rebuild-output={capture_path}"));
                        }
                        if let Some(result) = ensure_rebase_aborted(
                            &worktree_path,
                            session_path,
                            input.run_id,
                            MergeStage::Reconciliation,
                            &merger_evidence,
                            retry_warnings,
                            input.live.as_ref(),
                        ) {
                            return result;
                        }
                        return park(
                            session_path,
                            input.run_id,
                            MergeStage::Reconciliation,
                            format!(
                                "{}: {argv:?} exit={:?}",
                                reasons::GENERATED_ARTIFACT_REBUILD_FAILED_PREFIX,
                                run.outcome.exit_code
                            ),
                            merger_evidence,
                            retry_warnings,
                            input.live.as_ref(),
                        );
                    }
                }
                for path in &entry.paths {
                    if let Err(error) =
                        ctx_traits_io::worktree::stage_path(&worktree_path, path, retry_warnings)
                    {
                        return parked_on_error(
                            &worktree_path,
                            ParkTarget {
                                session_path,
                                run_id: input.run_id,
                            },
                            MergeStage::Reconciliation,
                            merger_evidence,
                            error.into(),
                            retry_warnings,
                            input.live.as_ref(),
                        );
                    }
                    merger_evidence.push(format!("regenerated={path}"));
                }
            }
            if let Err(error) = ctx_traits_io::worktree::amend_head(&worktree_path, retry_warnings)
            {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Reconciliation,
                    merger_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        }

        // P420 `--deep`: record the accumulated typed per-hunk decision log on
        // one `Reconciliation` frame and amend it onto the branch tip as
        // repeatable `Ctx-Merge-Decision` trailers, before capturing the
        // revision that lands below — the trailer amendment produces a new
        // commit, so `landed_rev` must be read after it, not before. Standard
        // no-flag reconciliation never reaches here with any decisions, so its
        // ledger/branch bytes are unaffected.
        if !accumulated_deep_decisions.is_empty() {
            let reconciled_frame = MergeFrame {
                stage: MergeStage::Reconciliation,
                status: MergeStatus::Reconciled,
                reason: None,
                evidence: with_retry_evidence(merger_evidence.clone(), retry_warnings),
                park_reason: None,
                deep_decisions: accumulated_deep_decisions.clone(),
            };
            if let Some(live) = input.live.as_ref() {
                live.emit(&MergeProgress::FrameRecorded(&reconciled_frame));
            }
            ctx_traits_io::run_session::append_merge_frame(session_path, reconciled_frame)?;
            let trailers: Vec<String> = accumulated_deep_decisions
                .iter()
                .map(|decision| {
                    format!(
                        "Ctx-Merge-Decision={}",
                        deep_decision_trailer_value(decision)
                    )
                })
                .collect();
            if let Err(error) = ctx_traits_io::worktree::amend_head_with_trailers(
                &worktree_path,
                &trailers,
                retry_warnings,
            ) {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Reconciliation,
                    merger_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        }

        // The rebase has now fully replayed onto main (and, for `--deep` with
        // recorded decisions, just been amended with trailers): the revision
        // that will land is the post-rebase HEAD, not the pre-rebase
        // `branch_rev` — those differ whenever main had actually advanced,
        // which is the headline scenario this command exists for.
        let landed_rev =
            match ctx_traits_io::worktree::rev_parse(&worktree_path, "HEAD", retry_warnings) {
                Ok(rev) => rev,
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Reconciliation,
                        merger_evidence,
                        error.into(),
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            };
        (landed_rev, merger_tag)
    };

    // --- Single shared terminal-landing-evidence construction for both
    // strategies: always `landed=<rev>` first, then overlap evidence (empty
    // for the fast path, which never reaches the overlap check), then the
    // merger-strategy evidence. A fast and a forced-merger landing of the
    // same graph therefore carry byte-identical evidence apart from this last
    // entry — see `scripts/byte_compare.rs`'s differential proof. ---
    let mut landed_evidence = vec![format!("landed={landed_rev}")];
    landed_evidence.extend(overlap_evidence.iter().cloned());
    landed_evidence.push(merger_evidence_tag);

    // --- Declared pre-landing gate (P477): every landing path — clean
    // fast-forward, standard no-flag reconciliation, and `--deep` alike —
    // runs the effective `[merge] gate`'s ordered commands, in order, before
    // `main` is ever touched. The product default is an empty gate: no
    // repository command runs, and one advisory is recorded instead. A red
    // gate parks here with nothing yet landed. ---
    if let Some(result) = run_landing_gate(
        &LandingGateContext {
            worktree_path: &worktree_path,
            session_path,
            run_id: input.run_id,
            live: input.live.as_ref(),
        },
        &landed_evidence,
        gate_policy,
        retry_warnings,
        gate_warnings,
    ) {
        return result;
    }

    // --- Seed harvest planning: build the plan for every declared seed root
    // now — after reconciliation (or the fast path) but before `main` is
    // touched — so a real conflict, or an unavailable seed-time ancestor for
    // a legacy worktree, parks before any fast-forward instead of after one.
    // Gitignored seed roots (e.g. `.docs`, `.plans`) copied into the worktree
    // at prepare time are otherwise destroyed by `remove_worktree` below,
    // silently losing any work a phase authored there. ---
    let mut harvest_plan = match ctx_traits_io::worktree::plan_harvest_seeds(
        repo_root,
        &worktree_path,
        &worktree.seed_snapshots,
        default_branch,
        retry_warnings,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            return parked_on_error(
                &worktree_path,
                ParkTarget {
                    session_path,
                    run_id: input.run_id,
                },
                MergeStage::Cleanup,
                landed_evidence,
                error.into(),
                retry_warnings,
                input.live.as_ref(),
            );
        }
    };
    if !harvest_plan.conflicts.is_empty() {
        let ancestor_unavailable = harvest_plan.conflicts.iter().any(|conflict| {
            matches!(
                conflict.reason,
                ctx_traits_io::worktree::SeedConflictReason::AncestorUnavailable
            )
        });
        let ancestor_digest_mismatch = harvest_plan.conflicts.iter().any(|conflict| {
            matches!(
                conflict.reason,
                ctx_traits_io::worktree::SeedConflictReason::AncestorDigestMismatch
            )
        });
        // P463 item A: only an all-`Overlap` conflict set is adjudicable —
        // `AncestorUnavailable`/`AncestorDigestMismatch` fail closed exactly
        // as without `--deep` (no honest three-way input to hand a model in
        // either case), and a mixed set parks entirely rather than
        // adjudicating a partial subset (mirrors `plan_harvest_seeds`'s own
        // all-or-nothing conflict invariant).
        if input.deep && !ancestor_unavailable && !ancestor_digest_mismatch {
            match adjudicate_harvest_conflicts(HarvestAdjudicationInputs {
                repo_root,
                worktree_path: &worktree_path,
                session,
                worktree,
                conflicts: &harvest_plan.conflicts,
                input,
                default_branch,
                retry_warnings,
            }) {
                Ok(HarvestAdjudicationOutcome::Proceed {
                    copies,
                    decisions,
                    evidence,
                }) => {
                    let mut adjudication_evidence = landed_evidence.clone();
                    for conflict in &harvest_plan.conflicts {
                        adjudication_evidence
                            .push(format!("harvest-adjudication-path={}", conflict.relative));
                    }
                    adjudication_evidence.extend(evidence);
                    let harvest_frame = MergeFrame {
                        stage: MergeStage::Cleanup,
                        status: MergeStatus::Reconciled,
                        reason: None,
                        evidence: with_retry_evidence(adjudication_evidence, retry_warnings),
                        park_reason: None,
                        deep_decisions: decisions,
                    };
                    if let Some(live) = input.live.as_ref() {
                        live.emit(&MergeProgress::FrameRecorded(&harvest_frame));
                    }
                    ctx_traits_io::run_session::append_merge_frame(session_path, harvest_frame)?;
                    // `plan_harvest_seeds` already populated `harvest_plan.copies`
                    // with every OTHER changed seeded path that resolved
                    // cleanly (see its doc) — that set must land alongside the
                    // adjudicated conflicts' resolutions, never be replaced by
                    // them, or every seeded path that merely didn't conflict
                    // would be silently dropped from the applied plan and then
                    // destroyed by `remove_worktree` below.
                    harvest_plan.copies.extend(copies);
                    harvest_plan
                        .copies
                        .sort_by(|left, right| left.relative.cmp(&right.relative));
                    harvest_plan.conflicts = Vec::new();
                }
                Ok(HarvestAdjudicationOutcome::Park {
                    reason,
                    decisions,
                    evidence,
                }) => {
                    let mut park_evidence = landed_evidence.clone();
                    for conflict in &harvest_plan.conflicts {
                        park_evidence.push(format!("harvest-conflict-path={}", conflict.relative));
                    }
                    park_evidence.extend(evidence);
                    return park_with_deep_decisions(
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Cleanup,
                        format!("{} {reason}", reasons::HARVEST_ADJUDICATION_PARKED_PREFIX),
                        park_evidence,
                        decisions,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
                Err(error) => {
                    return parked_on_error(
                        &worktree_path,
                        ParkTarget {
                            session_path,
                            run_id: input.run_id,
                        },
                        MergeStage::Cleanup,
                        landed_evidence,
                        error,
                        retry_warnings,
                        input.live.as_ref(),
                    );
                }
            }
        } else {
            let reason = if ancestor_digest_mismatch {
                reasons::HARVEST_DIGEST_MISMATCH.to_string()
            } else if ancestor_unavailable {
                reasons::HARVEST_ANCESTOR_UNAVAILABLE.to_string()
            } else {
                reasons::HARVEST_CHANGED_BOTH_SIDES.to_string()
            };
            let mut evidence = landed_evidence.clone();
            for conflict in &harvest_plan.conflicts {
                evidence.push(format!("harvest-conflict-path={}", conflict.relative));
                evidence.push(format!(
                    "harvest-conflict-lines={}-{}",
                    conflict.start_line, conflict.end_line
                ));
            }
            let detail = harvest_plan
                .conflicts
                .iter()
                .map(|conflict| {
                    format!(
                        "{} (lines {}-{})",
                        conflict.relative, conflict.start_line, conflict.end_line
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return park_with_detail(
                ParkTarget {
                    session_path,
                    run_id: input.run_id,
                },
                MergeStage::Cleanup,
                reason,
                Some(detail),
                evidence,
                retry_warnings,
                input.live.as_ref(),
            );
        }
    }

    // --- Land: re-read main's revision and cleanliness immediately before
    // touching it. Reconciliation can run for minutes; a lost fast-forward
    // race, or a human dirtying main in the meantime without changing its
    // revision, both park for retry rather than merging over unexpected
    // state. ---
    if let Some(live) = input.live.as_ref() {
        live.emit(&MergeProgress::StageEntered(MergeStage::Landing));
    }
    let current_main_rev =
        match ctx_traits_io::worktree::rev_parse(repo_root, default_branch, retry_warnings) {
            Ok(rev) => rev,
            Err(error) => {
                return parked_on_error(
                    &worktree_path,
                    ParkTarget {
                        session_path,
                        run_id: input.run_id,
                    },
                    MergeStage::Landing,
                    landed_evidence,
                    error.into(),
                    retry_warnings,
                    input.live.as_ref(),
                );
            }
        };
    if current_main_rev != main_rev {
        return park(
            session_path,
            input.run_id,
            MergeStage::Landing,
            format!(
                "{default_branch} {} {main_rev} to {current_main_rev} since the rebase revision was captured; retry the merge",
                reasons::MAIN_ADVANCED_INFIX
            ),
            vec![
                format!("{default_branch}-at-capture={main_rev}"),
                format!("{default_branch}-now={current_main_rev}"),
            ],
            retry_warnings,
            input.live.as_ref(),
        );
    }
    match ctx_traits_io::worktree::is_clean_for_landing(repo_root) {
        Ok(true) => {}
        Ok(false) => {
            return park(
                session_path,
                input.run_id,
                MergeStage::Landing,
                format!(
                    "{default_branch} {}; retry the merge",
                    reasons::MAIN_NOT_CLEAN_INFIX
                ),
                vec![format!("{default_branch}-at-capture={main_rev}")],
                retry_warnings,
                input.live.as_ref(),
            );
        }
        Err(error) => {
            return parked_on_error(
                &worktree_path,
                ParkTarget {
                    session_path,
                    run_id: input.run_id,
                },
                MergeStage::Landing,
                landed_evidence,
                error.into(),
                retry_warnings,
                input.live.as_ref(),
            );
        }
    }
    if let Err(error) =
        ctx_traits_io::worktree::fast_forward_merge(repo_root, &worktree.branch, retry_warnings)
    {
        return park_with_detail(
            ParkTarget {
                session_path,
                run_id: input.run_id,
            },
            MergeStage::Landing,
            reasons::FAST_FORWARD_FAILED.to_string(),
            Some(error.to_string()),
            landed_evidence,
            retry_warnings,
            input.live.as_ref(),
        );
    }

    // --- Cleanup: a failure here is a distinct terminal outcome — main has
    // already moved, so this must never be reported as parked. The ledger
    // frame's reason stays a stable code (`landed_rev` is a git revision, not
    // a path); the raw Git/filesystem error is surfaced only in the returned
    // CLI-facing report, never persisted. Applying the already-conflict-free
    // harvest plan rechecks each destination's digest immediately before
    // writing, so a `main`-side race after planning surfaces here — as a
    // cleanup failure, never as a park, since the fast-forward above has
    // already happened. ---
    if let Some(live) = input.live.as_ref() {
        live.emit(&MergeProgress::StageEntered(MergeStage::Cleanup));
    }
    let cleanup_failure =
        match ctx_traits_io::worktree::apply_harvest_plan(repo_root, &harvest_plan) {
            Err(error) => Some(("seed harvest apply failed", error.to_string())),
            Ok(()) => {
                match ctx_traits_io::worktree::remove_worktree(
                    repo_root,
                    &worktree_path,
                    retry_warnings,
                ) {
                    Err(error) => Some(("worktree removal failed", error.to_string())),
                    Ok(()) => match ctx_traits_io::worktree::delete_branch(
                        repo_root,
                        &worktree.branch,
                        retry_warnings,
                    ) {
                        Err(error) => Some(("branch delete failed", error.to_string())),
                        Ok(()) => None,
                    },
                }
            }
        };
    if let Some((cleanup_stage_reason, cleanup_detail)) = cleanup_failure {
        let ledger_reason = format!(
            "{} fast-forward to {landed_rev}: {cleanup_stage_reason}",
            reasons::POST_MERGE_CLEANUP_FAILED_PREFIX
        );
        let cli_reason = format!("{ledger_reason}: {cleanup_detail}");
        let cleanup_frame = MergeFrame {
            stage: MergeStage::Cleanup,
            status: MergeStatus::PostMergeCleanupFailure,
            reason: Some(ledger_reason),
            evidence: with_retry_evidence(landed_evidence, retry_warnings),
            park_reason: None,
            deep_decisions: Vec::new(),
        };
        if let Some(live) = input.live.as_ref() {
            live.emit(&MergeProgress::FrameRecorded(&cleanup_frame));
        }
        ctx_traits_io::run_session::append_merge_frame(session_path, cleanup_frame)?;
        return Ok(MergeReport {
            status: "post-merge-cleanup-failure".to_string(),
            run_id: input.run_id.to_string(),
            reason: Some(cli_reason),
            ..Default::default()
        });
    }

    // The terminal Merged frame's evidence includes the cumulative
    // git-lock-retry evidence gathered across this entire merge lifecycle
    // (rebase, reconciliation, fast-forward, and cleanup), so a successful
    // landing that hit transient lock contention still records it.
    let merged_frame = MergeFrame {
        stage: MergeStage::Cleanup,
        status: MergeStatus::Merged,
        reason: None,
        evidence: with_retry_evidence(landed_evidence, retry_warnings),
        park_reason: None,
        deep_decisions: Vec::new(),
    };
    if let Some(live) = input.live.as_ref() {
        live.emit(&MergeProgress::FrameRecorded(&merged_frame));
    }
    ctx_traits_io::run_session::append_merge_frame(session_path, merged_frame)?;
    Ok(MergeReport {
        status: "merged".to_string(),
        run_id: input.run_id.to_string(),
        reason: None,
        ..Default::default()
    })
}

/// Run the declared `[merge] gate` (P477) inside `worktree_path`, in order,
/// using the resolved `[worktree].env` overlay every other in-worktree
/// subprocess uses. Runs on every landing path (clean fast-forward, standard
/// reconciliation, and `--deep` alike), after reconciliation/trailer-
/// amendment and before seed harvest or the fast-forward. An empty gate (the
/// product default) runs no subprocess at all — it records one advisory and
/// still runs the tracked-tree check below, so every gate-independent
/// safeguard applies regardless of whether a gate is declared. Returns
/// `None` when every command passed and the tracked worktree stayed clean
/// afterward (the caller proceeds to land); `Some` with a terminal
/// `Gates`-stage parked (or propagated-error) report otherwise — any nonzero
/// exit, timeout, output truncation, or gate-created tracked change parks
/// here, before `main` is ever touched.
/// Resolve the effective `[worktree].env` overlay for the declared
/// pre-landing gate's subprocesses, exactly like every other in-worktree
/// caller. Split into its own function purely so the two IO calls, each with
/// a large `ctx_traits_io::Error` case, compose via `?` instead of a clippy
/// `result_large_err`-tripping `Result::and_then` closure.
fn resolve_worktree_env_for_gate(
    worktree_path: &Utf8Path,
) -> crate::Result<std::collections::BTreeMap<String, String>> {
    let config = ctx_traits_io::harness_config::resolve_runtime_config(Utf8Path::new("."))?;
    // P564: gate subprocesses run INSIDE this worktree, so a `{worktree}`
    // overlay value must name this worktree — the same cache the run's own
    // frames warmed and built into. Resolving it anywhere else would make the
    // pre-landing gate compile against a different tree's artifacts, which is
    // the exact cross-run contamination the token exists to prevent.
    Ok(
        ctx_traits_io::harness_config::resolve_effective_worktree_env(
            &config.worktree,
            Some(worktree_path),
        )?,
    )
}

/// Resolve the effective long-running git timeout (`[git] long-seconds`,
/// P489) applied to `rebase`/`rebase --continue`.
fn resolve_git_long_timeout_ms() -> u64 {
    ctx_traits_io::harness_config::resolve_git_long_timeout_ms(Utf8Path::new("."))
}

/// Derive a bounded, path-safe evidence/capture label from a declared gate
/// command's argv, generically — the merge machinery does not know
/// repository tools, so it cannot special-case any one command. Strips
/// leading dashes off each argument, joins the remainder with `-`, and keeps
/// only filesystem-safe characters — a two-word command with a flag argument
/// therefore labels the same as its bare word form (e.g. `word --flag` and
/// `word flag` both label `word-flag`), which is what keeps a historically
/// declared two-command chain's evidence/capture names stable across this
/// change. Falls back to an index-based label if the argv is somehow reduced
/// to nothing printable.
fn gate_command_label(index: usize, argv: &[String]) -> String {
    let mut label = argv
        .iter()
        .map(|part| part.trim_start_matches('-'))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    label.retain(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_');
    if label.is_empty() {
        label = format!("gate-{index}");
    }
    label.truncate(40);
    label
}

/// One declared repository-relative command's run outcome, plus a
/// best-effort path to a persisted capture of its output when it failed
/// (non-zero exit, timeout, or truncated output) — `None` on success, or when
/// the best-effort write itself failed (which must never mask the run's own
/// outcome).
struct DeclaredCommandRun {
    outcome: ctx_traits_io::command::RunOutput,
    capture_path: Option<String>,
}

/// Run one declared, literal-argv (never shell) repository command inside
/// `worktree_path` under the resolved `[worktree].env` overlay, persisting a
/// best-effort capture of its output on failure so a park is diagnosable
/// without re-running the command by hand (four live parks on 2026-07-24
/// each cost a full manual reproduction because the only record was the exit
/// code). Shared by the declared `[merge] gate` (P477) and the
/// `[[merge.generated]]` rebuild step (P463) — both run the same class of
/// declared command under the same overlay and want identical
/// diagnosability, rather than reimplementing the capture-on-failure logic
/// beside each other.
fn run_declared_command(
    worktree_path: &Utf8Path,
    env_overlay: &std::collections::BTreeMap<String, String>,
    run_id: &str,
    label: &str,
    argv: &[String],
    timeout_ms: u64,
) -> crate::Result<DeclaredCommandRun> {
    let outcome = ctx_traits_io::command::run_with_env(
        ctx_traits_io::command::RunRequest {
            argv,
            cwd: None,
            exec_dir: Some(worktree_path),
            success_exit_code: &[0],
            timeout_ms: Some(timeout_ms),
            idle_timeout_ms: None,
            // A landing gate is a whole test suite, not a status line: this
            // repository's `just test-full` alone emits ~71 KiB — a full
            // workspace build plus ~40 proof binaries — so a 64 KiB budget
            // truncated EVERY green gate and made the "output exceeded the
            // capture limit" note constant background noise instead of a
            // signal. Room for a suite that grows, still bounded so a runaway
            // gate cannot stream without end.
            capture_limit: 1_048_576,
            tick_observer: None,
        },
        env_overlay,
    )?;
    let capture_path =
        ctx_traits_io::command::persist_failure_capture(&format!("gate-{label}"), run_id, &outcome)
            .map(|path| path.to_string());
    Ok(DeclaredCommandRun {
        outcome,
        capture_path,
    })
}

/// Index of the first declared `[[merge.generated]]` entry `path` matches:
/// exact path or path under a declared directory prefix — no glob engine, no
/// pattern language (P463 §3).
fn generated_artifact_index_for_path(
    generated: &[ctx_traits_io::harness_config::GeneratedArtifact],
    path: &str,
) -> Option<usize> {
    generated.iter().position(|entry| {
        entry.paths.iter().any(|declared| {
            path == declared || path.starts_with(&format!("{}/", declared.trim_end_matches('/')))
        })
    })
}

/// P549: the identity/context fields `run_landing_gate` needs alongside its
/// mutable evidence params — grouped so the live-sink addition didn't push
/// the free function over clippy's argument limit (the file's established
/// shape for this, see `MergeLockedInputs`/`MergerCall`).
#[derive(Clone, Copy)]
struct LandingGateContext<'a> {
    worktree_path: &'a Utf8Path,
    session_path: &'a Utf8Path,
    run_id: &'a str,
    live: Option<&'a MergeLive>,
}

fn run_landing_gate(
    ctx: &LandingGateContext<'_>,
    landed_evidence: &[String],
    gate_policy: &ctx_traits_io::harness_config::EffectiveMergePolicy,
    retry_warnings: &mut ctx_traits_io::worktree::RetryWarnings,
    gate_warnings: &mut Vec<String>,
) -> Option<crate::Result<MergeReport>> {
    let LandingGateContext {
        worktree_path,
        session_path,
        run_id,
        live,
    } = *ctx;
    if let Some(live) = live {
        live.emit(&MergeProgress::StageEntered(MergeStage::Gates));
    }
    let mut gate_evidence = landed_evidence.to_vec();
    if gate_policy.gate.is_empty() {
        gate_evidence.push(format!("advisory: {EMPTY_GATE_ADVISORY}"));
        gate_warnings.push(EMPTY_GATE_ADVISORY.to_string());
    } else {
        // P462: probe free disk space on the worktree's volume BEFORE any
        // declared gate command runs, so a below-floor park carries no
        // partial-build evidence — nothing was attempted. `Ok(None)` (a
        // non-unix target) and a probe `Err` both skip the check rather than
        // park on an unknowable answer.
        if gate_policy.disk_floor_mb > 0
            && let Ok(Some(available_bytes)) =
                ctx_traits_io::environment::available_disk_bytes(worktree_path)
        {
            let floor_bytes = gate_policy.disk_floor_mb.saturating_mul(1024 * 1024);
            if available_bytes < floor_bytes {
                gate_evidence.push(format!(
                    "disk-preflight: available={available_bytes} floor={floor_bytes} (merge.disk-floor-mb={})",
                    gate_policy.disk_floor_mb
                ));
                return Some(park(
                    session_path,
                    run_id,
                    MergeStage::Gates,
                    format!(
                        "{} {available_bytes} bytes free, below the {floor_bytes}-byte floor \
                         (merge.disk-floor-mb={}); declared regenerable caches were pruned; \
                         free space and re-run merge",
                        reasons::GATE_DISK_FLOOR_PREFIX,
                        gate_policy.disk_floor_mb
                    ),
                    gate_evidence,
                    retry_warnings,
                    live,
                ));
            }
        }
        let env_overlay = match resolve_worktree_env_for_gate(worktree_path) {
            Ok(overlay) => overlay,
            Err(error) => {
                return Some(parked_on_error(
                    worktree_path,
                    ParkTarget {
                        session_path,
                        run_id,
                    },
                    MergeStage::Gates,
                    gate_evidence,
                    error,
                    retry_warnings,
                    live,
                ));
            }
        };
        let timeout_ms = gate_policy.gate_seconds.saturating_mul(1_000);
        for (index, argv) in gate_policy.gate.iter().enumerate() {
            let label = gate_command_label(index, argv);
            if let Some(live) = live {
                live.emit(&MergeProgress::GateCommandStarted { label: &label });
            }
            let run = match run_declared_command(
                worktree_path,
                &env_overlay,
                run_id,
                &label,
                argv,
                timeout_ms,
            ) {
                Ok(run) => run,
                Err(error) => {
                    return Some(parked_on_error(
                        worktree_path,
                        ParkTarget {
                            session_path,
                            run_id,
                        },
                        MergeStage::Gates,
                        gate_evidence,
                        error,
                        retry_warnings,
                        live,
                    ));
                }
            };
            let outcome = run.outcome;
            gate_evidence.push(format!(
                "gate={label} argv={argv:?} exit={:?} timed-out={}",
                outcome.exit_code, outcome.timed_out
            ));
            if let Some(live) = live {
                live.emit(&MergeProgress::GateCommandFinished {
                    label: &label,
                    exit_code: outcome.exit_code,
                    timed_out: outcome.timed_out,
                });
            }
            let truncated = outcome.stdout_truncated || outcome.stderr_truncated;
            if !outcome.success {
                if let Some(capture_path) = run.capture_path {
                    gate_evidence.push(format!("gate-output={capture_path}"));
                }
                return Some(park(
                    session_path,
                    run_id,
                    MergeStage::Gates,
                    format!(
                        "{}{label} failed: exit={:?} timed-out={} truncated={truncated}",
                        reasons::GATE_FAILED_PREFIX,
                        outcome.exit_code,
                        outcome.timed_out,
                    ),
                    gate_evidence,
                    retry_warnings,
                    live,
                ));
            }
            // Truncation is a property of OUR COPY of the output, never of the
            // gate's verdict — the exit code is the verdict, and a gate that
            // exited 0 passed. Parking on truncation alone turned every green
            // gate red the moment `just test-full` grew past the 64 KiB
            // capture limit (its output is ~71 KiB: a full workspace build
            // plus ~40 proof binaries), so every run in flight parked with
            // `exit=Some(0)` and nothing to fix. Record where the complete
            // output lives and carry on.
            if truncated {
                if let Some(capture_path) = run.capture_path {
                    gate_evidence.push(format!("gate-output={capture_path}"));
                    gate_warnings.push(format!(
                        "{label} passed, but its output exceeded the capture limit; \
                         the complete output is at {capture_path}"
                    ));
                } else {
                    gate_warnings.push(format!(
                        "{label} passed, but its output exceeded the capture limit \
                         and no capture file was written"
                    ));
                }
            }
        }
    }
    match ctx_traits_io::worktree::is_clean(worktree_path) {
        Ok(true) => {}
        Ok(false) => {
            return Some(park(
                session_path,
                run_id,
                MergeStage::Gates,
                reasons::GATE_LEFT_DIRTY.to_string(),
                gate_evidence,
                retry_warnings,
                live,
            ));
        }
        Err(error) => {
            return Some(parked_on_error(
                worktree_path,
                ParkTarget {
                    session_path,
                    run_id,
                },
                MergeStage::Gates,
                gate_evidence,
                error.into(),
                retry_warnings,
                live,
            ));
        }
    }
    let gates_passed_frame = MergeFrame {
        stage: MergeStage::Gates,
        status: MergeStatus::GatesPassed,
        reason: None,
        evidence: with_retry_evidence(gate_evidence, retry_warnings),
        park_reason: None,
        deep_decisions: Vec::new(),
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&gates_passed_frame));
    }
    if let Err(error) =
        ctx_traits_io::run_session::append_merge_frame(session_path, gates_passed_frame)
    {
        return Some(Err(error.into()));
    }
    None
}

/// Resolved standing merger agent — harness, CLI convention, assignment, and
/// the effective `[worktree].env` overlay for its one-shot process — assembled
/// only when a merge actually needs it (a divergent history, or
/// `--force-merger`). A true clean fast-forward never calls this: a missing
/// or invalid merger configuration must not block a case that cannot use it.
struct ResolvedMerger {
    harness: ctx_traits_io::harness_config::HarnessDefinition,
    cli: ctx_traits_io::harness_config::HarnessCliConvention,
    assignment: ctx_traits_io::harness_config::ProfileAssignment,
    harness_id: String,
    env_overlay: std::collections::BTreeMap<String, String>,
    /// P478: generated write-confinement payloads for the merger's one-shot
    /// process, which always runs inside the run worktree. `None` when
    /// confinement is disabled — never `None` merely because the merger
    /// itself has no worktree, since the merger always dispatches inside one.
    confinement: Option<ctx_traits_io::confinement::ConfinementPayloads>,
    /// `true` when this call actually resolved `[agent.role.merger-deep]` (or
    /// fell back to `[agent.role.merger]` under `--deep`) — selects the deep
    /// prompt/receipt-parsing convention in [`dispatch_merger`].
    deep: bool,
}

/// Resolve and validate the merger assignment, validate/probe its harness,
/// and resolve the worktree environment overlay for its one-shot process —
/// exactly the checks `merge()` used to perform unconditionally, before the
/// merge lock was even acquired, moved here so they only run for a merge that
/// is actually going to invoke the merger. When `deep` is set,
/// `[agent.role.merger-deep]` is preferred, falling back to
/// `[agent.role.merger]` when the deep table is absent (in which case
/// `ResolvedMerger::deep` is still `true`: the deep prompt/receipt convention
/// applies to whichever harness ends up dispatched).
fn resolve_merger(
    profile: &mut ctx_traits_io::harness_config::ResolvedRuntimeAssignments,
    deep: bool,
    worktree_path: &Utf8Path,
) -> crate::Result<ResolvedMerger> {
    let (role_label, merger_assignment) = if deep {
        let role_label = if profile.merger_deep_table_present() {
            "merger-deep"
        } else {
            "merger"
        };
        match profile.resolved_merger_deep_or_fallback_assignment()? {
            Some(assignment) => (role_label, assignment),
            None => {
                return Err(crate::Error::Command {
                    message:
                        "merge unavailable: no [agent.role.merger-deep] or [agent.role.merger] configured"
                            .to_string(),
                });
            }
        }
    } else {
        let Some(assignment) = profile.resolved_merger_assignment()? else {
            return Err(crate::Error::Command {
                message: "merge unavailable: no [agent.role.merger] configured".to_string(),
            });
        };
        ("merger", assignment)
    };
    let merger_harness_id =
        merger_assignment
            .harness
            .clone()
            .ok_or_else(|| crate::Error::Command {
                message: format!(
                    "merge unavailable: [agent.role.{role_label}] must declare harness"
                ),
            })?;
    // A `--assign merger=...` override with `replace_inherited = true` can
    // fully replace the base `[agent.role.merger]` table and end up with no
    // model or reasoning-effort even though the base table declared them —
    // `validate_cli_standing_agent` only checks CLI-flag-vs-model consistency
    // when a model is actually present, so the *final resolved* assignment
    // must be checked here too, not just the base table at config-load time.
    if merger_assignment.model.is_none() {
        return Err(crate::Error::Command {
            message: format!(
                "merge unavailable: resolved [agent.role.{role_label}] assignment has no model"
            ),
        });
    }
    if merger_assignment.reasoning_effort.is_none() {
        return Err(crate::Error::Command {
            message: format!(
                "merge unavailable: resolved [agent.role.{role_label}] assignment has no reasoning-effort"
            ),
        });
    }
    let (merger_harness, merger_cli) = agent_dispatch::validate_cli_standing_agent(
        &profile.registry,
        merger_assignment.mode,
        &merger_harness_id,
        merger_assignment
            .transport
            .unwrap_or(ctx_traits_io::harness_config::RunTransport::Cli),
        merger_assignment.model.as_deref(),
        merger_assignment.reasoning_effort.as_deref(),
        role_label,
    )?;

    // --- Probe the merger's actual binary before any Git-mutating step: a
    // missing/broken harness must fail loudly here, not mid-rebase. ---
    if let Err(error) = ctx_traits_io::harness_config::probe_harness_version(&merger_harness) {
        return Err(crate::Error::Command {
            message: format!("merge unavailable: merger harness probe failed: {error}"),
        });
    }

    // Effective `[worktree].env` overlay for the merger's one-shot process,
    // which always runs inside the run's worktree, plus the P478
    // write-confinement payloads generated for that same worktree.
    // Repository-relative path values resolve against the invocation
    // checkout, not the worktree. Empty overlay (byte-identical to no
    // overlay) when `.ctx/config.toml [worktree]` declared none.
    let (merger_env_overlay, confinement) =
        ctx_traits_io::confinement::resolve_worktree_spawn(&profile.worktree, Some(worktree_path))?;

    Ok(ResolvedMerger {
        harness: merger_harness,
        cli: merger_cli,
        assignment: merger_assignment,
        harness_id: merger_harness_id,
        env_overlay: merger_env_overlay,
        confinement,
        deep,
    })
}

/// Append this merge lifecycle's accumulated `git-lock-retry` evidence to a
/// ledger frame's `evidence`, so every terminal frame — parked, recovery
/// failure, post-merge cleanup failure, or merged — records contention that
/// happened anywhere in the lifecycle, not only on the path that led to it.
/// The single point every terminal ledger write goes through, instead of each
/// call site individually remembering to append `retry_warnings`.
fn with_retry_evidence(
    mut evidence: Vec<String>,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
) -> Vec<String> {
    evidence.extend(retry_warnings.as_slice().iter().cloned());
    evidence
}

/// P549: fan one merger call's stdout out to every present observer — the
/// debug-trace writer, the always-on P504 activity adapter, and the
/// completion-to-landing caller's own narrator-feeder/passthrough sink —
/// none of which may silently replace another. Mirrors `drive.rs`'s
/// `combine_stdout_observers`, generalized to N observers rather than two
/// since a merger call can have all three at once.
fn combine_stdout_observers(
    observers: Vec<Option<ctx_traits_io::harness::OutputObserver>>,
) -> Option<ctx_traits_io::harness::OutputObserver> {
    let observers: Vec<ctx_traits_io::harness::OutputObserver> =
        observers.into_iter().flatten().collect();
    match observers.len() {
        0 => None,
        1 => observers.into_iter().next(),
        _ => Some(Arc::new(move |chunk: &[u8]| {
            for observer in &observers {
                observer(chunk);
            }
        })),
    }
}

fn format_holder(holder: &ctx_traits_io::merge_lock::HolderInfo) -> String {
    format!("pid={} run-id={}", holder.pid, holder.run_id)
}

/// Any unexpected IO/harness failure once the rebase has begun must never
/// silently bubble past the merge boundary as a raw error: it confirms the
/// active rebase is aborted and records a typed parked outcome (branch and
/// worktree left intact) instead. The persisted ledger frame's `reason` and
/// `evidence` are a stable code with no embedded error text — Git/filesystem
/// error text can carry machine-local absolute paths, and ledger frames are
/// audit material, not CLI-only diagnostics. The detailed error is surfaced
/// only in the returned CLI-facing report, which is never itself persisted.
fn parked_on_error(
    worktree_path: &Utf8Path,
    target: ParkTarget<'_>,
    stage: MergeStage,
    evidence: Vec<String>,
    error: crate::Error,
    retry_warnings: &mut ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    if let Some(result) = ensure_rebase_aborted(
        worktree_path,
        target.session_path,
        target.run_id,
        stage,
        &evidence,
        retry_warnings,
        live,
    ) {
        return result;
    }
    let report = park(
        target.session_path,
        target.run_id,
        stage,
        reasons::UNEXPECTED_FAILURE_PREFIX.to_string(),
        evidence,
        retry_warnings,
        live,
    )?;
    Ok(MergeReport {
        reason: Some(format!("unexpected failure during merge: {error}")),
        ..report
    })
}

/// Best-effort-but-confirmed rebase abort for a caller about to record a
/// `Parked` outcome. `Parked` promises the branch/worktree were left intact;
/// if the abort itself cannot be confirmed (detection or `git rebase --abort`
/// failed), that promise cannot be made, so `Parked` must not be recorded.
/// Returns `Some` with a distinct terminal report in that case — the caller
/// must propagate it instead of proceeding to its own `park(...)` call.
/// Returns `None` when the abort was confirmed (or none was needed), meaning
/// it is safe for the caller to proceed.
fn ensure_rebase_aborted(
    worktree_path: &Utf8Path,
    session_path: &Utf8Path,
    run_id: &str,
    stage: MergeStage,
    evidence: &[String],
    retry_warnings: &mut ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> Option<crate::Result<MergeReport>> {
    match ctx_traits_io::worktree::abort_rebase_if_in_progress(worktree_path, retry_warnings) {
        Ok(_) => None,
        Err(error) => Some(recovery_failure(
            session_path,
            run_id,
            stage,
            evidence.to_vec(),
            error.into(),
            retry_warnings,
            live,
        )),
    }
}

/// Distinct terminal outcome for when an in-progress rebase's abort could not
/// itself be confirmed: unlike `Parked`, this makes no claim about the
/// branch/worktree being left intact. The persisted ledger frame uses a
/// stable reason code; the detailed error is surfaced only in the returned
/// CLI-facing report. P549: emits the same terminal frame live (via `live`)
/// that gets persisted — the choke point every park/recovery helper routes
/// through, so a live sink is never a per-call-site opt-in.
fn recovery_failure(
    session_path: &Utf8Path,
    run_id: &str,
    stage: MergeStage,
    evidence: Vec<String>,
    error: crate::Error,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    let ledger_reason = reasons::RECOVERY_UNCONFIRMED.to_string();
    let frame = MergeFrame {
        stage,
        status: MergeStatus::RecoveryFailure,
        reason: Some(ledger_reason.clone()),
        evidence: with_retry_evidence(evidence, retry_warnings),
        park_reason: None,
        deep_decisions: Vec::new(),
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&frame));
    }
    ctx_traits_io::run_session::append_merge_frame(session_path, frame)?;
    Ok(MergeReport {
        status: "recovery-failure".to_string(),
        run_id: run_id.to_string(),
        reason: Some(format!("{ledger_reason}: {error}")),
        ..Default::default()
    })
}

fn park(
    session_path: &Utf8Path,
    run_id: &str,
    stage: MergeStage,
    reason: String,
    evidence: Vec<String>,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    park_with_detail(
        ParkTarget {
            session_path,
            run_id,
        },
        stage,
        reason,
        None,
        evidence,
        retry_warnings,
        live,
    )
}

/// P549: `session_path`/`run_id` bundled once `live` pushed
/// [`park_with_detail`], [`park_with_deep_decisions`], and [`parked_on_error`]
/// over the arity lint — the two are always this merge attempt's ledger
/// identity, threaded everywhere together and never independently.
#[derive(Clone, Copy)]
struct ParkTarget<'a> {
    session_path: &'a Utf8Path,
    run_id: &'a str,
}

/// Same as [`park`], but for callers holding a raw Git/filesystem diagnostic
/// (which can carry machine-local absolute paths) alongside a stable reason.
/// Only the stable `reason` is persisted to the ledger frame; `detail`, when
/// present, is appended solely to the returned CLI-facing report. P549: the
/// terminal `Parked` frame is emitted live (when `live` is present) before it
/// is persisted, so a live sink never has to guess this ran — the same
/// choke-point discipline every park helper in this family shares.
fn park_with_detail(
    target: ParkTarget<'_>,
    stage: MergeStage,
    reason: String,
    detail: Option<String>,
    evidence: Vec<String>,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    let frame = MergeFrame {
        stage,
        status: MergeStatus::Parked,
        reason: Some(reason.clone()),
        evidence: with_retry_evidence(evidence, retry_warnings),
        park_reason: None,
        deep_decisions: Vec::new(),
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&frame));
    }
    ctx_traits_io::run_session::append_merge_frame(target.session_path, frame)?;
    let cli_reason = match detail {
        Some(detail) => format!("{reason}: {detail}"),
        None => reason,
    };
    Ok(MergeReport {
        status: "parked".to_string(),
        run_id: target.run_id.to_string(),
        reason: Some(cli_reason),
        ..Default::default()
    })
}

/// Same as [`park`], but for a P420 `--deep` merger's park (most commonly a
/// rule-4 refusal): persists the attempted decision log up to that point on
/// the same `Reconciliation`-stage frame the reason names, rather than
/// silently discarding it. Never used for the standard merger or a
/// harness-level park, both of which carry no decisions.
fn park_with_deep_decisions(
    target: ParkTarget<'_>,
    stage: MergeStage,
    reason: String,
    evidence: Vec<String>,
    deep_decisions: Vec<DeepMergeDecision>,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    if deep_decisions.is_empty() {
        return park(
            target.session_path,
            target.run_id,
            stage,
            reason,
            evidence,
            retry_warnings,
            live,
        );
    }
    let frame = MergeFrame {
        stage,
        status: MergeStatus::Parked,
        reason: Some(reason.clone()),
        evidence: with_retry_evidence(evidence, retry_warnings),
        park_reason: None,
        deep_decisions,
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&frame));
    }
    ctx_traits_io::run_session::append_merge_frame(target.session_path, frame)?;
    Ok(MergeReport {
        status: "parked".to_string(),
        run_id: target.run_id.to_string(),
        reason: Some(reason),
        ..Default::default()
    })
}

/// Stable, ordered evidence strings naming a detected stale-base overlap's
/// paths and the `main` commits responsible for them. Shared by a
/// `--park-on-overlap` parked report and the evidence threaded through to a
/// landed report (the default outcome), so both surfaces name the same
/// overlapping paths and commits rather than one carrying only a count.
fn stale_base_overlap_evidence(paths: &[String], main_commits: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|path| format!("stale-base-overlap-path={path}"))
        .chain(
            main_commits
                .iter()
                .map(|commit| format!("stale-base-overlap-main-commit={commit}")),
        )
        .collect()
}

/// Park for a detected stale-base overlap under `--park-on-overlap`: the run
/// branch and `main` both changed the same repository-relative paths since
/// the run branch's base. No rebase has started yet at the call site (see the
/// detection block in `merge_locked`), so unlike every other park path here
/// there is no in-progress rebase to abort — the branch and worktree are
/// already intact. Persists the typed [`ParkReason::StaleBaseOverlap`] detail
/// alongside the stable free-text `reason`/`evidence` every park carries, so a
/// ledger reader (or a retry without `--park-on-overlap`, which now lands by
/// default) can recover the overlapping paths and responsible `main` commits
/// without re-deriving them from Git.
fn park_stale_base_overlap(
    session_path: &Utf8Path,
    run_id: &str,
    paths: &[String],
    main_commits: &[String],
    evidence: Vec<String>,
    retry_warnings: &ctx_traits_io::worktree::RetryWarnings,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    let reason = format!(
        "{} run branch and main both changed {} path(s) since the branch base; parked because --park-on-overlap was set",
        reasons::STALE_BASE_OVERLAP_PREFIX,
        paths.len()
    );
    let frame = MergeFrame {
        stage: MergeStage::Rebase,
        status: MergeStatus::Parked,
        reason: Some(reason.clone()),
        evidence: with_retry_evidence(evidence, retry_warnings),
        park_reason: Some(ParkReason::StaleBaseOverlap {
            paths: paths.to_vec(),
            main_commits: main_commits.to_vec(),
        }),
        deep_decisions: Vec::new(),
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&frame));
    }
    ctx_traits_io::run_session::append_merge_frame(session_path, frame)?;
    Ok(MergeReport {
        status: "parked".to_string(),
        run_id: run_id.to_string(),
        reason: Some(reason),
        stale_base_overlap: stale_base_overlap_evidence(paths, main_commits),
        ..Default::default()
    })
}

/// Park for a P479 out-of-tree-mutation finding recorded under `policy =
/// "park"`. Modeled on [`park_stale_base_overlap`]: no rebase has started yet
/// at this call site, so there is no in-progress rebase to abort — the
/// branch and worktree are already intact. Persists the typed
/// [`ParkReason::OutOfTreeMutation`] detail so a ledger reader recovers the
/// offending paths and frame without re-deriving them. This park is
/// permanent for this session — nothing clears or acknowledges the finding —
/// so the reason names the only real way past it: a fresh `--worktree` run
/// (after reverting or deliberately keeping the flagged paths), or
/// `[worktree.tripwire] policy = "warn"` set deliberately before that next
/// run, never a resubmission of this same run-id.
fn park_out_of_tree_mutation(
    session_path: &Utf8Path,
    run_id: &str,
    finding: &ctx_traits_core::procedure::session::OutOfTreeMutationEvidence,
    live: Option<&MergeLive>,
) -> crate::Result<MergeReport> {
    let reason = format!(
        "invocation repository changed outside this run's worktree during frame {:?}: {}; \
         inspect {:?} in the invocation checkout, revert or keep them deliberately, then start a \
         fresh --worktree run (or set [worktree.tripwire] policy = \"warn\" deliberately first) — \
         this run's park cannot be cleared or re-merged",
        finding.frame,
        finding.paths.join(", "),
        finding.paths
    );
    let frame = MergeFrame {
        stage: MergeStage::Preflight,
        status: MergeStatus::Parked,
        reason: Some(reason.clone()),
        evidence: vec![format!("out-of-tree-mutation-frame={}", finding.frame)],
        park_reason: Some(ParkReason::OutOfTreeMutation {
            paths: finding.paths.clone(),
            frame: finding.frame.clone(),
        }),
        deep_decisions: Vec::new(),
    };
    if let Some(live) = live {
        live.emit(&MergeProgress::FrameRecorded(&frame));
    }
    ctx_traits_io::run_session::append_merge_frame(session_path, frame)?;
    Ok(MergeReport {
        status: "parked".to_string(),
        run_id: run_id.to_string(),
        reason: Some(reason),
        ..Default::default()
    })
}

/// Outcome of [`adjudicate_harvest_conflicts`]: `Proceed` carries every
/// conflict resolved into a ready-to-apply [`PlannedSeedCopy`](ctx_traits_io::worktree::PlannedSeedCopy)
/// (bytes/mode re-read from the worktree after the merger wrote them, main's
/// digest re-read at that same moment for `apply_harvest_plan`'s race
/// guard), the typed decision log, and this call's transcript/session-id
/// evidence; `Park` carries the reason, whatever partial decision log was
/// made, and the same evidence. Mirrors the partial-plan invariant
/// `plan_harvest_seeds` itself keeps: a `Proceed` here always covers every
/// supplied conflict (enforced by [`parse_deep_merge_decision`]'s coverage
/// check), never a subset.
enum HarvestAdjudicationOutcome {
    Proceed {
        copies: Vec<ctx_traits_io::worktree::PlannedSeedCopy>,
        decisions: Vec<DeepMergeDecision>,
        evidence: Vec<String>,
    },
    Park {
        reason: String,
        decisions: Vec<DeepMergeDecision>,
        evidence: Vec<String>,
    },
}

/// Bundled arguments for [`adjudicate_harvest_conflicts`] — kept as a struct
/// rather than individual parameters purely to stay under the arity lint.
struct HarvestAdjudicationInputs<'a> {
    repo_root: &'a Utf8Path,
    worktree_path: &'a Utf8Path,
    session: &'a Session,
    worktree: &'a WorktreeProvenance,
    conflicts: &'a [ctx_traits_io::worktree::SeedConflict],
    input: &'a MergeInputs<'a>,
    default_branch: &'a str,
    retry_warnings: &'a mut ctx_traits_io::worktree::RetryWarnings,
}

/// P463 item A: adjudicate every supplied (already confirmed `Overlap`-only)
/// seed-harvest conflict through the deep merger, reusing exactly the same
/// typed proceed/refuse receipt machinery ([`dispatch_merger`],
/// [`parse_deep_merge_decision`]) `--deep` reconciliation uses for code
/// conflicts. Resolves the merger assignment itself (rather than reusing one
/// resolved earlier in `merge_locked`) since this call site is reached on
/// both the true-fast-forward and divergent landing paths, and the former
/// never otherwise resolves a merger at all.
fn adjudicate_harvest_conflicts(
    args: HarvestAdjudicationInputs<'_>,
) -> crate::Result<HarvestAdjudicationOutcome> {
    let HarvestAdjudicationInputs {
        repo_root,
        worktree_path,
        session,
        worktree,
        conflicts,
        input,
        default_branch,
        retry_warnings,
    } = args;
    let mut profile =
        ctx_traits_io::harness_config::resolve_runtime_assignments(input.assignments)?;
    let merger = resolve_merger(&mut profile, true, worktree_path)?;

    let mut harvest_inputs = Vec::with_capacity(conflicts.len());
    for conflict in conflicts {
        let inputs = ctx_traits_io::worktree::read_seed_conflict_inputs(
            repo_root,
            worktree_path,
            &worktree.seed_snapshots,
            &conflict.relative,
            retry_warnings,
        )?;
        harvest_inputs.push((conflict.relative.clone(), inputs));
    }

    let mut trace_sequence: u64 = 0;
    let item_id = format!("merge-harvest-{}", epoch_seconds());
    let no_revision_evidence: Vec<String> = Vec::new();
    let (decision, call_evidence) = dispatch_merger(MergerCall {
        harness: &merger.harness,
        harness_id: &merger.harness_id,
        cli: &merger.cli,
        assignment: &merger.assignment,
        session,
        worktree,
        worktree_path,
        conflicted_paths: &[],
        deferred_generated_paths: &[],
        harvest_conflicts: Some(&harvest_inputs),
        revision_evidence: &no_revision_evidence,
        env_overlay: &merger.env_overlay,
        confinement: merger.confinement.as_ref(),
        deep: true,
        repo_root,
        base_rev: "",
        main_rev: "",
        default_branch,
        item_id: &item_id,
        trace_sequence: &mut trace_sequence,
        live: input.live.as_ref(),
        merger_stdout_observer: input.merger_stdout_observer.as_ref(),
    })?;

    match decision {
        MergerDecision::Proceed { deep_decisions } => {
            let mut copies = Vec::with_capacity(conflicts.len());
            for conflict in conflicts {
                let worktree_file = worktree_path.join(&conflict.relative);
                let bytes = std::fs::read(worktree_file.as_std_path()).map_err(|source| {
                    ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                        path: worktree_file.to_string(),
                        source,
                    })
                })?;
                let mode = std::fs::metadata(worktree_file.as_std_path())
                    .map_err(|source| {
                        ctx_traits_io::Error::from(ctx_traits_io::environment::Error::Filesystem {
                            path: worktree_file.to_string(),
                            source,
                        })
                    })?
                    .permissions()
                    .mode();
                let main_file = repo_root.join(&conflict.relative);
                let expected_main_digest = match std::fs::read(main_file.as_std_path()) {
                    Ok(main_bytes) => {
                        Some(ctx_traits_core::digest::Digest::from_bytes(&main_bytes))
                    }
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
                    Err(source) => {
                        return Err(ctx_traits_io::Error::from(
                            ctx_traits_io::environment::Error::Filesystem {
                                path: main_file.to_string(),
                                source,
                            },
                        )
                        .into());
                    }
                };
                copies.push(ctx_traits_io::worktree::PlannedSeedCopy {
                    relative: conflict.relative.clone(),
                    bytes,
                    expected_main_digest,
                    mode,
                });
            }
            Ok(HarvestAdjudicationOutcome::Proceed {
                copies,
                decisions: deep_decisions,
                evidence: call_evidence,
            })
        }
        MergerDecision::Park {
            reason,
            deep_decisions,
        } => Ok(HarvestAdjudicationOutcome::Park {
            reason,
            decisions: deep_decisions,
            evidence: call_evidence,
        }),
    }
}

/// Verify main-side preconditions: invocation is on clean `main`, the run's
/// worktree is registered on its recorded branch and clean, and no rebase is
/// already in progress there. Returns the worktree path on success, or a
/// `(stable reason, optional raw detail)` pair on failure — the detail, when
/// present, wraps a raw Git/filesystem diagnostic (which can carry
/// machine-local absolute paths) that must reach the CLI report only, never
/// the persisted ledger frame.
fn preflight(
    repo_root: &Utf8Path,
    worktree: &WorktreeProvenance,
    default_branch: &str,
    default_branch_source: ctx_traits_io::worktree::DefaultBranchSource,
    retry_warnings: &mut ctx_traits_io::worktree::RetryWarnings,
) -> Result<Utf8PathBuf, (String, Option<String>)> {
    let branch = ctx_traits_io::worktree::current_branch(repo_root).map_err(|e| {
        (
            reasons::CHECKOUT_BRANCH_PROBE_FAILED.to_string(),
            Some(e.to_string()),
        )
    })?;
    if branch != default_branch {
        return Err((
            format!(
                "{} {branch:?}, expected {default_branch:?} (from {})",
                reasons::BRANCH_MISMATCH_PREFIX,
                default_branch_source.label()
            ),
            None,
        ));
    }
    if !ctx_traits_io::worktree::is_clean_for_landing(repo_root).map_err(|e| {
        (
            reasons::CHECKOUT_CLEANLINESS_PROBE_FAILED.to_string(),
            Some(e.to_string()),
        )
    })? {
        return Err((
            format!(
                "{}{default_branch}) is not clean",
                reasons::INVOCATION_CHECKOUT_DIRTY_PREFIX
            ),
            None,
        ));
    }
    let worktree_path = ctx_traits_io::worktree::verify_worktree_registration(
        &worktree.id,
        &worktree.branch,
        retry_warnings,
    )
    .map_err(|e| {
        (
            reasons::WORKTREE_UNREGISTERED.to_string(),
            Some(e.to_string()),
        )
    })?;
    if !ctx_traits_io::worktree::is_clean(&worktree_path).map_err(|e| {
        (
            reasons::WORKTREE_CLEANLINESS_PROBE_FAILED.to_string(),
            Some(e.to_string()),
        )
    })? {
        return Err((
            format!(
                "worktree {:?} {}",
                worktree.id,
                reasons::WORKTREE_DIRTY_SUFFIX
            ),
            None,
        ));
    }
    if ctx_traits_io::worktree::rebase_in_progress(&worktree_path).map_err(|e| {
        (
            reasons::WORKTREE_REBASE_STATE_PROBE_FAILED.to_string(),
            Some(e.to_string()),
        )
    })? {
        return Err((
            format!(
                "worktree {:?} {}",
                worktree.id,
                reasons::REBASE_IN_PROGRESS_SUFFIX
            ),
            None,
        ));
    }
    Ok(worktree_path)
}

/// Bundled arguments for [`dispatch_merger`] — kept as a struct rather than
/// individual parameters purely to stay under the arity lint; every field is
/// used exactly once, at the merger call site in `merge()`.
struct MergerCall<'a> {
    harness: &'a ctx_traits_io::harness_config::HarnessDefinition,
    /// Registered harness id, for transcript/evidence labeling only — the
    /// dispatch itself only needs `harness`/`cli`.
    harness_id: &'a str,
    cli: &'a ctx_traits_io::harness_config::HarnessCliConvention,
    assignment: &'a ctx_traits_io::harness_config::ProfileAssignment,
    session: &'a Session,
    worktree: &'a WorktreeProvenance,
    worktree_path: &'a Utf8Path,
    conflicted_paths: &'a [String],
    /// P463 item C: paths a declared `[[merge.generated]]` entry matched and
    /// this call already resolved mechanically (never sent as a conflict) —
    /// named in the prompt so the merger knows not to touch them.
    deferred_generated_paths: &'a [String],
    /// P463 item A: when `Some`, this call is a seed-harvest adjudication
    /// rather than rebase-conflict reconciliation — `conflicted_paths` is
    /// unused (empty) and [`build_harvest_prompt`] replaces
    /// [`build_deep_prompt`]/[`merger_prompt`]. Always paired with `deep:
    /// true` (harvest adjudication only ever runs under `--deep`). Every
    /// decision's `path`/`supporting-edits` is additionally required to lie
    /// under a declared seed root (see [`interpret_merger_attempt`]) — a
    /// containment rule that does not apply to reconciliation.
    harvest_conflicts: Option<&'a [(String, ctx_traits_io::worktree::SeedConflictInputs)]>,
    revision_evidence: &'a [String],
    env_overlay: &'a std::collections::BTreeMap<String, String>,
    /// P478: this worktree's generated write-confinement payloads, `None`
    /// when disabled.
    confinement: Option<&'a ctx_traits_io::confinement::ConfinementPayloads>,
    /// P420: dispatch the judgment-capable deep merger convention (prompt,
    /// system prompt, and typed receipt parsing) instead of the standard
    /// mechanical-only `proceed`/`park` convention.
    deep: bool,
    repo_root: &'a Utf8Path,
    base_rev: &'a str,
    main_rev: &'a str,
    /// P488: the resolved landing/default branch name, named in the prompt
    /// text sent to the merger (never a literal `"main"`).
    default_branch: &'a str,
    /// P463 item B: this ONE `ctx traits merge` invocation's debug-trace step
    /// discriminator (`merge-<epoch>`), shared by every merger call the
    /// invocation makes so their trace files sort together and never collide
    /// with a separate later invocation's.
    item_id: &'a str,
    /// Drive-wide-style physical invocation counter for THIS invocation's
    /// merger calls (confirmation, every reconciliation-loop iteration, and
    /// every correction-retry attempt each count once) — mutated in place so
    /// trace filenames stay unique across the whole reconciliation loop.
    trace_sequence: &'a mut u64,
    /// P549: optional live-activity sink for this call's dispatch/retry
    /// events and the merger's own stdout activity.
    live: Option<&'a MergeLive>,
    /// P549: the completion-to-landing caller's narrator feeder (a seat is
    /// configured) or passthrough panel sink (none is), threaded straight
    /// through from [`MergeInputs::merger_stdout_observer`] — combined with
    /// the debug-trace writer's own observer and the always-on P504 activity
    /// adapter below, never replacing either.
    merger_stdout_observer: Option<&'a ctx_traits_io::harness::OutputObserver>,
}

/// One P463 item D correction retry: contract-shaped failures only. A
/// process failure (timeout/idle-timeout/killed/non-zero-exit/truncation) or
/// a valid decision (proceed, park, or deep refuse) is never retried — see
/// [`interpret_merger_attempt`].
const MAX_MERGER_ATTEMPTS: u32 = 2;

/// Invoke the merger for reconciliation confirmation (clean rebase) or
/// mechanical conflict reconciliation (conflicts), requiring a structured
/// `proceed`/`park` JSON result (or, under `--deep`, a typed
/// `proceed`/`refuse` decision receipt — see [`parse_deep_merge_decision`]).
/// A process failure, timeout, or truncation is treated as a park with an
/// honest reason (P463 item E) — judgment is never guessed. A contract-shaped
/// reply problem (no/invalid receipt, missing coverage, unparseable standard
/// reply) gets exactly one resume-or-cold-start correction retry (P463 item
/// D) before parking. Every physical attempt persists a transcript to the
/// run's debug store (P463 item B); the returned evidence carries every
/// attempt's transcript path, the last observed harness-side session id (so
/// the harness's own session store can be located), and retry disposition —
/// on every outcome, park or proceed, not only a landed one.
fn dispatch_merger(call: MergerCall<'_>) -> crate::Result<(MergerDecision, Vec<String>)> {
    let system_prompt = if call.harvest_conflicts.is_some() {
        format!(
            "{}\n\nThis particular call is a P463 seed-harvest adjudication (see the prompt \
             body for full instructions): write resolved bytes directly to the seeded path in \
             the worktree instead of staging a conflicted repository file, and touch nothing \
             outside the declared seed roots named in the prompt.",
            deep_merger_system_prompt()
        )
    } else if call.deep {
        deep_merger_system_prompt()
    } else {
        merger_system_prompt()
    };
    // The merger must never dispatch through the narrator's tiny, tool-less
    // argv: it has to actually edit conflicted files, so it always gets the
    // normal tool-capable convention (`use_narrator_argv: false`).
    let mut base_argv = agent_dispatch::standing_agent_argv(
        call.harness,
        call.cli,
        false,
        call.assignment.model.as_deref(),
        call.assignment.reasoning_effort.as_deref(),
        &system_prompt,
        &call.assignment.extra_args,
    );
    // P478: generated write confinement, composed with — never replacing —
    // the rest of this argv.
    agent_dispatch::append_confinement(&mut base_argv, call.harness, call.confinement);
    let (base_prompt, hunks) = if let Some(harvest_conflicts) = call.harvest_conflicts {
        build_harvest_prompt(
            call.session,
            call.worktree,
            harvest_conflicts,
            call.default_branch,
        )
    } else if call.deep {
        build_deep_prompt(&call)?
    } else {
        (
            merger_prompt(
                call.session,
                call.worktree,
                call.conflicted_paths,
                call.revision_evidence,
                call.default_branch,
            ),
            Vec::new(),
        )
    };
    let base_prompt = if call.deferred_generated_paths.is_empty() {
        base_prompt
    } else {
        format!(
            "{base_prompt}\n\nThe following declared generated-artifact path(s) were deferred \
             and already resolved mechanically outside this call — do not edit or stage them: {}",
            call.deferred_generated_paths.join(", ")
        )
    };
    // `standing_agent_argv` only delivers the system prompt via a CLI flag
    // when the harness's convention declares one; a harness without a
    // `system_prompt_flag` (e.g. opencode's usual convention) would otherwise
    // silently drop the mechanical-only/structured-reply instruction, which
    // the merger's correctness depends on. Compose it into the prompt body
    // instead in that case.
    let base_prompt = if call.cli.system_prompt_flag.is_none() {
        format!("{system_prompt}\n\n{base_prompt}")
    } else {
        base_prompt
    };
    let prompt_via_stdin = call.cli.prompt_via.as_deref() == Some("stdin");
    // Captured immediately before dispatch, only for `--deep`: the delta
    // against the post-call snapshot below isolates exactly what THIS call's
    // edits touched, excluding paths the rebase replay already staged before
    // the merger ever ran. A snapshot failure propagates as a real error
    // (via `?`) rather than failing open to an empty touch set. Shared
    // across every correction-retry attempt of this dispatch — re-capturing
    // between attempts would make attempt 1's edits invisible to attempt 2's
    // touch check, reproducing the phantom-uncovered-path class of bug in a
    // new form (P463 §3 "one subtlety").
    let declared_seed_roots: Vec<String> = call
        .worktree
        .seed_snapshots
        .iter()
        .map(|snapshot| snapshot.root.clone())
        .collect();
    let pre_fingerprint = if call.deep {
        Some(ctx_traits_io::worktree::worktree_content_fingerprint(
            call.worktree_path,
            &declared_seed_roots,
        )?)
    } else {
        None
    };
    let output_id = call.cli.output.as_deref().unwrap_or("raw-json").to_string();
    let role = if call.harvest_conflicts.is_some() {
        "merger-harvest"
    } else if call.deep {
        "merger-deep"
    } else {
        "merger"
    };

    // P478: whichever payload actually applies to this attempt's harness
    // kind (argv-delivered for claude-code, env-delivered for opencode),
    // carried only for the debug trace — the argv above already carries the
    // actual enforcement.
    let confinement_trace = call.confinement.and_then(|payloads| {
        ctx_traits_io::confinement::confinement_trace_payload(payloads, call.harness.kind())
    });
    // P480: the OS-level spawn sandbox applies at the merger's own spawn
    // seam (`run_one_shot`), regardless of harness kind.
    let spawn_sandbox = call
        .confinement
        .and_then(|payloads| payloads.spawn_sandbox.clone());

    let mut call_evidence: Vec<String> = Vec::new();
    // P475: the resolved per-call budget this dispatch actually ran under,
    // and whether it came from the seat's own declared budget or the
    // built-in default — the honest-timeout-park text above names the same
    // resolved number, never a bare constant.
    call_evidence.push(format!(
        "merger-budget-ms={} source={}",
        merger_timeout_ms(call.assignment),
        if call.assignment.budget.frame_seconds.is_some() {
            "declared"
        } else {
            "built-in"
        }
    ));
    let mut observed_session_id: Option<String> = None;
    let mut attempt_argv = base_argv.clone();
    let mut attempt_prompt = base_prompt.clone();
    let mut attempt: u32 = 1;
    let decision = loop {
        if let Some(live) = call.live {
            live.emit(&MergeProgress::MergerDispatching { role, attempt });
        }
        *call.trace_sequence += 1;
        let sequence = *call.trace_sequence;
        let trace = HarnessAttemptWriter::start(&HarnessAttemptStart {
            run_id: call.session.run_id.as_str(),
            session_id: call.session.session_id.as_str(),
            sequence,
            item_id: Some(call.item_id),
            frame_title: "merge",
            role,
            harness_id: call.harness_id,
            attempt: attempt as u64,
            invocation: "one-shot",
            argv: &attempt_argv,
            prompt: &attempt_prompt,
            stdout_limit: ctx_traits_io::harness::DEFAULT_CAPTURE_LIMIT,
            confinement: confinement_trace,
            spawn_sandbox: spawn_sandbox
                .as_ref()
                .map(|sandbox| sandbox.profile.as_str()),
        });
        let active_writer = match trace {
            Ok(writer) => Some(writer),
            Err(error) => {
                // Best-effort: a trace-write failure never blocks the merge
                // (same posture as drive's `warn_trace_once`).
                call_evidence.push(format!("merger-transcript-warning={error}"));
                None
            }
        };
        // P549: the dim ~60s stderr line is gone — a live sink (when present)
        // now gets a repaint nudge on the same cadence; without one, the tick
        // observer does nothing, same as an uninstrumented one-shot call.
        let last_tick = Arc::new(Mutex::new(Instant::now()));
        let tick_observer: ctx_traits_io::harness::TickObserver = {
            let last_tick = Arc::clone(&last_tick);
            let live = call.live.cloned();
            Arc::new(move || {
                let Ok(mut last) = last_tick.lock() else {
                    return;
                };
                if last.elapsed() >= Duration::from_secs(60) {
                    *last = Instant::now();
                    if let Some(live) = live.as_ref() {
                        live.emit(&MergeProgress::MergerDispatching { role, attempt });
                    }
                }
            })
        };
        // P549: the merger call's own stdout speaks the P504 activity
        // vocabulary — via the same `HarnessActivityAdapter` a driven
        // frame's stream uses — into the live sink when one is present,
        // ALONGSIDE (never instead of) the debug-trace writer and the
        // caller's own narrator-feeder/passthrough observer.
        let activity_observer: Option<ctx_traits_io::harness::OutputObserver> = call
            .live
            .cloned()
            .zip(ctx_traits_io::harness_config::activity_adapter_kind(
                call.harness,
            ))
            .map(|(live, kind)| {
                let adapter = Arc::new(Mutex::new(
                    ctx_traits_io::harness_activity::HarnessActivityAdapter::new(
                        kind,
                        "merge:reconciliation",
                    ),
                ));
                Arc::new(move |chunk: &[u8]| {
                    let Ok(mut adapter) = adapter.lock() else {
                        return;
                    };
                    for event in adapter.push(chunk) {
                        live.emit_activity_event(event);
                    }
                }) as ctx_traits_io::harness::OutputObserver
            });
        let outcome = agent_dispatch::run_one_shot(agent_dispatch::RunOneShotRequest {
            harness: call.harness,
            argv: attempt_argv.clone(),
            prompt: attempt_prompt.clone(),
            prompt_via_stdin,
            timeout_ms: merger_timeout_ms(call.assignment),
            exec_dir: Some(call.worktree_path),
            env_overlay: call.env_overlay,
            observers: RunOneShotObservers {
                stdout_observer: combine_stdout_observers(vec![
                    active_writer
                        .as_ref()
                        .map(|writer| writer.stdout_observer()),
                    activity_observer,
                    call.merger_stdout_observer.cloned(),
                ]),
                tick_observer: Some(tick_observer),
            },
            sandbox: spawn_sandbox.clone(),
        })?;
        if let Some(writer) = active_writer {
            match writer.finish(&HarnessAttemptExit {
                exit_code: outcome.exit_code,
                timed_out: outcome.timed_out,
                idle_timed_out: outcome.idle_timed_out,
                stdout_truncated: outcome.stdout_truncated,
                duration_ms: outcome.duration_ms,
                stderr: &outcome.stderr,
            }) {
                Ok(path) => call_evidence.push(format!("merger-transcript={path}")),
                Err(error) => call_evidence.push(format!("merger-transcript-warning={error}")),
            }
        }
        if let Some(session_id) =
            harness_stream::stream_harness_session_id(&output_id, &outcome.stdout)
        {
            observed_session_id = Some(session_id);
        }

        let interpreted = interpret_merger_attempt(
            &call,
            &outcome,
            &output_id,
            &hunks,
            pre_fingerprint.as_ref(),
            &declared_seed_roots,
        )?;
        let problem = match interpreted {
            Ok(decision) => break decision,
            Err(problem) => problem,
        };
        if attempt >= MAX_MERGER_ATTEMPTS {
            call_evidence.push(format!(
                "merger-retry=exhausted attempt={attempt} problem={problem}"
            ));
            break MergerDecision::Park {
                reason: format!(
                    "{} {attempt} attempt(s) (one correction retry already used): {problem}",
                    reasons::RECEIPT_INVALID_PREFIX
                ),
                deep_decisions: Vec::new(),
            };
        }
        // One resume-or-visible-cold-start correction retry (P463 item D):
        // never for a process failure or a valid refuse/park (both already
        // broke out above via `Ok(decision)`), only for THIS contract-shaped
        // problem.
        let (retry_argv, retry_prompt, resumed, cold_start_reason) = build_correction_delivery(
            &call,
            &base_argv,
            &base_prompt,
            &problem,
            observed_session_id.as_deref(),
        );
        call_evidence.push(format!(
            "merger-retry={} attempt={attempt} problem={problem}",
            if resumed { "resumed" } else { "cold-start" }
        ));
        if let Some(reason) = cold_start_reason {
            call_evidence.push(format!("merger-retry-cold-start-reason={reason}"));
        }
        attempt_argv = retry_argv;
        attempt_prompt = retry_prompt;
        attempt += 1;
        if let Some(live) = call.live {
            live.emit(&MergeProgress::MergerRetrying { role, attempt });
        }
    };
    if let Some(session_id) = observed_session_id {
        call_evidence.push(format!("harness-session-id={session_id}"));
    }
    Ok((decision, call_evidence))
}

/// Seconds since the Unix epoch, best-effort (0 on a clock error). Mirrors
/// the same small private helper already duplicated per-module in
/// `ctx_traits_io` (`debug_trace`, `state`, `run_session`) rather than
/// exporting a new shared one for a single-line computation.
fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Trimmed last non-blank stderr line, for an honest non-zero-exit park
/// reason (P463 item E) — never the full stderr body, which can be large and
/// is already captured in the attempt's transcript.
fn trimmed_stderr_tail(stderr: &str) -> String {
    stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect::<String>())
        .unwrap_or_else(|| "no stderr output".to_string())
}

/// Classify one physical merger attempt's raw outcome. `Ok(Ok(decision))` is
/// a terminal decision that must never be retried: a process failure
/// (timeout, idle timeout, killed by signal, non-zero exit, truncation —
/// each with an honest, distinct reason per P463 item E) or a valid parsed
/// decision (proceed, standard park, or deep refuse). `Ok(Err(problem))` is a
/// contract-shaped reply problem — eligible for exactly one correction retry
/// (P463 item D). `Err` is a real IO/harness failure, propagated by the
/// caller via `?`.
fn interpret_merger_attempt(
    call: &MergerCall<'_>,
    outcome: &ctx_traits_io::harness::HarnessRunOutcome,
    output_id: &str,
    hunks: &[DeepHunk],
    pre_fingerprint: Option<
        &std::collections::BTreeMap<String, ctx_traits_io::worktree::PathFingerprint>,
    >,
    declared_seed_roots: &[String],
) -> crate::Result<Result<MergerDecision, String>> {
    if outcome.timed_out {
        let budget_ms = merger_timeout_ms(call.assignment);
        return Ok(Ok(MergerDecision::Park {
            reason: format!(
                "{} after {}ms (budget {budget_ms}ms)",
                reasons::MERGER_TIMEOUT_PREFIX,
                outcome.duration_ms
            ),
            deep_decisions: Vec::new(),
        }));
    }
    if outcome.idle_timed_out {
        return Ok(Ok(MergerDecision::Park {
            reason: format!(
                "{}: no output progress before the idle budget elapsed",
                reasons::MERGER_IDLE_TIMEOUT_PREFIX
            ),
            deep_decisions: Vec::new(),
        }));
    }
    if outcome.exit_code.is_none() {
        return Ok(Ok(MergerDecision::Park {
            reason: format!(
                "{} (no exit code observed) — likely an interrupt, not a malformed reply",
                reasons::MERGER_SIGNAL_KILL_PREFIX
            ),
            deep_decisions: Vec::new(),
        }));
    }
    if outcome.exit_code != Some(0) {
        return Ok(Ok(MergerDecision::Park {
            reason: format!(
                "{} {:?}: {}",
                reasons::MERGER_NONZERO_EXIT_PREFIX,
                outcome.exit_code,
                trimmed_stderr_tail(&outcome.stderr)
            ),
            deep_decisions: Vec::new(),
        }));
    }
    if outcome.stdout_truncated {
        return Ok(Ok(MergerDecision::Park {
            reason: format!(
                "{} — refusing to parse a truncated reply",
                reasons::MERGER_TRUNCATED_PREFIX
            ),
            deep_decisions: Vec::new(),
        }));
    }
    if call.deep {
        // Guaranteed `Some` by the caller's `if call.deep` capture.
        let pre_fingerprint =
            pre_fingerprint.expect("pre-dispatch fingerprint captured for --deep");
        let post_fingerprint = ctx_traits_io::worktree::worktree_content_fingerprint(
            call.worktree_path,
            declared_seed_roots,
        )?;
        let changed_paths =
            ctx_traits_io::worktree::touched_paths_delta(pre_fingerprint, &post_fingerprint);
        match parse_deep_merge_decision(output_id, &outcome.stdout, hunks, &changed_paths) {
            Ok(decision) => match harvest_containment_problem(call, &decision, declared_seed_roots)
            {
                Some(problem) => Ok(Err(problem)),
                None => Ok(Ok(decision)),
            },
            Err(receipt_problem) => Ok(Err(receipt_problem)),
        }
    } else {
        match parse_merge_decision(output_id, &outcome.stdout) {
            Some(decision) => Ok(Ok(decision)),
            None => Ok(Err(
                "merger output did not contain a structured proceed/park result".to_string(),
            )),
        }
    }
}

/// P463 item A's extra containment rule, applied only to a harvest call's
/// `Proceed` receipt (a `Park`/refuse never lands anything, so nothing to
/// contain): every decision `path` and `supporting-edits` entry must lie
/// under one of the declared seed roots. Returns `Some(problem)` (eligible
/// for the same one correction retry as any other contract-shaped failure) on
/// a violation, `None` for a non-harvest call or a clean containment check.
fn harvest_containment_problem(
    call: &MergerCall<'_>,
    decision: &MergerDecision,
    declared_seed_roots: &[String],
) -> Option<String> {
    call.harvest_conflicts?;
    let MergerDecision::Proceed { deep_decisions } = decision else {
        return None;
    };
    for entry in deep_decisions {
        for path in std::iter::once(&entry.path).chain(entry.supporting_edits.iter()) {
            if !path_is_under_seed_root(path, declared_seed_roots) {
                return Some(format!(
                    "decision touches {path:?}, which lies outside every declared seed root \
                     ({}) — a harvest adjudication may only touch declared seed-root paths",
                    declared_seed_roots.join(", ")
                ));
            }
        }
    }
    None
}

/// `true` when `path` is exactly one declared seed root, or lies under one as
/// a directory prefix — path-separator-boundary-safe, mirroring
/// [`generated_artifact_index_for_path`]'s near-miss guard (a declared root
/// `"scripts/goldens"` must not match `"scripts/goldens-extra/x"`).
fn path_is_under_seed_root(path: &str, seed_roots: &[String]) -> bool {
    seed_roots.iter().any(|root| {
        let root = root.trim_end_matches('/');
        path == root || path.starts_with(&format!("{root}/"))
    })
}

/// One P463 item D correction retry's resolved delivery: resume the harness
/// conversation when the CLI convention declares a `session-flag`/
/// `resume-flag` AND a session id was actually observed on the failed
/// attempt, otherwise fall back to a visible cold start re-sending the full
/// original prompt with a `CORRECTION:` block appended. Mirrors
/// `drive.rs`'s `prepare_correction_retry` in spirit but is not shared with
/// it: that function is built around frame prompts/`AssignmentPlan`/JSON
/// schemas, none of which the merger dispatch has.
fn build_correction_delivery(
    call: &MergerCall<'_>,
    base_argv: &[String],
    original_prompt: &str,
    problem: &str,
    observed_session_id: Option<&str>,
) -> (Vec<String>, String, bool, Option<String>) {
    let resume_flag = call
        .cli
        .session_flag
        .as_ref()
        .or(call.cli.resume_flag.as_ref());
    let correction = format!(
        "CORRECTION: your previous reply did not satisfy the required receipt contract: \
         {problem}\n\nReply again with exactly one corrected JSON object satisfying the \
         contract above."
    );
    if let (Some(flag), Some(session_id)) = (resume_flag, observed_session_id) {
        let mut argv = base_argv.to_vec();
        argv.push(flag.clone());
        argv.push(session_id.to_string());
        return (argv, correction, true, None);
    }
    let cold_start_reason = if resume_flag.is_none() {
        "harness declares no session-flag or resume-flag".to_string()
    } else {
        "no harness session id observed for the failed attempt".to_string()
    };
    let cold_prompt = format!("{original_prompt}\n\n{correction}");
    (
        base_argv.to_vec(),
        cold_prompt,
        false,
        Some(cold_start_reason),
    )
}

/// One conflicted path's supplied hunk id for one conflict region, computed
/// deterministically before dispatch (see [`deep_hunk_id`]) — a path with
/// several distinct conflict regions supplies several `DeepHunk` entries
/// sharing the same `path`.
struct DeepHunk {
    path: String,
    hunk_id: String,
}

fn deep_merger_system_prompt() -> String {
    "You are the ctx.traits deep merger: a judgment-capable standing agent reconciling an \
     already-completed, already-reviewed run's rebase onto main. Resolve every conflicted hunk \
     under exactly one of five stable doctrines: (1) preserve-landed-surface — main's landed \
     surface (a rename, regroup, or refactor already on main) wins and must NOT be re-litigated; \
     port the branch's accepted payload INTO that landed surface rather than restoring the \
     branch's pre-rename/pre-refactor shape; (2) union-additive — union additive same-anchor changes from \
     both sides rather than picking one; (3) unify-duplicate — unify duplicate components around \
     the stronger primitive, keeping every caller/API working; (4) refuse-regression — refuse a \
     change that would revert a landed decision; (5) verify-factual-claim — when the two sides \
     make contradictory factual claims, verify against the actual code before choosing. Stage \
     every resolved path. You may make supporting edits outside a conflicted path only if you \
     name every one of them in a decision's supporting-edits list. \"Touch\" is judged by a \
     before/after content fingerprint of the whole worktree (tracked index state including \
     staging, plus untracked files): every file your session edits, creates, deletes, or stages \
     — even incidentally, even via a script or build command — MUST appear as some decision's \
     path or inside a supporting-edits list, so avoid running commands that write files you do \
     not intend to account for. Never run git rebase --continue \
     or git rebase --abort yourself; ctx does that based on your reply. Reply with exactly one \
     JSON object and nothing else: {\"result\":\"proceed\",\"decisions\":[{\"path\":\"<repo-relative \
     path>\",\"hunk\":\"<the supplied hunk id for that region — a path with more than one listed \
     hunk needs one decision entry per hunk>\",\"rule\":\"<one of the five rule ids \
     above>\",\"choice\":\"<what you resolved it to>\",\"rationale\":\"<why>\",\
     \"supporting-edits\":[\"<optional: another path you also edited>\", ...]}, ...]} — every \
     supplied conflicted hunk, and every path you touch, must be covered by exactly one decision \
     entry's path or supporting-edits. If any conflict requires refusing under rule 4, reply \
     instead with: {\"result\":\"refuse\",\"reason\":\"<why, naming refuse-regression>\",\
     \"decisions\":[<any decisions you had already made>]}."
        .to_string()
}

/// Build a P420 `--deep` merger prompt: exact base64 conflict-stage bytes and
/// per-path `main`-side history for every currently conflicted path (see
/// [`ConflictBlobs`](ctx_traits_io::worktree::ConflictBlobs)'s rebase
/// stage-to-side labeling), the same accepted-slot/final-output run context
/// [`merger_prompt`] gives the standard merger, and the five doctrines
/// verbatim in operational form. Returns the prompt text and the
/// deterministic per-path hunk ids the reply's receipt must cover.
fn build_deep_prompt(call: &MergerCall<'_>) -> crate::Result<(String, Vec<DeepHunk>)> {
    let mut sections = vec![
        format!("run-id: {}", call.session.run_id.as_str()),
        format!("worktree-id: {}", call.worktree.id),
        format!("branch: {}", call.worktree.branch),
        format!("revisions: {}", call.revision_evidence.join(", ")),
    ];
    let mut hunks = Vec::new();
    if call.conflicted_paths.is_empty() {
        sections.push(format!(
            "The rebase onto {} completed with no conflicts. Confirm this run is ready to \
             land by replying proceed with an empty decisions list, or refuse if it should not \
             land.",
            call.default_branch
        ));
    } else {
        sections.push(
            "Conflicted paths (currently marked unmerged in the worktree), each with exact \
             whole-file conflict-stage bytes and one or more listed hunk ids (one per distinct \
             conflict region in the file, or a single whole-file hunk when no region is \
             separately identified):"
                .to_string(),
        );
        let mut history_warnings = ctx_traits_io::worktree::RetryWarnings::new();
        for path in call.conflicted_paths {
            let blobs = ctx_traits_io::worktree::conflict_stage_blobs(call.worktree_path, path)?;
            // One hunk id per distinct textual conflict region in the file
            // (see `conflict_marker_regions`), so a file with several
            // independent conflicts requires several typed decisions rather
            // than one coarse whole-file decision. A path with no parseable
            // regions (binary content, or an add/delete conflict form with
            // no merged text to mark up) falls back to exactly one
            // whole-file hunk.
            let regions =
                ctx_traits_io::worktree::conflict_marker_regions(call.worktree_path, path)?;
            let region_specs: Vec<Option<(usize, usize)>> = if regions.is_empty() {
                vec![None]
            } else {
                regions.into_iter().map(Some).collect()
            };
            sections.push(format!("- path: {path}"));
            sections.push(format!(
                "  stage-1 common-ancestor (base64, absent for an add conflict): {}",
                blob_field(blobs.base.as_deref())
            ));
            sections.push(format!(
                "  stage-2 {}-side (the branch being rebased ONTO; base64, absent for a {}-side delete): {}",
                call.default_branch,
                call.default_branch,
                blob_field(blobs.main_side.as_deref())
            ));
            sections.push(format!(
                "  stage-3 branch-side (the run branch's own replayed commit content; base64, absent for a branch-side delete): {}",
                blob_field(blobs.branch_side.as_deref())
            ));
            for (region_index, region) in region_specs.into_iter().enumerate() {
                let hunk_id = deep_hunk_id(
                    path,
                    region_index,
                    region,
                    blobs.base.as_deref(),
                    blobs.main_side.as_deref(),
                    blobs.branch_side.as_deref(),
                    "reconciliation",
                );
                hunks.push(DeepHunk {
                    path: path.clone(),
                    hunk_id: hunk_id.clone(),
                });
                match region {
                    Some((start_line, end_line)) => sections.push(format!(
                        "  hunk: {hunk_id} (working-tree conflict-marker lines {start_line}-{end_line})"
                    )),
                    None => sections.push(format!("  hunk: {hunk_id} (whole-file unit)")),
                }
            }
            let history = ctx_traits_io::worktree::main_history_for_path(
                call.repo_root,
                call.base_rev,
                call.main_rev,
                path,
                &mut history_warnings,
            )?;
            if !history.is_empty() {
                sections.push("  main-side history since the branch base:".to_string());
                for (sha, subject) in history {
                    sections.push(format!("    {sha} {subject}"));
                }
            }
        }
    }
    sections.extend(accepted_context_sections(call.session));
    Ok((sections.join("\n"), hunks))
}

fn blob_field(bytes: Option<&[u8]>) -> String {
    match bytes {
        Some(bytes) => ctx_traits_io::worktree::base64_encode(bytes),
        None => "<absent>".to_string(),
    }
}

/// Build a P463 item A harvest-adjudication prompt: exact base64 seed-time
/// ancestor / current-`main` / current-worktree bytes for every conflicted
/// seed path, one whole-file hunk id per path (a seed conflict carries no
/// git conflict-marker regions — see [`ctx_traits_io::worktree::SeedConflict`]
/// — so, unlike [`build_deep_prompt`], there is never more than one hunk per
/// path), plus the same accepted-slot/final-output run context every merger
/// prompt gives. The reply must write resolved bytes DIRECTLY to the seeded
/// path in the worktree (these paths are gitignored: never staged or
/// committed) and touch nothing outside the declared seed roots — enforced
/// after parsing by [`interpret_merger_attempt`]'s containment check, not by
/// this prompt text alone.
fn build_harvest_prompt(
    session: &Session,
    worktree: &WorktreeProvenance,
    conflicts: &[(String, ctx_traits_io::worktree::SeedConflictInputs)],
    default_branch: &str,
) -> (String, Vec<DeepHunk>) {
    let mut sections = vec![
        format!("run-id: {}", session.run_id.as_str()),
        format!("worktree-id: {}", worktree.id),
        format!(
            "This is a P463 seed-harvest adjudication, not a rebase conflict: the paths below \
             are declared `[worktree] seed` roots (typically gitignored, e.g. `.plans`, `.docs`) \
             that changed independently on both {default_branch} and this run's worktree since \
             they were seeded. Write each resolved file's full final bytes DIRECTLY to the given \
             path in the worktree — these paths are gitignored, so do not `git \
             add`/stage/commit them. Touch nothing outside the paths listed below."
        ),
    ];
    let mut hunks = Vec::new();
    for (relative, inputs) in conflicts {
        let hunk_id = deep_hunk_id(
            relative,
            0,
            None,
            Some(&inputs.ancestor),
            Some(&inputs.main),
            Some(&inputs.worktree),
            "harvest",
        );
        hunks.push(DeepHunk {
            path: relative.clone(),
            hunk_id: hunk_id.clone(),
        });
        sections.push(format!("- path: {relative}"));
        sections.push(format!(
            "  seed-time ancestor (base64): {}",
            blob_field(Some(&inputs.ancestor))
        ));
        sections.push(format!(
            "  {default_branch}-side (base64, current {default_branch} content): {}",
            blob_field(Some(&inputs.main))
        ));
        sections.push(format!(
            "  worktree-side (base64, this run's content): {}",
            blob_field(Some(&inputs.worktree))
        ));
        sections.push(format!("  hunk: {hunk_id} (whole-file unit)"));
    }
    sections.extend(accepted_context_sections(session));
    (sections.join("\n"), hunks)
}

/// Deterministic hunk id for one conflict region within one conflicted path
/// — `region` is that region's 1-based inclusive working-tree conflict-
/// marker line range (`None` for a whole-file unit, the fallback for a
/// binary or add/delete conflict form with no parseable regions, or for a
/// P463 item A harvest conflict, which is always one whole-file hunk).
/// `kind` distinguishes the caller (`"reconciliation"` vs `"harvest"`) so the
/// two callers can never collide on the same id for coincidentally identical
/// path/region/blob inputs. Derived from the path, the region's index and
/// line range, and the digest of each of the three supplied blobs (or a
/// stable `absent` marker for `None`): stable across dispatch retries of the
/// same conflict, and distinct for any change to any blob OR for each of
/// several independent regions within the same file. Shared by
/// [`build_deep_prompt`] (conflict-stage blobs) and P463 item A's harvest
/// adjudication prompt (seed-time ancestor/main/worktree blobs) rather than
/// each computing its own scheme.
fn deep_hunk_id(
    path: &str,
    region_index: usize,
    region: Option<(usize, usize)>,
    base: Option<&[u8]>,
    main_side: Option<&[u8]>,
    branch_side: Option<&[u8]>,
    kind: &str,
) -> String {
    fn digest_of(bytes: Option<&[u8]>) -> String {
        match bytes {
            Some(bytes) => ctx_traits_core::digest::Digest::from_bytes(bytes)
                .as_str()
                .to_string(),
            None => "absent".to_string(),
        }
    }
    let region_field = match region {
        Some((start, end)) => format!("{start}-{end}"),
        None => "whole-file".to_string(),
    };
    let seed = format!(
        "{kind}\n{path}\n{region_index}\n{region_field}\n{}\n{}\n{}",
        digest_of(base),
        digest_of(main_side),
        digest_of(branch_side)
    );
    let full = ctx_traits_core::digest::Digest::source(&seed);
    format!(
        "hunk-{}",
        &full.as_str().trim_start_matches("sha256:")[..12]
    )
}

fn merger_system_prompt() -> String {
    "You are the ctx.traits merger: a standing agent that performs ONLY mechanical rebase \
     reconciliation for an already-completed, already-reviewed run. Do not redesign, re-review, \
     or second-guess the accepted work — that judgment already happened. If reconciling the \
     situation described is purely mechanical (no semantic ambiguity), resolve any listed \
     conflicts faithfully to the accepted design in the worktree, stage the resolved paths, and \
     reply with exactly one JSON object: {\"result\":\"proceed\"}. If anything requires judgment, \
     redesign, or is not obviously mechanical, do not attempt it — reply with exactly one JSON \
     object: {\"result\":\"park\",\"reason\":\"<why>\"}. Do not run git rebase --continue or git \
     rebase --abort yourself; ctx does that based on your reply. Reply with the JSON object and \
     nothing else."
        .to_string()
}

fn merger_prompt(
    session: &Session,
    worktree: &WorktreeProvenance,
    conflicted_paths: &[String],
    revision_evidence: &[String],
    default_branch: &str,
) -> String {
    let mut sections = vec![
        format!("run-id: {}", session.run_id.as_str()),
        format!("worktree-id: {}", worktree.id),
        format!("branch: {}", worktree.branch),
        format!("revisions: {}", revision_evidence.join(", ")),
    ];
    if conflicted_paths.is_empty() {
        sections.push(format!(
            "The rebase onto {default_branch} completed with no conflicts. Confirm this run is \
             ready to land by replying proceed, or park if anything about the situation above \
             looks wrong."
        ));
    } else {
        sections.push(format!(
            "Conflicted paths (currently marked unmerged in the worktree): {}",
            conflicted_paths.join(", ")
        ));
    }
    sections.extend(accepted_context_sections(session));
    sections.join("\n")
}

/// The full accepted-slot ledger (agreed design, work summary, commit
/// message, and every other accepted slot value from the run) plus accepted
/// final outputs — the run context both the standard [`merger_prompt`] and
/// the P420 [`build_deep_prompt`] give their merger, so reconciliation
/// (mechanical or judgment-capable) is informed by what the run actually
/// agreed to, not guessed from the diff alone.
fn accepted_context_sections(session: &Session) -> Vec<String> {
    let mut sections = Vec::new();
    if !session.accepted_slot_values.is_empty() {
        sections.push(
            "accepted slot values (priority slots first, then acceptance order):".to_string(),
        );
        // Cheap prioritization: the named reconciliation-critical slots
        // (agreed design, work summary, commit message) are the ones a
        // mechanical merger most needs, so they sort ahead of the rest of the
        // truncated ledger dump rather than being buried in acceptance order.
        let mut slots: Vec<&_> = session.accepted_slot_values.iter().collect();
        slots.sort_by_key(|slot| !is_priority_reconciliation_slot(&slot.ref_text));
        for slot in slots {
            sections.push(format!(
                "  {}: {}",
                slot.ref_text,
                truncate(&slot.value.to_string(), 4000)
            ));
        }
    }
    if let Some(completion) = session.completion.as_ref() {
        sections.push("accepted final outputs:".to_string());
        for output in &completion.final_outputs {
            sections.push(format!(
                "  {}: {}",
                output.port_ref.id(),
                truncate(&output.value.to_string(), 4000)
            ));
        }
    }
    sections
}

/// `true` for the named slots a mechanical reconciliation most needs
/// (agreed/reviewed design, work summary, commit message) — advisory
/// prioritization only, see [`merger_prompt`].
fn is_priority_reconciliation_slot(ref_text: &str) -> bool {
    ["agreed-design", "work-summary", "commit-message"]
        .iter()
        .any(|needle| ref_text.contains(needle))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

/// Extract a structured `{"result":"proceed"}` / `{"result":"park",...}`
/// decision from a merger harness's stdout, using the same output-id
/// conventions (streaming JSON events vs. a single JSON document) as the
/// worker frame dispatch parser, and the same recursive wrapper-key/array/
/// embedded-JSON-string traversal ([`harness_stream::find_nested_object`]) as
/// slot extraction — a harness (e.g. opencode) that wraps its reply in nested
/// `message`/`content`/`part(s)` event payloads is walked exactly the same
/// way, not just checked at the root.
fn parse_merge_decision(output_id: &str, stdout: &str) -> Option<MergerDecision> {
    let values: Vec<serde_json::Value> = match output_id {
        id if harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => {
            harness_stream::stream_values(stdout)
        }
        "raw-json" | "claude-json" => serde_json::from_str(stdout).into_iter().collect(),
        _ => Vec::new(),
    };
    values.iter().rev().find_map(|value| {
        harness_stream::find_nested_object(value, &mut decision_from_object, &mut |_| {})
    })
}

fn decision_from_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<MergerDecision> {
    match object.get("result").and_then(serde_json::Value::as_str)? {
        "proceed" => Some(MergerDecision::Proceed {
            deep_decisions: Vec::new(),
        }),
        "park" => Some(MergerDecision::Park {
            reason: object
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("merger requested park")
                .to_string(),
            deep_decisions: Vec::new(),
        }),
        _ => None,
    }
}

/// One P420 `--deep` decision-receipt entry as the deep merger's reply
/// literally names it, before validation converts it to a typed
/// [`DeepMergeDecision`] (or rejects it).
struct RawDeepDecision {
    path: String,
    hunk: String,
    rule: String,
    choice: String,
    rationale: String,
    supporting_edits: Vec<String>,
}

/// Extract the deep merger's top-level `result` (`"proceed"` or `"refuse"`),
/// its `reason` (required non-empty for `"refuse"`, otherwise ignored), and
/// its raw `decisions` array, using the same wrapper/array/embedded-JSON-
/// string traversal as [`parse_merge_decision`]. Every field of every array
/// entry is decoded strictly here: a missing, wrong-typed, or empty required
/// field (`path`, `hunk`, `rule`, `choice`, `rationale`) rejects the ENTIRE
/// receipt (returns `None`), rather than silently dropping just that one
/// malformed entry — a partially-decoded receipt must never be treated as
/// complete. `supporting-edits`, when present, must decode as an array whose
/// every member is a string — a malformed (non-string) array member rejects
/// the entire receipt exactly like a malformed required field.
fn deep_decision_from_object(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Option<(String, Option<String>, Vec<RawDeepDecision>)> {
    let result = object.get("result").and_then(serde_json::Value::as_str)?;
    if result != "proceed" && result != "refuse" {
        return None;
    }
    let decisions: &[serde_json::Value] = match object.get("decisions") {
        None => &[],
        Some(value) => value.as_array()?,
    };
    let mut raw = Vec::with_capacity(decisions.len());
    for entry in decisions {
        let entry = entry.as_object()?;
        let path = entry.get("path")?.as_str()?.to_string();
        let hunk = entry.get("hunk")?.as_str()?.to_string();
        let rule = entry.get("rule")?.as_str()?.to_string();
        let choice = entry.get("choice")?.as_str()?.to_string();
        let rationale = entry.get("rationale")?.as_str()?.to_string();
        if choice.trim().is_empty() || rationale.trim().is_empty() {
            return None;
        }
        let supporting_edits = match entry.get("supporting-edits") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(value) => {
                let array = value.as_array()?;
                let mut edits = Vec::with_capacity(array.len());
                for member in array {
                    edits.push(member.as_str()?.to_string());
                }
                edits
            }
        };
        raw.push(RawDeepDecision {
            path,
            hunk,
            rule,
            choice,
            rationale,
            supporting_edits,
        });
    }
    let reason = object
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    if result == "refuse" && reason.as_deref().unwrap_or("").trim().is_empty() {
        // A refusal without a stated, non-empty reason is itself a
        // malformed receipt: rule 4 always requires naming why.
        return None;
    }
    Some((result.to_string(), reason, raw))
}

fn deep_rule_from_str(value: &str) -> Option<DeepMergeRule> {
    match value {
        "preserve-landed-surface" => Some(DeepMergeRule::PreserveLandedSurface),
        "union-additive" => Some(DeepMergeRule::UnionAdditive),
        "unify-duplicate" => Some(DeepMergeRule::UnifyDuplicate),
        "refuse-regression" => Some(DeepMergeRule::RefuseRegression),
        "verify-factual-claim" => Some(DeepMergeRule::VerifyFactualClaim),
        _ => None,
    }
}

/// A repository-relative path is well-formed for a P420 decision receipt when
/// it is non-empty, not absolute, and does not walk up via `..` — the same
/// shape every other repository-relative path this command handles is
/// expected to have.
fn is_well_formed_relative_path(path: &str) -> bool {
    !path.trim().is_empty()
        && !path.starts_with('/')
        && !path.split('/').any(|segment| segment == "..")
}

/// Validate and convert a P420 `--deep` merger's raw decision receipt.
/// Returns `Err` with a PRECISE reason (treated as an unsuccessful
/// reconciliation — the caller parks with that reason verbatim) for a
/// malformed, missing, duplicate, absolute, or unobserved-path entry, or
/// when a `"proceed"` reply does not cover every supplied conflicted hunk
/// and every path [`ctx_traits_io::worktree::touched_paths_delta`] observed
/// the call actually touch (as some decision's primary path or declared
/// supporting edit). The reasons exist because two live incidents (one at
/// the P409 landing, another on 2026-07-25) parked on one generic sentence
/// that named neither the failing check nor the paths,
/// making every occurrence a hand-forensics session. A `"refuse"` reply
/// always maps to a park (rule 4) regardless of coverage, persisting
/// whatever decisions were already made.
fn parse_deep_merge_decision(
    output_id: &str,
    stdout: &str,
    hunks: &[DeepHunk],
    changed_paths: &[String],
) -> Result<MergerDecision, String> {
    let values: Vec<serde_json::Value> = match output_id {
        id if harness_stream::GENERIC_STREAM_JSON_OUTPUTS.contains(&id) => {
            harness_stream::stream_values(stdout)
        }
        "raw-json" | "claude-json" => serde_json::from_str(stdout).into_iter().collect(),
        _ => Vec::new(),
    };
    let Some((result, refuse_reason, raw)) = values.iter().rev().find_map(|value| {
        harness_stream::find_nested_object(value, &mut deep_decision_from_object, &mut |_| {})
    }) else {
        return Err(
            "no structured proceed/refuse JSON receipt object found in the merger output"
                .to_string(),
        );
    };

    let mut seen = std::collections::BTreeSet::new();
    let mut decisions = Vec::new();
    for entry in raw {
        if !is_well_formed_relative_path(&entry.path) {
            return Err(format!(
                "decision path is not a well-formed repo-relative path: {:?}",
                entry.path
            ));
        }
        for supporting in &entry.supporting_edits {
            if !is_well_formed_relative_path(supporting) {
                return Err(format!(
                    "supporting-edits entry is not a well-formed repo-relative path: {supporting:?}"
                ));
            }
            // A supporting edit must be an actually-observed touched path
            // (see `touched_paths_delta`) — never a fabricated path the
            // merger merely claims to have edited.
            if !changed_paths.iter().any(|path| path == supporting) {
                return Err(format!(
                    "supporting-edits entry {supporting:?} is not among the paths this call \
                     actually touched (observed: {})",
                    changed_paths.join(", ")
                ));
            }
        }
        let Some(rule) = deep_rule_from_str(&entry.rule) else {
            return Err(format!("unknown decision rule {:?}", entry.rule));
        };
        // Every decision entry must be tied to an actually-supplied
        // conflicted hunk — never a fabricated (path, hunk) pair the merger
        // invented.
        if !hunks
            .iter()
            .any(|hunk| hunk.path == entry.path && hunk.hunk_id == entry.hunk)
        {
            return Err(format!(
                "decision ({}, {}) matches no supplied conflicted hunk (supplied: {})",
                entry.path,
                entry.hunk,
                hunks
                    .iter()
                    .map(|hunk| format!("{}/{}", hunk.path, hunk.hunk_id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !seen.insert((entry.path.clone(), entry.hunk.clone())) {
            return Err(format!(
                "duplicate decision for ({}, {})",
                entry.path, entry.hunk
            ));
        }
        decisions.push(DeepMergeDecision {
            path: entry.path,
            hunk: entry.hunk,
            rule,
            choice: entry.choice,
            rationale: entry.rationale,
            supporting_edits: entry.supporting_edits,
        });
    }

    if result == "refuse" {
        // Rule 4 requires the refusal to be anchored to a real, supplied
        // conflict hunk decided under `refuse-regression` — an unanchored
        // `refuse` (no decisions, or none actually using the rule) is
        // treated as a malformed receipt, not a valid park.
        let has_refuse_regression_decision = decisions
            .iter()
            .any(|decision| decision.rule == DeepMergeRule::RefuseRegression);
        if !has_refuse_regression_decision {
            // Wording constraint: the deep-merge proof distinguishes a VALID
            // rule-4 park from this invalid-receipt park by the literal
            // "refuse-regression" appearing only in the former — keep that
            // token out of this message.
            return Err(
                "refuse reply carries no decision anchored to a supplied hunk under the \
                 refusal rule (rule 4) — an unanchored refuse is not a valid receipt"
                    .to_string(),
            );
        }
        return Ok(MergerDecision::Park {
            reason: format!(
                "{} (rule refuse-regression): {}",
                reasons::DEEP_REFUSE_REGRESSION_PREFIX,
                refuse_reason.unwrap_or_default()
            ),
            deep_decisions: decisions,
        });
    }

    let covers = |path: &str| -> bool {
        decisions
            .iter()
            .any(|d| d.path == path || d.supporting_edits.iter().any(|edit| edit == path))
    };
    let uncovered_hunks: Vec<String> = hunks
        .iter()
        .filter(|hunk| {
            !decisions
                .iter()
                .any(|d| d.path == hunk.path && d.hunk == hunk.hunk_id)
        })
        .map(|hunk| format!("{}/{}", hunk.path, hunk.hunk_id))
        .collect();
    if !uncovered_hunks.is_empty() {
        return Err(format!(
            "proceed reply leaves conflicted hunk(s) with no decision: {}",
            uncovered_hunks.join(", ")
        ));
    }
    let uncovered_paths: Vec<String> = changed_paths
        .iter()
        .filter(|path| !covers(path))
        .cloned()
        .collect();
    if !uncovered_paths.is_empty() {
        return Err(format!(
            "touched path(s) covered by no decision path or supporting-edits entry: {}",
            uncovered_paths.join(", ")
        ));
    }
    Ok(MergerDecision::Proceed {
        deep_decisions: decisions,
    })
}

/// Render one [`DeepMergeDecision`] as a single compact, deterministic line
/// suitable for a `Ctx-Merge-Decision` commit trailer value. Fields are
/// pipe-separated in a fixed order; any embedded newline or `|` in a
/// free-text field is replaced with a space so the trailer stays one line.
fn deep_decision_trailer_value(decision: &DeepMergeDecision) -> String {
    let sanitize = |text: &str| text.replace(['\n', '|'], " ");
    format!(
        "path={}|hunk={}|rule={}|choice={}|rationale={}{}",
        sanitize(&decision.path),
        sanitize(&decision.hunk),
        decision.rule.as_str(),
        sanitize(&decision.choice),
        sanitize(&decision.rationale),
        if decision.supporting_edits.is_empty() {
            String::new()
        } else {
            format!(
                "|supporting-edits={}",
                sanitize(&decision.supporting_edits.join(","))
            )
        }
    )
}

#[cfg(test)]
mod tests {
    use super::{
        MergeLive, ParkTarget, deep_hunk_id, gate_command_label, generated_artifact_index_for_path,
        park, park_with_detail, path_is_under_seed_root, trimmed_stderr_tail,
    };
    use ctx_traits_core::digest::Digest;
    use ctx_traits_core::procedure::activity::ActivityEvent;
    use ctx_traits_core::procedure::runtime::FinalState;
    use ctx_traits_core::procedure::session::MergeStage;
    use ctx_traits_io::harness_config::GeneratedArtifact;
    use std::sync::{Arc, Mutex};

    /// Minimal-but-valid [`Session`] for the park-helper tests below — every
    /// field the type requires (no `#[serde(default)]`), with every other
    /// field left at its `serde` default via a bare JSON object. Not a
    /// reusable test double: `ctx traits merge`'s own tests only ever need a
    /// session whose ledger frames can be appended to and read back.
    fn write_test_session(path: &camino::Utf8Path, run_id: &str) {
        let session = ctx_traits_core::procedure::session::Session {
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
            status: ctx_traits_core::procedure::session::Status::Completed,
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
                    caller: "merge-live-test".to_string(),
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
                started_at_epoch: None,
                trust_approval: None,
                session_title: None,
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
        };
        ctx_traits_io::run_session::write_run_session(path, &session).expect("write session");
    }

    fn scratch_session_path(name: &str) -> camino::Utf8PathBuf {
        let mut dir = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir");
        dir.push(format!(
            "ctx-traits-merge-live-test-{name}-{:?}",
            std::thread::current().id()
        ));
        dir
    }

    /// P549 blocker `parked-merge-invisible-live`: `park` (and, through it,
    /// every park/recovery helper) must never append a terminal ledger frame
    /// without also emitting the same frame to a present live sink — the
    /// choke-point invariant, exercised through the actual helper (not just
    /// the pure `merge_story` projection this already covers), with a real
    /// on-disk session round-trip.
    #[test]
    fn park_emits_the_same_terminal_frame_live_before_persisting() {
        let path = scratch_session_path("park");
        write_test_session(&path, "run-live-park");
        let events: Arc<Mutex<Vec<ActivityEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let live = MergeLive::new(move |event: ActivityEvent| {
            sink_events.lock().expect("lock").push(event);
        });

        let report = park(
            &path,
            "run-live-park",
            MergeStage::Gates,
            "post-run gate just-test failed: exit=Some(1)".to_string(),
            Vec::new(),
            &ctx_traits_io::worktree::RetryWarnings::new(),
            Some(&live),
        )
        .expect("park");
        assert_eq!(report.status, "parked");

        let recorded = events.lock().expect("lock");
        let terminal = recorded
            .iter()
            .find(|event| event.frame_id == "merge:gates")
            .expect("a live event for the gates frame");
        let persisted_session = ctx_traits_io::run_session::read_run_session(&path).expect("read");
        let persisted_frame = persisted_session
            .provenance
            .merge_frames
            .last()
            .expect("a persisted merge frame");
        assert_eq!(
            terminal.text.as_deref(),
            Some(
                crate::app::merge_story::explain_frame(persisted_frame)
                    .sentence
                    .as_str()
            ),
            "live text must equal explain_frame's sentence for the same persisted frame"
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Same invariant as above, but through [`park_with_detail`] directly —
    /// the helper every other park call site in this family (including
    /// `park` itself) ultimately funnels through — with no live sink
    /// present: must still persist correctly and never panic.
    #[test]
    fn park_with_detail_is_a_no_op_live_emission_without_a_sink() {
        let path = scratch_session_path("park-with-detail-no-live");
        write_test_session(&path, "run-no-live");

        let report = park_with_detail(
            ParkTarget {
                session_path: &path,
                run_id: "run-no-live",
            },
            MergeStage::Preflight,
            "some reason".to_string(),
            None,
            Vec::new(),
            &ctx_traits_io::worktree::RetryWarnings::new(),
            None,
        )
        .expect("park_with_detail");
        assert_eq!(report.status, "parked");

        let _ = std::fs::remove_file(&path);
    }

    /// P463 item A: a declared seed root matches a path under it, but a
    /// near-miss whose prefix is a string-superset of the declared root (not
    /// an actual path-separator boundary) must NOT match — same class of bug
    /// as `generated_matching_respects_path_separator_boundaries`.
    #[test]
    fn seed_root_containment_respects_path_separator_boundaries() {
        let roots = vec![".notes".to_string()];
        assert!(path_is_under_seed_root(".notes", &roots));
        assert!(path_is_under_seed_root(".notes/P463.md", &roots));
        assert!(!path_is_under_seed_root(".notes-extra/x", &roots));
        assert!(!path_is_under_seed_root(".note", &roots));
    }

    /// P463 item A: `deep_hunk_id`'s generalized signature is deterministic
    /// across repeated calls with the same inputs, distinct for a change to
    /// any one blob, and distinct across the two callers' `kind` tags even
    /// when every other input coincidentally matches — the reason `kind` was
    /// added to the seed rather than reusing the three-callers-share-a-scheme
    /// id space unqualified.
    #[test]
    fn deep_hunk_id_is_deterministic_and_kind_distinct() {
        let a = deep_hunk_id(
            "path",
            0,
            None,
            Some(b"ancestor"),
            Some(b"main"),
            Some(b"worktree"),
            "harvest",
        );
        let a_again = deep_hunk_id(
            "path",
            0,
            None,
            Some(b"ancestor"),
            Some(b"main"),
            Some(b"worktree"),
            "harvest",
        );
        assert_eq!(a, a_again);
        let different_blob = deep_hunk_id(
            "path",
            0,
            None,
            Some(b"ancestor"),
            Some(b"main-changed"),
            Some(b"worktree"),
            "harvest",
        );
        assert_ne!(a, different_blob);
        let different_kind = deep_hunk_id(
            "path",
            0,
            None,
            Some(b"ancestor"),
            Some(b"main"),
            Some(b"worktree"),
            "reconciliation",
        );
        assert_ne!(a, different_kind);
    }

    fn artifact(paths: &[&str]) -> GeneratedArtifact {
        GeneratedArtifact {
            paths: paths.iter().map(|path| path.to_string()).collect(),
            rebuild: vec![vec!["true".to_string()]],
        }
    }

    /// P463 item C: a declared directory prefix matches a path under it, but
    /// a near-miss whose prefix is a string-superset of the declared
    /// directory (not an actual path separator boundary) must NOT match —
    /// the 2026-07-25 incident (`scripts/golde` vs
    /// `scripts/goldens/drive.json`) is exactly this class of bug.
    #[test]
    fn generated_matching_respects_path_separator_boundaries() {
        let generated = vec![artifact(&["scripts/goldens"])];
        assert_eq!(
            generated_artifact_index_for_path(&generated, "scripts/goldens/drive.json"),
            Some(0)
        );
        assert_eq!(
            generated_artifact_index_for_path(&generated, "scripts/goldens"),
            Some(0)
        );
        assert_eq!(
            generated_artifact_index_for_path(&generated, "scripts/golde"),
            None
        );
        assert_eq!(
            generated_artifact_index_for_path(&generated, "scripts/goldens-extra/x"),
            None
        );
    }

    #[test]
    fn generated_matching_is_absent_by_default() {
        assert_eq!(generated_artifact_index_for_path(&[], "anything"), None);
    }

    #[test]
    fn trimmed_stderr_tail_takes_the_last_non_blank_line() {
        assert_eq!(
            trimmed_stderr_tail("first line\n\nlast real line\n\n"),
            "last real line"
        );
        assert_eq!(trimmed_stderr_tail("   \n\n"), "no stderr output");
    }

    /// A two-word command whose second word is a flag argument labels the
    /// same as the bare two-word form (P477: keeps a historically declared
    /// two-command chain's evidence/capture names stable, without the
    /// helper special-casing any one repository tool).
    #[test]
    fn generic_two_word_command_derives_a_stable_hyphenated_label() {
        assert_eq!(
            gate_command_label(0, &["word".to_string(), "--flag".to_string()]),
            "word-flag"
        );
        assert_eq!(
            gate_command_label(0, &["word".to_string(), "flag".to_string()]),
            "word-flag"
        );
    }

    /// A single bare command labels as itself.
    #[test]
    fn single_word_command_labels_as_itself() {
        assert_eq!(gate_command_label(0, &["false".to_string()]), "false");
    }

    /// An argv that sanitizes to nothing printable (every argument is
    /// entirely dashes, so every part is stripped away before the
    /// filesystem-safe filter even runs) falls back to an index-based label
    /// rather than an empty string.
    #[test]
    fn empty_after_sanitization_falls_back_to_index_label() {
        assert_eq!(gate_command_label(3, &["--".to_string()]), "gate-3");
        assert_eq!(gate_command_label(7, &[]), "gate-7");
    }

    /// A label longer than 40 characters is truncated at the ASCII
    /// boundary, never panicking on a multi-byte codepoint at the cut
    /// point.
    #[test]
    fn long_label_truncates_at_forty_ascii_chars() {
        let long_word = "a".repeat(50);
        let label = gate_command_label(0, &[long_word]);
        assert_eq!(label.len(), 40);
        assert!(label.chars().all(|ch| ch == 'a'));
    }

    /// Non-filesystem-safe characters (spaces, slashes, quotes) are
    /// stripped, never passed through into the label used to build a
    /// captured-output file path.
    #[test]
    fn unsafe_characters_are_stripped_from_the_label() {
        assert_eq!(
            gate_command_label(0, &["cmd/with space\"quote".to_string()]),
            "cmdwithspacequote"
        );
    }
}
