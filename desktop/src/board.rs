//! Tasks-pane projection assembled only from the center's served board answer.

use ctx_traits_core::task::provider::board_summary_counts;
use ctx_traits_io::center::BoardWireResult;
use ctx_traits_io::task_files::BoardPresence;

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

pub fn tasks_bar(state: &BoardState) -> BottomBar {
    let clean = match state {
        BoardState::Accepted {
            answer,
            stale: None,
        } => {
            matches!(
                answer.resolution.presence,
                BoardPresence::Empty | BoardPresence::Loaded
            ) && answer.resolution.digest.is_some()
        }
        _ => false,
    };
    let (word, role, detail) = if clean {
        let BoardState::Accepted { answer, .. } = state else {
            unreachable!()
        };
        (
            "synced",
            StateRole::Ok,
            vec![format!(
                ".internal/tasks @ {}",
                short_board_digest(answer.resolution.digest.as_deref().unwrap_or_default())
            )],
        )
    } else {
        let reason = match state {
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
        };
        ("unavailable", StateRole::Danger, vec![reason])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_digest_is_display_only_and_total() {
        assert_eq!(short_board_digest("sha256:123456789"), "12345678");
        assert_eq!(short_board_digest("short"), "short");
    }
}
