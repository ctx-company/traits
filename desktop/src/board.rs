//! Tasks-pane projection assembled only from the center's served board answer.

use ctx_traits_core::task::provider::{
    self, BoardRun, BoardTaskState, ClaimResolution, TaskSummary, board_summary_counts,
};
use ctx_traits_io::center::BoardWireResult;
use ctx_traits_io::task_files::{BOARD_DIR_NAME, BoardPresence};

use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
use crate::placeholders;
use crate::rail::repo_display_name;
use crate::run_row::{StatePresentation, StateRole};
use crate::screen_header::ScreenHeader;

pub enum BoardState {
    Loading,
    Accepted {
        answer: BoardWireResult,
        stale: Option<String>,
    },
    Failed(String),
}

/// The complete right-side presentation of one served task row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRowStack {
    pub dot_color: u32,
    pub word: String,
    pub word_role: StateRole,
    pub meta: Option<String>,
}

pub fn task_row_stack(
    summary: &TaskSummary,
    joined: &[BoardRun],
    unmet_dependencies: &[String],
) -> TaskRowStack {
    let state = provider::task_state(summary.derived_status, joined);
    let claim = provider::chosen_claim(joined);
    match (state, claim) {
        (BoardTaskState::InProgress, ClaimResolution::One(run)) => TaskRowStack {
            dot_color: crate::tokens::ACCENT,
            word: format!("live · {}", run.run_id),
            word_role: StateRole::Accent,
            meta: None,
        },
        (BoardTaskState::InProgress, ClaimResolution::Ambiguous(_))
        | (BoardTaskState::InProgress, ClaimResolution::NoClaim) => TaskRowStack {
            dot_color: crate::tokens::ACCENT,
            word: "live".to_string(),
            word_role: StateRole::Accent,
            meta: None,
        },
        (BoardTaskState::Pending, ClaimResolution::NoClaim)
        | (BoardTaskState::Pending, ClaimResolution::One(_))
        | (BoardTaskState::Pending, ClaimResolution::Ambiguous(_)) => TaskRowStack {
            dot_color: crate::tokens::WARN,
            word: provider::state_word(state).to_string(),
            word_role: StateRole::Warn,
            meta: None,
        },
        (BoardTaskState::AwaitingMerge, ClaimResolution::NoClaim)
        | (BoardTaskState::AwaitingMerge, ClaimResolution::One(_))
        | (BoardTaskState::AwaitingMerge, ClaimResolution::Ambiguous(_)) => {
            neutral_stack(state, None)
        }
        (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Ready),
            ClaimResolution::NoClaim,
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Ready),
            ClaimResolution::One(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Ready),
            ClaimResolution::Ambiguous(_),
        ) => neutral_stack(state, Some("deps met".to_string())),
        (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Blocked),
            ClaimResolution::NoClaim,
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Blocked),
            ClaimResolution::One(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Blocked),
            ClaimResolution::Ambiguous(_),
        ) => neutral_stack(state, blocked_meta(unmet_dependencies)),
        (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Draft),
            ClaimResolution::NoClaim,
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Draft),
            ClaimResolution::One(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Draft),
            ClaimResolution::Ambiguous(_),
        ) => TaskRowStack {
            dot_color: crate::tokens::DOT_DIM,
            word: provider::state_word(state).to_string(),
            word_role: StateRole::Neutral,
            meta: blocked_meta(unmet_dependencies),
        },
        (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Done),
            ClaimResolution::NoClaim,
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Done),
            ClaimResolution::One(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Done),
            ClaimResolution::Ambiguous(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Cancelled),
            ClaimResolution::NoClaim,
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Cancelled),
            ClaimResolution::One(_),
        )
        | (
            BoardTaskState::Board(ctx_traits_core::task::graph::DerivedStatus::Cancelled),
            ClaimResolution::Ambiguous(_),
        ) => neutral_stack(state, None),
    }
}

fn neutral_stack(state: BoardTaskState, meta: Option<String>) -> TaskRowStack {
    TaskRowStack {
        dot_color: crate::tokens::DOT_IDLE,
        word: provider::state_word(state).to_string(),
        word_role: StateRole::Neutral,
        meta,
    }
}

fn blocked_meta(unmet_dependencies: &[String]) -> Option<String> {
    (!unmet_dependencies.is_empty())
        .then(|| format!("blocked by {}", unmet_dependencies.join(" · ")))
}

pub fn tasks_header(answer: &BoardWireResult, repo_key: &str, repo_path: &str) -> ScreenHeader {
    let counts = board_summary_counts(answer.resolution.rows.iter().map(|row| {
        (
            &row.summary,
            answer.sections.get(&row.summary.key).copied().flatten(),
            answer
                .joined_runs
                .get(&row.summary.key)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
    }));
    ScreenHeader {
        title: format!("tasks — {}", repo_display_name(repo_key, repo_path)),
        summary: placeholders::board_summary(counts.0, counts.1, counts.2, counts.3),
    }
}

pub fn short_board_digest(digest: &str) -> String {
    digest
        .strip_prefix("sha256:")
        .unwrap_or(digest)
        .chars()
        .take(8)
        .collect()
}

fn board_status(state: &BoardState) -> Result<(&'static str, Vec<String>), String> {
    match state {
        BoardState::Accepted {
            answer,
            stale: None,
        } if matches!(
            answer.resolution.presence,
            BoardPresence::Empty | BoardPresence::Loaded
        ) && answer.resolution.digest.is_some() =>
        {
            Ok((
                "synced",
                vec![format!(
                    "{BOARD_DIR_NAME} @ {}",
                    short_board_digest(answer.resolution.digest.as_deref().unwrap_or_default())
                )],
            ))
        }
        _ => Err(match state {
            BoardState::Loading => "loading board".to_string(),
            BoardState::Failed(reason) => reason.clone(),
            BoardState::Accepted {
                stale: Some(reason),
                ..
            } => format!("stale: {reason}"),
            BoardState::Accepted { answer, .. } => match &answer.resolution.presence {
                BoardPresence::Absent => "tasks absent".to_string(),
                BoardPresence::Unreadable { reason } => format!("tasks unreadable: {reason}"),
                BoardPresence::Empty | BoardPresence::Loaded => "board incomplete".to_string(),
            },
        }),
    }
}

pub fn tasks_bar(state: &BoardState) -> BottomBar {
    let (word, role, detail) = match board_status(state) {
        Ok((word, detail)) => (word, StateRole::Ok, detail),
        Err(reason) => ("unavailable", StateRole::Danger, vec![reason]),
    };
    BottomBar {
        state: StatePresentation { word, role },
        detail,
        actions: vec![BarAction {
            id: BarActionId::NewTask,
            label: "new task".to_string(),
            tone: ActionTone::Primary,
        }],
    }
}

pub fn tasks_footer(state: &BoardState) -> String {
    match board_status(state) {
        Ok((_, _)) => {
            let BoardState::Accepted { answer, .. } = state else {
                unreachable!()
            };
            let epoch = answer.resolution.resolved_at;
            let clock = ctx_traits_io::clock::epoch_clock_minutes(
                epoch,
                ctx_traits_io::clock::local_utc_offset_seconds(epoch),
            );
            format!("{BOARD_DIR_NAME} \u{b7} synced \u{b7} {clock}")
        }
        Err(reason) => reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::graph::DerivedStatus;

    fn summary(status: DerivedStatus) -> TaskSummary {
        TaskSummary {
            key: "0266.6".to_string(),
            title: "State stack".to_string(),
            stored_status: None,
            derived_status: status,
            archived: false,
        }
    }

    fn run(run_id: &str, live: bool, awaiting_owner: bool, not_merged: bool) -> BoardRun {
        BoardRun {
            run_id: run_id.to_string(),
            repo_key: None,
            task_key: "0266.6".to_string(),
            live,
            awaiting_owner,
            not_merged,
        }
    }

    #[test]
    fn short_digest_is_display_only_and_total() {
        assert_eq!(short_board_digest("sha256:123456789"), "12345678");
        assert_eq!(short_board_digest("short"), "short");
    }

    #[test]
    fn task_row_stack_covers_each_named_form_without_residue() {
        let cases = [
            (
                summary(DerivedStatus::Ready),
                vec![run("run-live", true, false, false)],
                vec![],
                "live · run-live",
                StateRole::Accent,
                crate::tokens::ACCENT,
                None,
            ),
            (
                summary(DerivedStatus::Ready),
                vec![
                    run("run-a", true, false, false),
                    run("run-b", true, false, false),
                ],
                vec![],
                "live",
                StateRole::Accent,
                crate::tokens::ACCENT,
                None,
            ),
            (
                summary(DerivedStatus::Ready),
                vec![run("run-owner", false, true, false)],
                vec![],
                "awaiting owner",
                StateRole::Warn,
                crate::tokens::WARN,
                None,
            ),
            (
                summary(DerivedStatus::Ready),
                vec![run("run-merge", false, false, true)],
                vec![],
                "awaiting merge",
                StateRole::Neutral,
                crate::tokens::DOT_IDLE,
                None,
            ),
            (
                summary(DerivedStatus::Ready),
                vec![],
                vec![],
                "ready",
                StateRole::Neutral,
                crate::tokens::DOT_IDLE,
                Some("deps met"),
            ),
            (
                summary(DerivedStatus::Blocked),
                vec![],
                vec!["0002".to_string(), "9999".to_string()],
                "blocked",
                StateRole::Neutral,
                crate::tokens::DOT_IDLE,
                Some("blocked by 0002 · 9999"),
            ),
            (
                summary(DerivedStatus::Draft),
                vec![],
                vec!["0002".to_string()],
                "draft",
                StateRole::Neutral,
                crate::tokens::DOT_DIM,
                Some("blocked by 0002"),
            ),
        ];
        for (summary, runs, unmet, word, role, dot_color, meta) in cases {
            let stack = task_row_stack(&summary, &runs, &unmet);
            assert_eq!(stack.word, word);
            assert_eq!(stack.word_role, role);
            assert_eq!(stack.dot_color, dot_color);
            assert_eq!(stack.meta.as_deref(), meta);
        }

        assert_eq!(
            task_row_stack(&summary(DerivedStatus::Blocked), &[], &[]).meta,
            None
        );
        assert_eq!(
            task_row_stack(&summary(DerivedStatus::Draft), &[], &[]).meta,
            None
        );
    }
}
