//! The dashboard's keyed model: a `ledger_path`-keyed map of the center's
//! wire rows, plus a projection cache rebuilt only when the map or scope
//! actually changes. Plain Rust — no gpui types — so it is unit-testable
//! without an `App`.
//!
//! The map, not a `Vec`, is the source of truth: a snapshot installs it
//! wholesale, and every later delta is folded in through
//! `CenterDelta::apply_to`, the same rule the CLI dashboard worker uses. That
//! shared rule is what keeps `Ended` (removal) and `RowChanged` (an in-place
//! update, including a completed run) from ever being conflated here.

use std::collections::HashMap;

use ctx_traits_io::center::{CenterDelta, CenterPublicRow};

use crate::run_row::{self, RepoScope, RunRow};

pub struct Dashboard {
    rows: HashMap<String, CenterPublicRow>,
    scope: RepoScope,
    projected: Vec<RunRow>,
}

impl Dashboard {
    pub fn from_snapshot(rows: Vec<CenterPublicRow>, scope: RepoScope) -> Self {
        let mut dashboard = Self {
            rows: HashMap::new(),
            scope,
            projected: Vec::new(),
        };
        dashboard.install_snapshot(rows);
        dashboard
    }

    /// Replace the model wholesale — a snapshot is coherent state, never a
    /// merge, so no row from before this call survives unless the new
    /// snapshot carries it too.
    pub fn install_snapshot(&mut self, rows: Vec<CenterPublicRow>) {
        self.rows = rows
            .into_iter()
            .map(|row| (row.ledger_path.clone(), row))
            .collect();
        self.reproject();
    }

    /// Fold one delta into the model. Returns whether the *visible*
    /// projection changed. `ActivityLine` never touches the row map. A
    /// no-op fold of the map — an `Appeared`/`RowChanged` replaying a row
    /// already present with identical content, or an `Ended` for a key
    /// that is not in the model — also reports no change and skips the
    /// reprojection. A real model change that the current `RepoScope`
    /// still hides is retained in the model but reports `false`, since
    /// nothing paints differently.
    pub fn apply(&mut self, delta: CenterDelta) -> bool {
        if matches!(delta, CenterDelta::ActivityLine { .. }) {
            return false;
        }
        let ledger_path = delta.ledger_path().to_string();
        let before = self.rows.get(&ledger_path).cloned();
        delta.apply_to(&mut self.rows);
        let after = self.rows.get(&ledger_path).cloned();
        if before == after {
            return false;
        }
        let previous_projection = std::mem::take(&mut self.projected);
        self.reproject();
        self.projected != previous_projection
    }

    pub fn set_scope(&mut self, scope: RepoScope) {
        self.scope = scope;
        self.reproject();
    }

    pub fn rows(&self) -> &[RunRow] {
        &self.projected
    }

    pub fn len(&self) -> usize {
        self.projected.len()
    }

    pub fn is_empty(&self) -> bool {
        self.projected.is_empty()
    }

    fn reproject(&mut self) {
        let values: Vec<CenterPublicRow> = self.rows.values().cloned().collect();
        self.projected = run_row::project(&values, &self.scope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::session::Status;
    use ctx_traits_io::run_summary::RunSummary;

    fn summary(run_id: &str, session_id: &str, status: Status) -> RunSummary {
        let mut summary = RunSummary::unreadable(session_id.to_string(), "unused".to_string());
        summary.run_id = run_id.to_string();
        summary.parse_error = None;
        summary.status = status;
        summary.trait_id = "fixture-trait".to_string();
        summary
    }

    fn row(
        repo_key: &str,
        ledger_path: &str,
        run_id: &str,
        status: Status,
        live: bool,
        modified_epoch_secs: u64,
    ) -> CenterPublicRow {
        CenterPublicRow {
            summary: summary(run_id, run_id, status),
            repo_key: repo_key.to_string(),
            repo_path: format!("/{repo_key}"),
            ledger_path: ledger_path.to_string(),
            live,
            modified_epoch_secs,
        }
    }

    fn boxed(row: CenterPublicRow) -> Box<CenterPublicRow> {
        Box::new(row)
    }

    #[test]
    fn ordered_snapshot_then_deltas_leave_no_loss_and_no_duplicate() {
        let snapshot = vec![
            row("repo", "/a.json", "a", Status::Completed, false, 1),
            row("repo", "/b.json", "b", Status::Completed, false, 2),
        ];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);

        assert!(dashboard.apply(CenterDelta::Appeared {
            row: boxed(row("repo", "/c.json", "c", Status::Completed, true, 3)),
        }));
        assert!(dashboard.apply(CenterDelta::RowChanged {
            row: boxed(row(
                "repo",
                "/a.json",
                "a-updated",
                Status::Completed,
                false,
                4
            )),
        }));
        assert!(dashboard.apply(CenterDelta::Ended {
            row: boxed(row("repo", "/b.json", "b", Status::Completed, false, 2)),
        }));

        let ledger_paths: Vec<_> = dashboard
            .rows()
            .iter()
            .map(|r| r.ledger_path.as_str())
            .collect();
        assert_eq!(dashboard.len(), 2);
        assert_eq!(ledger_paths, vec!["/c.json", "/a.json"]);
        assert_eq!(
            dashboard
                .rows()
                .iter()
                .find(|r| r.ledger_path == "/a.json")
                .unwrap()
                .run_id,
            "a-updated"
        );
    }

    #[test]
    fn a_delta_replaying_a_snapshot_row_updates_in_place() {
        let snapshot = vec![row("repo", "/a.json", "a", Status::Completed, false, 1)];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);

        let changed = dashboard.apply(CenterDelta::Appeared {
            row: boxed(row("repo", "/a.json", "a", Status::Completed, false, 1)),
        });

        assert_eq!(dashboard.len(), 1);
        assert!(
            !changed,
            "an identical replay is a map no-op and must not report a change"
        );
    }

    #[test]
    fn identical_replay_and_absent_key_ended_report_no_change() {
        let snapshot = vec![row("repo", "/a.json", "a", Status::Completed, false, 1)];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);
        let before = dashboard.rows().to_vec();

        assert!(!dashboard.apply(CenterDelta::RowChanged {
            row: boxed(row("repo", "/a.json", "a", Status::Completed, false, 1)),
        }));
        assert!(!dashboard.apply(CenterDelta::Ended {
            row: boxed(row(
                "repo",
                "/missing.json",
                "missing",
                Status::Completed,
                false,
                9
            )),
        }));

        assert_eq!(dashboard.rows(), before.as_slice());
    }

    #[test]
    fn a_visible_insert_update_and_removal_each_report_a_change() {
        let snapshot = vec![row("repo", "/a.json", "a", Status::Completed, false, 1)];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);

        assert!(dashboard.apply(CenterDelta::Appeared {
            row: boxed(row("repo", "/b.json", "b", Status::Completed, true, 2)),
        }));
        assert!(dashboard.apply(CenterDelta::RowChanged {
            row: boxed(row(
                "repo",
                "/a.json",
                "a-updated",
                Status::Completed,
                false,
                3
            )),
        }));
        assert!(dashboard.apply(CenterDelta::Ended {
            row: boxed(row("repo", "/b.json", "b", Status::Completed, false, 2)),
        }));
    }

    #[test]
    fn ended_removes_but_row_changed_retains_a_completed_run() {
        let snapshot = vec![
            row("repo", "/a.json", "a", Status::Completed, true, 1),
            row("repo", "/b.json", "b", Status::Completed, true, 2),
        ];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);

        dashboard.apply(CenterDelta::RowChanged {
            row: boxed(row("repo", "/a.json", "a", Status::Completed, false, 3)),
        });
        dashboard.apply(CenterDelta::Ended {
            row: boxed(row("repo", "/b.json", "b", Status::Completed, false, 2)),
        });

        assert_eq!(dashboard.len(), 1);
        let remaining = &dashboard.rows()[0];
        assert_eq!(remaining.ledger_path, "/a.json");
        assert!(!remaining.live);
    }

    #[test]
    fn activity_line_returns_false_and_leaves_the_projection_byte_identical() {
        let snapshot = vec![row("repo", "/a.json", "a", Status::Completed, true, 1)];
        let mut dashboard = Dashboard::from_snapshot(snapshot, RepoScope::All);
        let before = dashboard.rows().to_vec();

        let changed = dashboard.apply(CenterDelta::ActivityLine {
            row: boxed(row("repo", "/a.json", "a", Status::Completed, true, 1)),
            activity: ctx_traits_io::activity_sidecar::ActivityRecord::SessionTitle {
                at_epoch_ms: 1,
                title: "activity".to_string(),
            },
        });

        assert!(!changed);
        assert_eq!(dashboard.rows(), before.as_slice());
    }

    #[test]
    fn repository_identity_survives_an_update_and_scoping_hides_then_reveals_it() {
        let snapshot = vec![
            row("repo-a", "/a.json", "a", Status::Completed, false, 1),
            row("repo-b", "/b.json", "b", Status::Completed, false, 2),
        ];
        let mut dashboard =
            Dashboard::from_snapshot(snapshot, RepoScope::Repo("repo-a".to_string()));

        let changed = dashboard.apply(CenterDelta::RowChanged {
            row: boxed(row(
                "repo-b",
                "/b.json",
                "b-updated",
                Status::Completed,
                false,
                3,
            )),
        });

        // The update is real and retained in the model, but the visible
        // projection under the current scope is unchanged, so it reports
        // false.
        assert!(!changed);
        assert_eq!(dashboard.len(), 1);
        assert_eq!(dashboard.rows()[0].repo_key, "repo-a");

        dashboard.set_scope(RepoScope::All);
        assert_eq!(dashboard.len(), 2);
        assert!(dashboard.rows().iter().any(|r| r.run_id == "b-updated"));
    }

    #[test]
    fn a_snapshot_install_after_existing_state_replaces_rather_than_merges() {
        let mut dashboard = Dashboard::from_snapshot(
            vec![row(
                "repo",
                "/stale.json",
                "stale",
                Status::Completed,
                false,
                1,
            )],
            RepoScope::All,
        );
        dashboard.install_snapshot(vec![row(
            "repo",
            "/fresh.json",
            "fresh",
            Status::Completed,
            false,
            2,
        )]);

        assert_eq!(dashboard.len(), 1);
        assert_eq!(dashboard.rows()[0].ledger_path, "/fresh.json");
    }
}
