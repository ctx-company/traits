//! The one destructive session-artifact-deletion path (0252.5), extracted
//! out of `dashboard.rs` so the dashboard's row DELETE/MERGES DROP, the
//! dashboard's group DELETE, and `ctx traits sessions delete --failed` share
//! exactly one implementation. Eligibility itself stays in `dashboard.rs`
//! (it reads dashboard-private `SessionClass`/`MergeClass`), so
//! [`execute_confirmed_delete`] takes eligibility as a predicate rather than
//! a concrete type.

use ctx_traits_core::response::CommandOutput;

use crate::app::presentation::{
    HumanOutputMode, Panel, PanelRow, PanelStatus, RowTone, emit_human,
};

/// The exact artifact list a DELETE confirms before touching anything, and a
/// pure function of already-resolved inputs: the caller does the git probing
/// ([`plan_delete_for_ledger`]) and passes the resolved worktree location in,
/// so this function itself does no IO and is directly unit-testable.
#[derive(Clone)]
pub(crate) struct DeletePlan {
    pub(crate) ledger_path: camino::Utf8PathBuf,
    pub(crate) driver_lock_path: Option<camino::Utf8PathBuf>,
    pub(crate) sidecars_root: Option<camino::Utf8PathBuf>,
    /// `(repo_root, worktree_path, branch)`, present only when provenance
    /// named a worktree AND its registration was verified in the current
    /// repository.
    pub(crate) worktree: Option<(camino::Utf8PathBuf, camino::Utf8PathBuf, String)>,
    /// Explains why a provenance-named worktree/branch is NOT in `worktree`
    /// above (foreign repository, or registration failed) — rendered
    /// verbatim in the confirm modal so it never implies a cleaner sweep
    /// than what will actually happen.
    pub(crate) worktree_note: Option<String>,
}

impl DeletePlan {
    pub(crate) fn artifact_lines(&self) -> Vec<String> {
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

/// Resolves a DELETE/DROP plan's worktree location with exactly one git
/// probe, then hands off to the pure [`plan_delete`] to build the artifact
/// list. Narrowed to `(ledger_path, repo_path)` rather than `&SessionRow` so
/// both SESSIONS' DELETE and MERGES' DROP (and the CLI sweep) call it — a
/// genuine extraction, not a copy.
pub(crate) fn plan_delete_for_ledger(
    ledger_path: &camino::Utf8Path,
    repo_path: Option<&str>,
    worktree: Option<&ctx_traits_core::procedure::session::WorktreeProvenance>,
) -> DeletePlan {
    let driver_lock = ctx_traits_io::run_control::driver_lock_path(ledger_path);
    let driver_lock_path = driver_lock.as_std_path().exists().then_some(driver_lock);
    let sidecars = ctx_traits_io::run_branch::sidecars_root(ledger_path);
    let sidecars_root = sidecars.as_std_path().exists().then_some(sidecars);
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

/// Pure artifact-list builder (test-covered): `worktree_provenance` is
/// `None` when the ledger names no worktree; `verified` is the resolved
/// `(repo_root, worktree_path)` only when `same_repo` AND registration was
/// confirmed. No IO — every input is already resolved by the caller.
pub(crate) fn plan_delete(
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

/// A confirmed, executed delete's typed report. `execution_residue` (an
/// artifact that failed to remove) is a failure tier; `worktree_note`
/// (plan-time residue — a worktree/branch already known to be left behind
/// before execution ever ran) is reported but never a failure. Retained lock
/// inode is never residue of either tier.
pub(crate) struct DeleteReport {
    /// True only on a successful `remove_file` of the ledger.
    pub(crate) removed_ledger: bool,
    /// Today's message list, same strings, same order. [`Self::render`]
    /// joins them with `"; "`.
    pub(crate) messages: Vec<String>,
    /// EXECUTION residue: the `worktree … left in place`, `branch … left in
    /// place`, `ledger left in place`, `sidecars left in place` lines.
    pub(crate) execution_residue: Vec<String>,
    /// PLAN-TIME residue: the executed (refreshed) plan's `worktree_note`.
    pub(crate) worktree_note: Option<String>,
}

impl DeleteReport {
    pub(crate) fn render(&self) -> String {
        self.messages.join("; ")
    }
}

/// Executes a confirmed [`DeletePlan`]: `remove_worktree` then `delete_branch`
/// (which uses `-d`, so git itself refuses an unmerged branch rather than
/// this code forcing `-D`), then the ledger and its known-orphaned siblings.
/// Reports every artifact's own outcome so a partial failure never claims
/// more than actually happened.
pub(crate) fn execute_delete(plan: &DeletePlan) -> DeleteReport {
    let mut messages = Vec::new();
    let mut execution_residue = Vec::new();
    if let Some((repo_root, path, branch)) = &plan.worktree {
        let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
        match ctx_traits_io::worktree::remove_worktree(repo_root, path, &mut warnings) {
            Ok(()) => {
                messages.push(format!("worktree {path} removed"));
                let mut warnings = ctx_traits_io::worktree::RetryWarnings::new();
                match ctx_traits_io::worktree::delete_branch(repo_root, branch, &mut warnings) {
                    Ok(()) => messages.push(format!("branch {branch} deleted")),
                    Err(error) => {
                        let line = format!("branch {branch} left in place: {error}");
                        messages.push(line.clone());
                        execution_residue.push(line);
                    }
                }
            }
            Err(error) => {
                let worktree_line = format!("worktree {path} left in place: {error}");
                let branch_line =
                    format!("branch {branch} left in place (worktree removal failed)");
                messages.push(worktree_line.clone());
                messages.push(branch_line.clone());
                execution_residue.push(worktree_line);
                execution_residue.push(branch_line);
            }
        }
    }
    let mut removed_ledger = false;
    match std::fs::remove_file(plan.ledger_path.as_std_path()) {
        Ok(()) => {
            messages.push("ledger deleted".to_string());
            removed_ledger = true;
        }
        Err(error) => {
            let line = format!("ledger left in place: {error}");
            messages.push(line.clone());
            execution_residue.push(line);
        }
    }
    // Keep the lock file's inode stable. Removing it while holding flock would
    // let a new driver lock a replacement inode before this guard drops.
    if plan.driver_lock_path.is_some() {
        messages.push("driver lock retained (stable lock inode)".to_string());
    }
    if let Some(root) = &plan.sidecars_root {
        match std::fs::remove_dir_all(root.as_std_path()) {
            Ok(()) => messages.push("sidecars deleted".to_string()),
            Err(error) => {
                let line = format!("sidecars left in place: {error}");
                messages.push(line.clone());
                execution_residue.push(line);
            }
        }
    }
    ctx_traits_io::activity_sidecar::remove_activity_for_ledger(&plan.ledger_path);
    DeleteReport {
        removed_ledger,
        messages,
        execution_residue,
        worktree_note: plan.worktree_note.clone(),
    }
}

/// A confirmed delete's outcome, keeping every landed refusal string
/// byte-identical to what `dashboard.rs` reported before this extraction.
pub(crate) enum ConfirmedDelete {
    DriverHeld,
    SessionChanged,
    EligibilityChanged,
    PlanChanged,
    Executed(DeleteReport),
}

impl ConfirmedDelete {
    pub(crate) fn render(&self) -> String {
        match self {
            Self::DriverHeld => "delete refused: driver lock is now held".to_string(),
            Self::SessionChanged => "delete refused: session changed; reopen it".to_string(),
            Self::EligibilityChanged => {
                "delete refused: session eligibility changed; reopen it".to_string()
            }
            Self::PlanChanged => {
                "delete plan changed; review the refreshed artifact list".to_string()
            }
            Self::Executed(report) => report.render(),
        }
    }
}

/// Rebuild a destructive plan while maintenance ownership prevents a driver
/// from replacing its ledger. The modal's plan must still describe the same
/// artifacts; otherwise the caller must review the newly authoritative plan.
/// `eligible`/`allows_unreadable_ledger` stand in for `dashboard.rs`'s own
/// `DeleteEligibility`, which this module cannot depend on directly.
pub(crate) fn execute_confirmed_delete(
    ledger_path: &camino::Utf8Path,
    repo_path: Option<&str>,
    session_id: &str,
    eligible: impl Fn(&ctx_traits_core::procedure::session::Session) -> bool,
    allows_unreadable_ledger: bool,
    confirmed: &DeletePlan,
) -> crate::Result<ConfirmedDelete> {
    let Some(mut maintenance) = ctx_traits_io::run_control::try_acquire_maintenance(ledger_path)?
    else {
        return Ok(ConfirmedDelete::DriverHeld);
    };
    let current = match ctx_traits_io::run_session::read_run_session(ledger_path) {
        Ok(session) => session,
        // Only a modal opened on an unreadable row (or a plan that otherwise
        // declares itself unreadable-tolerant) may use the known-artifacts-
        // only branch. A later center projection must not relax the readable
        // modal's identity and eligibility checks.
        Err(_) if allows_unreadable_ledger => {
            let mut refreshed = plan_delete_for_ledger(ledger_path, repo_path, None);
            // Acquiring maintenance may have created the otherwise absent lock
            // file. It is our own stable inode, not a newly discovered user
            // artifact, in this known-artifacts-only branch as well.
            if confirmed.driver_lock_path.is_none() {
                refreshed.driver_lock_path = None;
            }
            // An unreadable ledger cannot verify identity or reconstruct
            // provenance. It may only delete the exact artifacts the caller
            // saw when confirming; newly discovered paths require a new
            // review.
            if refreshed.artifact_lines() != confirmed.artifact_lines() {
                return Ok(ConfirmedDelete::PlanChanged);
            }
            maintenance.clear_stale_metadata()?;
            return Ok(ConfirmedDelete::Executed(execute_delete(&refreshed)));
        }
        Err(error) => return Err(error.into()),
    };
    if current.session_id.as_str() != session_id {
        return Ok(ConfirmedDelete::SessionChanged);
    }
    if !eligible(&current) {
        return Ok(ConfirmedDelete::EligibilityChanged);
    }
    let mut refreshed =
        plan_delete_for_ledger(ledger_path, repo_path, current.provenance.worktree.as_ref());
    // Acquiring maintenance may have created the otherwise absent lock file.
    // It is our own stable inode, not a newly discovered user artifact.
    if confirmed.driver_lock_path.is_none() {
        refreshed.driver_lock_path = None;
    }
    if refreshed.artifact_lines() != confirmed.artifact_lines() {
        return Ok(ConfirmedDelete::PlanChanged);
    }
    maintenance.clear_stale_metadata()?;
    Ok(ConfirmedDelete::Executed(execute_delete(&refreshed)))
}

/// `ctx traits sessions delete --failed`'s bulk-selection report. Counts
/// sessions, not artifacts. Every skip/refusal/note names its session.
pub(crate) struct SweepReport {
    /// Ledgers actually gone.
    pub(crate) deleted: usize,
    /// One line per live-skipped session (probe-held, plus the TOCTOU close
    /// where maintenance acquisition lost the race to a driver that started
    /// between the probe and the confirm), naming it (decision 9) — printed
    /// to stderr. The panel's `skipped (live)` count is this vector's length,
    /// not a separately maintained counter — one source of truth for both.
    pub(crate) skips: Vec<String>,
    /// One line per failed session: a probe error, an unexpected outcome, or
    /// execution residue (several residue lines for one session join into a
    /// single entry).
    pub(crate) failures: Vec<String>,
    /// One line per session whose plan carried plan-time residue
    /// (`worktree_note`) — reported, never a failure.
    pub(crate) notes: Vec<String>,
}

/// Sweeps `store` for every ledger whose coarse status is exactly
/// `Status::Failed` and deletes it through the shared destructive path. An
/// unreadable ledger is never selected — it is silently out of scope, not a
/// failure. Parameterized by store path so selection is unit-testable
/// against a scratch directory; [`handle_sessions_delete`] supplies the
/// production default.
pub(crate) fn sweep_failed_in_store(store: &camino::Utf8Path) -> crate::Result<SweepReport> {
    let mut report = SweepReport {
        deleted: 0,
        skips: Vec::new(),
        failures: Vec::new(),
        notes: Vec::new(),
    };
    for path in ctx_traits_io::run_session::session_store_paths(Some(store.as_str()))? {
        let session = match ctx_traits_io::run_session::read_run_session(&path) {
            Ok(session) => session,
            Err(_) => continue,
        };
        if session.status != ctx_traits_core::procedure::session::Status::Failed {
            continue;
        }
        let session_id = session.session_id.as_str().to_string();
        match ctx_traits_io::run_control::probe(&path) {
            Ok(ctx_traits_io::run_control::DriverProbe::Held(_)) => {
                report
                    .skips
                    .push(format!("{session_id}: skipped, driver lock is held"));
                continue;
            }
            Ok(ctx_traits_io::run_control::DriverProbe::Unheld { .. }) => {}
            Err(error) => {
                report.failures.push(format!(
                    "{session_id}: could not probe driver lock: {error}"
                ));
                continue;
            }
        }
        let plan = plan_delete_for_ledger(&path, None, session.provenance.worktree.as_ref());
        let outcome = execute_confirmed_delete(
            &path,
            None,
            &session_id,
            |s| s.status == ctx_traits_core::procedure::session::Status::Failed,
            false,
            &plan,
        );
        match outcome {
            Ok(ConfirmedDelete::DriverHeld) => {
                report
                    .skips
                    .push(format!("{session_id}: skipped, driver lock is now held"));
            }
            Ok(ConfirmedDelete::Executed(delete_report)) => {
                if delete_report.removed_ledger {
                    report.deleted += 1;
                }
                if !delete_report.execution_residue.is_empty() {
                    report.failures.push(format!(
                        "{session_id}: {}",
                        delete_report.execution_residue.join("; ")
                    ));
                }
                if let Some(note) = delete_report.worktree_note {
                    report.notes.push(format!("{session_id}: {note}"));
                }
            }
            Ok(other) => report
                .failures
                .push(format!("{session_id}: {}", other.render())),
            Err(error) => report.failures.push(format!("{session_id}: {error}")),
        }
    }
    Ok(report)
}

/// `ctx traits sessions delete --failed`: sweeps this repository's run-
/// session store, deletes every exact `Status::Failed` ledger with its
/// worktree/branch/sidecars, and closes with exactly one compact panel
/// (`deleted`, plus `skipped (live)` only when nonzero). Refusal/residue
/// detail goes to stderr, never a third panel row.
pub(crate) fn handle_sessions_delete(failed: bool) -> crate::Result<CommandOutput<()>> {
    // The CLI's required Clap selector group makes this unreachable in
    // practice; the guard keeps it honest if a later selector changes the
    // parser shape.
    if !failed {
        return Err(crate::Error::Command {
            message: "sessions delete needs a selector; pass --failed".to_string(),
        });
    }
    let store = ctx_traits_io::run_session::default_session_store()?;
    let report = sweep_failed_in_store(&store)?;
    for skip in &report.skips {
        eprintln!("{skip}");
    }
    for note in &report.notes {
        eprintln!("{note}");
    }
    for failure in &report.failures {
        eprintln!("{failure}");
    }
    let (tone, status) = if report.failures.is_empty() {
        (RowTone::Default, PanelStatus::Passed("passed".to_string()))
    } else {
        (RowTone::Fail, PanelStatus::Blocked("blocked".to_string()))
    };
    let mut panel = Panel::new("ctx", "sessions delete", status).row(PanelRow::toned(
        "deleted",
        format!("{} session(s)", report.deleted),
        tone,
    ));
    if !report.skips.is_empty() {
        panel = panel.row(PanelRow::toned(
            "skipped (live)",
            format!("{} session(s)", report.skips.len()),
            tone,
        ));
    }
    emit_human(false, &panel, HumanOutputMode::Compact, || Ok(()))?;
    if !report.failures.is_empty() {
        return Err(crate::Error::AlreadyReported {
            message: format!(
                "{} session(s) refused or left artifacts behind",
                report.failures.len()
            ),
            exit_code: 1,
        });
    }
    Ok(CommandOutput::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session(
        path: &camino::Utf8Path,
        session_id: &str,
        status: ctx_traits_core::procedure::session::Status,
    ) {
        let status_word = match status {
            ctx_traits_core::procedure::session::Status::Failed => "failed",
            ctx_traits_core::procedure::session::Status::Completed => "completed",
            ctx_traits_core::procedure::session::Status::Rejected => "rejected",
            other => unreachable!("test fixture status not covered: {other:?}"),
        };
        let session: ctx_traits_core::procedure::session::Session =
            serde_json::from_value(serde_json::json!({
                "schema-version": "0.1.0",
                "session-id": session_id,
                "run-id": format!("run-{session_id}"),
                "trait-id": "session-delete-fixture",
                "current-run-index": 0,
                "status": status_word,
                "provenance": {
                    "started-by": {"surface": "test", "caller": "session-delete-proof"},
                    "state-source": "test",
                    "started-at-epoch": 1000,
                },
                "ledger": {
                    "run-id": format!("run-{session_id}"),
                    "trait-id": "session-delete-fixture",
                    "current-run-index": 0,
                    "final-state": status_word,
                },
                "state-digest": "sha256:session-delete-fixture",
            }))
            .expect("fixture session");
        ctx_traits_io::run_session::write_run_session(path, &session).expect("write fixture");
    }

    #[test]
    fn sweep_selects_only_exact_failed_ledgers() {
        let root = camino::Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("UTF-8 temp dir")
            .join(format!("ctx-session-delete-sweep-{}", std::process::id()));
        std::fs::create_dir_all(root.as_std_path()).expect("scratch store");

        write_session(
            &root.join("failed.json"),
            "session-failed",
            ctx_traits_core::procedure::session::Status::Failed,
        );
        write_session(
            &root.join("completed.json"),
            "session-completed",
            ctx_traits_core::procedure::session::Status::Completed,
        );
        write_session(
            &root.join("rejected.json"),
            "session-rejected",
            ctx_traits_core::procedure::session::Status::Rejected,
        );
        std::fs::write(root.join("corrupt.json").as_std_path(), "not json")
            .expect("corrupt fixture");

        let report = sweep_failed_in_store(&root).expect("sweep succeeds");

        assert_eq!(report.deleted, 1, "{:#?}", report.failures);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(!root.join("failed.json").exists());
        assert!(root.join("completed.json").exists());
        assert!(root.join("rejected.json").exists());
        assert!(root.join("corrupt.json").exists());

        let _ = std::fs::remove_dir_all(root.as_std_path());
    }
}
