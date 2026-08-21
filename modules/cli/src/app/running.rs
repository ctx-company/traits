//! `ctx traits internal running`: the machine-wide "what is running" read
//! — one small liveness-index file read plus at most one bounded `flock`
//! probe per indexed row, never a scan of every ledger this machine has.

use ctx_traits_core::response::CommandOutput;
use ctx_traits_io::run_liveness::{LiveIndexRow, Liveness};

use crate::app::command_handlers::print_json_report;
use crate::app::tui::write_plain_line as w;

#[derive(serde::Serialize)]
struct RunningRow {
    session: String,
    run_id: String,
    repo_key: String,
    repo_path: String,
    worktree_path: Option<String>,
    branch: Option<String>,
    pid: u32,
    started_at_epoch: u64,
    log_path: Option<String>,
    verdict: String,
}

/// JSON shape for `ctx traits internal running --json`: `available: false` is a
/// distinct value from `rows: []`, so a machine reader can never confuse
/// "the liveness index is unavailable" with "nothing is running".
/// contract (a)).
#[derive(serde::Serialize)]
struct RunningReport {
    available: bool,
    rows: Vec<RunningRow>,
}

fn row_from(row: LiveIndexRow) -> RunningRow {
    RunningRow {
        // JSON is an identity-bearing machine contract; presentation shortens
        // this only at the line-mode writer below.
        session: row.session_id,
        run_id: row.run_id,
        repo_key: row.repo_key,
        repo_path: row.repo_path,
        worktree_path: row.worktree_path,
        branch: row.branch,
        pid: row.pid,
        started_at_epoch: row.started_at_epoch,
        log_path: row.log_path,
        verdict: String::new(),
    }
}

pub(crate) fn handle_running(json: bool) -> crate::Result<CommandOutput<()>> {
    let root = ctx_traits_io::run_control::runtime_root();
    let report = ctx_traits_io::run_liveness::liveness_report(&root);
    // The sentinel [`Liveness::Unknown`] entry `liveness_report` returns on
    // an unavailable root has no row and must never be read as "zero rows":
    // that is exactly the false not-running answer contract (a) forbids.
    let available = !matches!(report.as_slice(), [(None, Liveness::Unknown)]);
    let all_ids: Vec<String> = report
        .iter()
        .filter_map(|(row, _)| row.as_ref().map(|row| row.session_id.clone()))
        .collect();

    let mut rows = Vec::with_capacity(report.len());
    for (row, liveness) in report {
        let verdict = match &liveness {
            Liveness::Live { .. } => "live",
            Liveness::Orphan { .. } => "orphan",
            Liveness::Adopted => "live (adopted)",
            Liveness::NotRunning => "not-running",
            Liveness::Unknown => "unknown",
        };
        let Some(row) = row else { continue };
        let mut running_row = row_from(row);
        running_row.verdict = verdict.to_string();
        rows.push(running_row);
    }

    if json {
        print_json_report(&RunningReport { available, rows }, "running report")?;
        return Ok(CommandOutput::new(()));
    }

    if !available {
        w(
            "ctx traits internal running: local liveness index is unavailable (unknown, not \"nothing running\")",
        )?;
        return Ok(CommandOutput::new(()));
    }
    if rows.is_empty() {
        w("ctx traits internal running: no rows in the local liveness index")?;
        return Ok(CommandOutput::new(()));
    }
    w("ctx traits internal running")?;
    for row in &rows {
        w(format!(
            "  {} [{}]",
            ctx_traits_io::run_session::short_session_display(&row.session, &all_ids),
            row.verdict
        ))?;
        w(format!("    repo: {} ({})", row.repo_key, row.repo_path))?;
        if let Some(worktree_path) = &row.worktree_path {
            w(format!(
                "    worktree: {} (branch: {})",
                worktree_path,
                row.branch.as_deref().unwrap_or("-")
            ))?;
        }
        w(format!(
            "    pid: {} started-at-epoch: {}",
            row.pid, row.started_at_epoch
        ))?;
        if let Some(log_path) = &row.log_path {
            w(format!("    log: {log_path}"))?;
        }
    }
    Ok(CommandOutput::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_row_keeps_the_full_session_identity() {
        let session_id = format!("session-{}", "a".repeat(64));
        let row = row_from(LiveIndexRow {
            session_id: session_id.clone(),
            run_id: "run".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            ledger_path: "/repo/ledger.json".to_string(),
            worktree_path: None,
            branch: None,
            log_path: None,
            started_at_epoch: 0,
            pid: 1,
        });
        assert_eq!(row.session, session_id);
    }
}
