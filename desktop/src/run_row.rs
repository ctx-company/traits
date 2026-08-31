//! Pure projection from the center's wire snapshot (`CenterPublicRow`) to a
//! presentation-ready run row. No IO, no `std::fs`, no env lookup, no
//! time-of-day read: everything here is a function of its `&[CenterPublicRow]`
//! argument and the caller-supplied [`RepoScope`]. That purity is what keeps
//! this module from ever resolving a repository relative to the desktop
//! process's own working directory — a scoped view is a filter over identity
//! that already arrived on the wire, never a discovery.
//!
//! `elapsed_text` mirrors `ctx_traits_cli`'s `app::tui::elapsed_text`, and
//! `tokens_text` mirrors `app::dashboard::dashboard_tokens_text_from_summary`
//! / `dashboard_token_value`. Both are `pub(crate)` inside `ctx-traits-cli`,
//! which the desktop must not depend on, so the ~30 lines below are a
//! deliberate, documented duplication rather than a shared extraction.

use ctx_traits_core::procedure::activity::SessionState;
use ctx_traits_core::procedure::session::DriveOutcomeKind;
use ctx_traits_core::task::TaskStatus;
use ctx_traits_io::center::CenterPublicRow;

/// Which repositories a projected list should include. Driven only by
/// `repo_key` values that arrived over the wire — never by the desktop
/// process's own working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoScope {
    All,
    Repo(String),
}

/// A run's actionable state, re-derived from the row's own `live` flag rather
/// than trusted from `summary.session_state` (which is always computed with
/// `live = false` at ledger-projection time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowState {
    Live,
    Paused,
    Resumable(SessionState),
    Terminal(SessionState),
    Unreadable,
}

/// `grammar.md` rule 1's colour roles for a state word. `Neutral` is not one
/// of rule 1's named roles — it is this projection's answer for the two
/// `SessionState` variants rule 1 does not assign a role to (see
/// [`session_state_presentation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateRole {
    Accent,
    Ok,
    Warn,
    Danger,
    Neutral,
}

/// A state word paired with the colour role it renders in — the bottom bar's
/// left-hand presentation of a [`RowState`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatePresentation {
    pub word: &'static str,
    pub role: StateRole,
}

/// The one role → token mapping, shared by the bottom bar (`bottom_bar_view`)
/// and the frame list (`frame_list_view`) — moved here beside [`StateRole`]
/// when it gained its second consumer in `0265.5`.
pub fn role_color(role: StateRole) -> u32 {
    match role {
        StateRole::Accent => crate::tokens::ACCENT,
        StateRole::Ok => crate::tokens::OK,
        StateRole::Warn => crate::tokens::WARN,
        StateRole::Danger => crate::tokens::DANGER,
        StateRole::Neutral => crate::tokens::TEXT,
    }
}

/// Exhaustive over all seven [`SessionState`] variants, no catch-all arm.
/// `Cancelled` and `WaitingOnAgent` are judgement calls: rule 1 gives no role
/// to "terminal but not a failure" or "in flight but not driven", so both
/// render `Neutral` rather than borrowing a role rule 1 reserves for
/// something else.
pub fn session_state_presentation(state: SessionState) -> StatePresentation {
    match state {
        SessionState::Running => StatePresentation {
            word: "running",
            role: StateRole::Accent,
        },
        SessionState::WaitingOnHuman => StatePresentation {
            word: "awaiting owner",
            role: StateRole::Warn,
        },
        SessionState::Blocked => StatePresentation {
            word: "blocked",
            role: StateRole::Warn,
        },
        SessionState::Completed => StatePresentation {
            word: "complete",
            role: StateRole::Ok,
        },
        SessionState::Failed => StatePresentation {
            word: "failed",
            role: StateRole::Danger,
        },
        SessionState::Cancelled => StatePresentation {
            word: "cancelled",
            role: StateRole::Neutral,
        },
        SessionState::WaitingOnAgent => StatePresentation {
            word: "awaiting agent",
            role: StateRole::Neutral,
        },
    }
}

/// Exhaustive over all five [`RowState`] variants, no catch-all arm. `Live`
/// and `Unreadable` are not `SessionState` values, so they get their own
/// arms rather than being forced through [`session_state_presentation`].
pub fn presentation(state: &RowState) -> StatePresentation {
    match state {
        RowState::Live => StatePresentation {
            word: "running",
            role: StateRole::Accent,
        },
        RowState::Paused => StatePresentation {
            word: "paused",
            role: StateRole::Warn,
        },
        RowState::Resumable(state) | RowState::Terminal(state) => {
            session_state_presentation(*state)
        }
        RowState::Unreadable => StatePresentation {
            word: "unreadable",
            role: StateRole::Danger,
        },
    }
}

/// The shared state presentation, extended for the claimed-task's stored
/// status. Mirrors `ctx_traits_cli::app::tasks::status_tone`'s pass/fail/
/// default semantics without importing that `pub(crate)` table: `Done` is
/// `Ok`, `Cancelled` is `Danger`, `Ready` is `Neutral`. Exhaustive, no
/// catch-all arm.
pub fn task_status_presentation(status: TaskStatus) -> StatePresentation {
    match status {
        TaskStatus::Done => StatePresentation {
            word: "done",
            role: StateRole::Ok,
        },
        TaskStatus::Cancelled => StatePresentation {
            word: "cancelled",
            role: StateRole::Danger,
        },
        TaskStatus::Ready => StatePresentation {
            word: "ready",
            role: StateRole::Neutral,
        },
    }
}

/// A presentation-ready run row. Identity fields are retained even when the
/// scope filters a row out of the currently painted list, so later selection
/// and delta application (0256.4) have something to key on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    // identity
    pub ledger_path: String,
    pub session_id: String,
    pub run_id: String,
    pub repo_key: String,
    pub repo_path: String,
    // presentation
    pub repo_label: String,
    pub title: String,
    pub trait_id: String,
    pub state: RowState,
    pub state_text: String,
    pub detail_text: String,
    pub elapsed_text: String,
    pub tokens_text: String,
    pub live: bool,
    pub modified_epoch_secs: u64,
    pub verdict_rounds: Option<u64>,
    // preview (0265.10) — the durable numbers, not the story-clock strings
    pub elapsed_seconds: u64,
    pub started_at_epoch: Option<u64>,
}

impl RunRow {
    /// The desktop analogue of `ctx_traits_cli::app::dashboard`'s
    /// `SessionClass::can_resume`: non-live, non-terminal, readable.
    pub fn can_resume(&self) -> bool {
        matches!(self.state, RowState::Resumable(_) | RowState::Paused)
    }
}

fn state_text_for(state: &RowState) -> String {
    presentation(state).word.to_string()
}

pub(crate) fn repo_label_for(repo_key: &str, repo_path: &str) -> String {
    if repo_path.is_empty() {
        return repo_key.to_string();
    }
    std::path::Path::new(repo_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| repo_key.to_string())
}

/// The one title-fallback chain, shared by compact rows and detail headers
/// (`detail_tree.rs`'s `DetailHeader::title`): authoritative `summary.title`
/// first, then an optional embellishment-only sidecar title (the compact row
/// has none, so it passes `None`), then `task_key`/`trait_id`/`run_id`, never
/// landing on an empty string. `pub(crate)` so `detail_tree.rs` reuses this
/// rather than copying it a second time.
pub(crate) fn summary_title(
    summary: &ctx_traits_io::run_summary::RunSummary,
    sidecar_title: Option<&str>,
) -> String {
    summary
        .title
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| sidecar_title.map(str::to_string).filter(|v| !v.is_empty()))
        .or_else(|| summary.task_key.clone().filter(|value| !value.is_empty()))
        .or_else(|| Some(summary.trait_id.clone()).filter(|value| !value.is_empty()))
        .or_else(|| Some(summary.run_id.clone()).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| summary.session_id.clone())
}

fn title_for(row: &CenterPublicRow) -> String {
    summary_title(&row.summary, None)
}

/// Mirrors `ctx_traits_cli::app::tui::elapsed_text`: an unbounded,
/// zero-padded `HH:MM:SS` clock. `pub(crate)` so `detail_tree.rs` reuses
/// this sanctioned mirror rather than copying it a second time.
pub(crate) fn elapsed_text(elapsed_seconds: u64) -> String {
    let hours = elapsed_seconds / 3600;
    let minutes = (elapsed_seconds % 3600) / 60;
    let seconds = elapsed_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Mirrors `ctx_traits_cli::app::dashboard::dashboard_token_value`'s
/// compaction: `-` for absent, otherwise a `k`/`m`-compacted count with no
/// trailing `.0`.
pub(crate) fn token_value(tokens: Option<u64>) -> String {
    let Some(tokens) = tokens else {
        return "-".to_string();
    };
    if tokens < 1_000 {
        return tokens.to_string();
    }
    if tokens >= 1_000_000 {
        let text = format!("{:.1}", tokens as f64 / 1_000_000.0);
        return format!("{}m", text.trim_end_matches(".0"));
    }
    let text = format!("{:.1}", tokens as f64 / 1_000.0);
    format!("{}k", text.trim_end_matches(".0"))
}

/// Mirrors `ctx_traits_cli::app::dashboard::dashboard_tokens_text_from_summary`.
pub(crate) fn tokens_text(work: Option<u64>, narrator: Option<u64>, guide: Option<u64>) -> String {
    if work.is_none() && narrator.is_none() && guide.is_none() {
        return "-".to_string();
    }
    format!(
        "W:{} N:{} G:{}",
        token_value(work),
        token_value(narrator),
        token_value(guide),
    )
}

fn row_state(row: &CenterPublicRow) -> RowState {
    let summary = &row.summary;
    if let Some(_error) = &summary.parse_error {
        return RowState::Unreadable;
    }
    let outcome = summary
        .last_drive_outcome
        .as_ref()
        .map(|outcome| DriveOutcomeKind::from_wire(outcome.as_str()));
    let state = SessionState::derive(&summary.status, outcome.as_ref(), row.live);
    if row.live {
        RowState::Live
    } else if state.is_terminal() {
        RowState::Terminal(state)
    } else if outcome == Some(DriveOutcomeKind::Paused) {
        RowState::Paused
    } else {
        RowState::Resumable(state)
    }
}

/// `pub(crate)` so `detail.rs` can project the exact same `RunRow` a live
/// `RowChanged`/`Appeared` delta or snapshot row carries, as the candidate a
/// resync's `LoadRequest` commits alongside its `DetailBaseline` — never a
/// second, independently-derived projection.
pub(crate) fn project_one(row: &CenterPublicRow) -> RunRow {
    let summary = &row.summary;
    let state = row_state(row);
    let state_text = state_text_for(&state);
    let detail_text = summary
        .parse_error
        .clone()
        .or_else(|| summary.current_sequence_title.clone())
        .unwrap_or_default();
    RunRow {
        ledger_path: row.ledger_path.clone(),
        session_id: summary.session_id.clone(),
        run_id: summary.run_id.clone(),
        repo_key: row.repo_key.clone(),
        repo_path: row.repo_path.clone(),
        repo_label: repo_label_for(&row.repo_key, &row.repo_path),
        title: title_for(row),
        trait_id: summary.trait_id.clone(),
        state,
        state_text,
        detail_text,
        elapsed_text: elapsed_text(summary.elapsed_seconds),
        tokens_text: tokens_text(
            summary.work_tokens,
            summary.narrator_tokens,
            summary.guide_tokens,
        ),
        live: row.live,
        modified_epoch_secs: row.modified_epoch_secs,
        verdict_rounds: summary.verdict_rounds,
        elapsed_seconds: summary.elapsed_seconds,
        started_at_epoch: summary.started_at_epoch,
    }
}

/// Project the center's wire rows into presentation-ready rows, filtered to
/// `scope`, ordered live-first then by newest ledger modification, with
/// `ledger_path` as the deterministic tiebreak.
pub fn project(rows: &[CenterPublicRow], scope: &RepoScope) -> Vec<RunRow> {
    let mut projected: Vec<RunRow> = rows
        .iter()
        .filter(|row| match scope {
            RepoScope::All => true,
            RepoScope::Repo(key) => &row.repo_key == key,
        })
        .map(project_one)
        .collect();
    projected.sort_by(|left, right| {
        right
            .live
            .cmp(&left.live)
            .then_with(|| right.modified_epoch_secs.cmp(&left.modified_epoch_secs))
            .then_with(|| left.ledger_path.cmp(&right.ledger_path))
    });
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::procedure::session::Status;
    use ctx_traits_io::run_summary::RunSummary;

    fn summary(run_id: &str, session_id: &str) -> RunSummary {
        RunSummary {
            run_id: run_id.to_string(),
            session_id: session_id.to_string(),
            ..RunSummary::unreadable(session_id.to_string(), "unused".to_string())
        }
    }

    fn readable_summary(run_id: &str, session_id: &str, status: Status) -> RunSummary {
        let mut summary = summary(run_id, session_id);
        summary.parse_error = None;
        summary.status = status;
        summary.trait_id = "fixture-trait".to_string();
        summary
    }

    fn row(
        repo_key: &str,
        repo_path: &str,
        ledger_path: &str,
        summary: RunSummary,
        live: bool,
        modified_epoch_secs: u64,
    ) -> CenterPublicRow {
        CenterPublicRow {
            summary,
            repo_key: repo_key.to_string(),
            repo_path: repo_path.to_string(),
            ledger_path: ledger_path.to_string(),
            live,
            modified_epoch_secs,
        }
    }

    #[test]
    fn repository_separation_keeps_same_named_runs_distinct() {
        let rows = vec![
            row(
                "repo-a",
                "/repo-a",
                "/repo-a/session.json",
                readable_summary("shared-run", "shared-session", Status::Completed),
                false,
                10,
            ),
            row(
                "repo-b",
                "/repo-b",
                "/repo-b/session.json",
                readable_summary("shared-run", "shared-session", Status::Completed),
                false,
                20,
            ),
        ];
        let projected = project(&rows, &RepoScope::All);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].repo_key, "repo-b");
        assert_eq!(projected[1].repo_key, "repo-a");
        assert_ne!(projected[0].ledger_path, projected[1].ledger_path);
    }

    #[test]
    fn repository_scoping_filters_to_one_repository() {
        let rows = vec![
            row(
                "repo-a",
                "/repo-a",
                "/repo-a/session.json",
                readable_summary("run-a", "session-a", Status::Completed),
                false,
                10,
            ),
            row(
                "repo-b",
                "/repo-b",
                "/repo-b/session.json",
                readable_summary("run-b", "session-b", Status::Completed),
                false,
                20,
            ),
        ];
        let scoped = project(&rows, &RepoScope::Repo("repo-a".to_string()));
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].repo_key, "repo-a");

        let unknown = project(&rows, &RepoScope::Repo("repo-unknown".to_string()));
        assert!(unknown.is_empty());
    }

    #[test]
    fn state_derivation_uses_the_row_liveness_flag() {
        let live_row = row(
            "repo",
            "/repo",
            "/repo/live.json",
            readable_summary("run", "session", Status::Completed),
            true,
            1,
        );
        assert_eq!(project_one(&live_row).state, RowState::Live);

        let completed_row = row(
            "repo",
            "/repo",
            "/repo/completed.json",
            readable_summary("run", "session", Status::Completed),
            false,
            1,
        );
        assert!(matches!(
            project_one(&completed_row).state,
            RowState::Terminal(SessionState::Completed)
        ));

        let awaiting_row = row(
            "repo",
            "/repo",
            "/repo/awaiting.json",
            readable_summary("run", "session", Status::AwaitingInput),
            false,
            1,
        );
        assert!(matches!(
            project_one(&awaiting_row).state,
            RowState::Resumable(SessionState::WaitingOnHuman)
        ));

        let mut interrupted_summary = readable_summary("run", "session", Status::Completed);
        interrupted_summary.last_drive_outcome = Some("interrupted".to_string());
        let interrupted_row = row(
            "repo",
            "/repo",
            "/repo/interrupted.json",
            interrupted_summary,
            false,
            1,
        );
        assert!(matches!(
            project_one(&interrupted_row).state,
            RowState::Terminal(SessionState::Cancelled)
        ));
    }

    #[test]
    fn paused_outcome_projects_to_row_state_paused_and_is_resumable() {
        let mut paused_summary = readable_summary("run", "session", Status::AwaitingInput);
        paused_summary.last_drive_outcome = Some("paused".to_string());
        let paused_row = row(
            "repo",
            "/repo",
            "/repo/paused.json",
            paused_summary,
            false,
            1,
        );
        let projected = project_one(&paused_row);
        assert_eq!(projected.state, RowState::Paused);
        assert_eq!(projected.state_text, "paused");
        assert!(projected.can_resume());
    }

    #[test]
    fn other_settled_pause_outcomes_still_project_resumable_not_paused() {
        for outcome in [
            "paused-provider-credits",
            "paused-budget-exhausted",
            "awaiting-owner",
        ] {
            let mut summary = readable_summary("run", "session", Status::AwaitingInput);
            summary.last_drive_outcome = Some(outcome.to_string());
            let outcome_row = row(
                "repo",
                "/repo",
                &format!("/repo/{outcome}.json"),
                summary,
                false,
                1,
            );
            let projected = project_one(&outcome_row);
            assert!(
                matches!(projected.state, RowState::Resumable(_)),
                "{outcome} must project Resumable, got {:?}",
                projected.state
            );
            assert!(projected.can_resume());
        }
    }

    #[test]
    fn can_resume_is_false_for_live_terminal_and_unreadable_rows() {
        let live_row = row(
            "repo",
            "/repo",
            "/repo/live.json",
            readable_summary("run", "session", Status::Completed),
            true,
            1,
        );
        assert!(!project_one(&live_row).can_resume());

        let terminal_row = row(
            "repo",
            "/repo",
            "/repo/terminal.json",
            readable_summary("run", "session", Status::Completed),
            false,
            1,
        );
        assert!(!project_one(&terminal_row).can_resume());

        let unreadable_row = row(
            "repo",
            "/repo",
            "/repo/unreadable.json",
            RunSummary::unreadable("session".to_string(), "bad json".to_string()),
            false,
            1,
        );
        assert!(!project_one(&unreadable_row).can_resume());
    }

    #[test]
    fn unreadable_rows_stay_distinct_across_repositories() {
        let rows = vec![
            row(
                "repo-a",
                "/repo-a",
                "/repo-a/broken.json",
                RunSummary::unreadable("session-a".to_string(), "bad json".to_string()),
                false,
                1,
            ),
            row(
                "repo-b",
                "/repo-b",
                "/repo-b/broken.json",
                RunSummary::unreadable("session-b".to_string(), "bad json too".to_string()),
                false,
                2,
            ),
        ];
        let projected = project(&rows, &RepoScope::All);
        assert_eq!(projected.len(), 2);
        for row in &projected {
            assert_eq!(row.state, RowState::Unreadable);
            assert!(row.run_id.is_empty());
        }
        assert_eq!(projected[0].repo_key, "repo-b");
        assert_eq!(projected[0].detail_text, "bad json too");
        assert_eq!(projected[1].repo_key, "repo-a");
        assert_eq!(projected[1].detail_text, "bad json");
    }

    #[test]
    fn ordering_is_live_first_then_newest_then_ledger_path() {
        let rows = vec![
            row(
                "repo",
                "/repo",
                "/repo/z-terminal.json",
                readable_summary("run-z", "session-z", Status::Completed),
                false,
                50,
            ),
            row(
                "repo",
                "/repo",
                "/repo/a-terminal.json",
                readable_summary("run-a", "session-a", Status::Completed),
                false,
                50,
            ),
            row(
                "repo",
                "/repo",
                "/repo/live.json",
                readable_summary("run-live", "session-live", Status::Completed),
                true,
                1,
            ),
        ];
        let projected = project(&rows, &RepoScope::All);
        let ledger_paths: Vec<_> = projected.iter().map(|r| r.ledger_path.as_str()).collect();
        assert_eq!(
            ledger_paths,
            vec![
                "/repo/live.json",
                "/repo/a-terminal.json",
                "/repo/z-terminal.json"
            ]
        );
    }

    #[test]
    fn repo_label_falls_back_to_repo_key_when_path_is_empty() {
        assert_eq!(repo_label_for("repository", ""), "repository");
        assert_eq!(repo_label_for("adhoc-abc123", ""), "adhoc-abc123");
        assert_eq!(repo_label_for("repo-a", "/tmp/checkouts/repo-a"), "repo-a");
    }

    #[test]
    fn formatting_helpers_match_expected_shapes() {
        assert_eq!(elapsed_text(0), "00:00:00");
        assert_eq!(elapsed_text(3661), "01:01:01");
        assert_eq!(elapsed_text(90_000), "25:00:00");

        assert_eq!(tokens_text(None, None, None), "-");
        assert_eq!(tokens_text(Some(500), None, None), "W:500 N:- G:-");
        assert_eq!(
            tokens_text(Some(1_500), Some(2_000_000), Some(999)),
            "W:1.5k N:2m G:999"
        );
    }

    #[test]
    fn title_falls_back_through_the_chain_and_is_never_empty() {
        let mut summary = readable_summary("run-id", "session-id", Status::Completed);
        summary.title = None;
        summary.task_key = None;
        let row = row("repo", "/repo", "/repo/session.json", summary, false, 1);
        assert_eq!(project_one(&row).title, "fixture-trait");
    }

    #[test]
    fn session_state_presentation_covers_every_variant() {
        let cases = [
            (SessionState::Running, "running", StateRole::Accent),
            (
                SessionState::WaitingOnHuman,
                "awaiting owner",
                StateRole::Warn,
            ),
            (SessionState::Blocked, "blocked", StateRole::Warn),
            (SessionState::Completed, "complete", StateRole::Ok),
            (SessionState::Failed, "failed", StateRole::Danger),
            (SessionState::Cancelled, "cancelled", StateRole::Neutral),
            (
                SessionState::WaitingOnAgent,
                "awaiting agent",
                StateRole::Neutral,
            ),
        ];
        for (state, word, role) in cases {
            let presented = session_state_presentation(state);
            assert_eq!(presented.word, word, "{state:?}");
            assert_eq!(presented.role, role, "{state:?}");
        }
    }

    #[test]
    fn row_state_presentation_covers_live_paused_and_unreadable() {
        assert_eq!(
            presentation(&RowState::Live),
            StatePresentation {
                word: "running",
                role: StateRole::Accent,
            }
        );
        assert_eq!(
            presentation(&RowState::Paused),
            StatePresentation {
                word: "paused",
                role: StateRole::Warn,
            }
        );
        assert_eq!(
            presentation(&RowState::Unreadable),
            StatePresentation {
                word: "unreadable",
                role: StateRole::Danger,
            }
        );
    }

    #[test]
    fn a_non_live_completed_row_presents_ok_not_accent() {
        let completed_row = row(
            "repo",
            "/repo",
            "/repo/completed.json",
            readable_summary("run", "session", Status::Completed),
            false,
            1,
        );
        let projected = project_one(&completed_row);
        let presented = presentation(&projected.state);
        assert_eq!(presented.word, "complete");
        assert_eq!(presented.role, StateRole::Ok);
    }

    #[test]
    fn unreadable_row_presents_danger_with_the_parse_error_as_detail() {
        let unreadable_row = row(
            "repo",
            "/repo",
            "/repo/unreadable.json",
            RunSummary::unreadable("session".to_string(), "bad json".to_string()),
            false,
            1,
        );
        let projected = project_one(&unreadable_row);
        assert_eq!(presentation(&projected.state).role, StateRole::Danger);
        assert_eq!(projected.detail_text, "bad json");
    }

    #[test]
    fn verdict_rounds_projects_through_from_the_summary() {
        let mut summary = readable_summary("run", "session", Status::Completed);
        summary.verdict_rounds = Some(3);
        let rounds_row = row("repo", "/repo", "/repo/rounds.json", summary, false, 1);
        assert_eq!(project_one(&rounds_row).verdict_rounds, Some(3));

        let none_row = row(
            "repo",
            "/repo",
            "/repo/no-rounds.json",
            readable_summary("run", "session", Status::Completed),
            false,
            1,
        );
        assert_eq!(project_one(&none_row).verdict_rounds, None);
    }

    #[test]
    fn empty_title_does_not_skip_a_present_task_key() {
        let mut summary = readable_summary("run-id", "session-id", Status::Completed);
        summary.title = Some(String::new());
        summary.task_key = Some("0256.3-render-repository-aware-run-rows".to_string());
        let row = row("repo", "/repo", "/repo/session.json", summary, false, 1);
        assert_eq!(
            project_one(&row).title,
            "0256.3-render-repository-aware-run-rows"
        );
    }
}
