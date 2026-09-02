//! Tasks-pane projection assembled only from the center's served board answer.

use ctx_traits_core::task::provider::{
    self, BoardRun, BoardTaskState, ClaimResolution, TaskSummary, board_summary_counts,
};
use ctx_traits_io::center::BoardWireResult;
use ctx_traits_io::center::CreateTaskWireResult;
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

/// The Tasks bar's local, gpui-free create state. It deliberately holds no
/// board data: creation becomes visible only through a later BoardChanged.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum NewTaskPresentation {
    #[default]
    Idle,
    Editing {
        text: String,
    },
    Requesting,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingCreate {
    repo_key: String,
    generation: u64,
    board_scope_generation: u64,
}

/// The local title affordance. Terminal events release the active guard but
/// retain one correlation long enough to render its late refusal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewTaskEntry {
    presentation: NewTaskPresentation,
    pending: Option<PendingCreate>,
    terminal: Option<PendingCreate>,
    awaiting_board_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRequest {
    pub repo_key: String,
    pub title: String,
    pub generation: u64,
    pub board_scope_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Result(CreateTaskWireResult),
    Failed(String),
}

/// The one production create transport mapping. Keeping payload construction
/// here makes the UI dispatch and transport-level tests exercise identical
/// title, repository, and Draft semantics.
pub fn dispatch_create(request: CreateRequest) -> CreateOutcome {
    let task = ctx_traits_core::task::provider::NewTask {
        title: request.title,
        status: Some(ctx_traits_core::task::TaskStatus::Draft),
        ..Default::default()
    };
    match ctx_traits_io::center::create_task_existing(&request.repo_key, task) {
        Ok(result) => CreateOutcome::Result(result),
        Err(error) => CreateOutcome::Failed(error.to_string()),
    }
}

impl NewTaskEntry {
    pub fn activate(&mut self) {
        if self.pending.is_none() {
            self.terminal = None;
            self.presentation = NewTaskPresentation::Editing {
                text: String::new(),
            };
        }
    }

    pub fn cancel(&mut self) {
        if matches!(self.presentation, NewTaskPresentation::Editing { .. }) {
            self.presentation = NewTaskPresentation::Idle;
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        if let NewTaskPresentation::Editing { text } = &mut self.presentation {
            text.push(ch);
        }
    }

    pub fn backspace(&mut self) {
        if let NewTaskPresentation::Editing { text } = &mut self.presentation {
            text.pop();
        }
    }

    pub fn submit(
        &mut self,
        repo_key: String,
        generation: u64,
        board_scope_generation: u64,
    ) -> Option<CreateRequest> {
        let NewTaskPresentation::Editing { text } = &self.presentation else {
            return None;
        };
        if text.is_empty() {
            self.presentation = NewTaskPresentation::Idle;
            return None;
        }
        let title = text.clone();
        self.pending = Some(PendingCreate {
            repo_key: repo_key.clone(),
            generation,
            board_scope_generation,
        });
        self.presentation = NewTaskPresentation::Requesting;
        Some(CreateRequest {
            repo_key,
            title,
            generation,
            board_scope_generation,
        })
    }

    pub fn settle(
        &mut self,
        repo_key: &str,
        generation: u64,
        board_scope_generation: u64,
        outcome: CreateOutcome,
    ) -> bool {
        let matches = |pending: &PendingCreate| {
            pending.repo_key == repo_key
                && pending.generation == generation
                && pending.board_scope_generation == board_scope_generation
        };
        let active = self.pending.as_ref().is_some_and(matches);
        let terminal = self.terminal.as_ref().is_some_and(matches);
        if !active && !terminal {
            return false;
        }
        if active {
            self.pending = None;
        }
        if terminal {
            self.terminal = None;
        }
        self.presentation = match outcome {
            CreateOutcome::Result(CreateTaskWireResult::Created(summary)) if active => {
                self.awaiting_board_key = Some(summary.key);
                NewTaskPresentation::Idle
            }
            CreateOutcome::Result(CreateTaskWireResult::Created(_)) => self.presentation.clone(),
            CreateOutcome::Result(result @ CreateTaskWireResult::Occupied) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Result(result @ CreateTaskWireResult::InvalidField { .. }) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Result(result @ CreateTaskWireResult::UnknownParent { .. }) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Result(result @ CreateTaskWireResult::AmbiguousParent { .. }) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Result(result @ CreateTaskWireResult::BoardAbsent) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Result(result @ CreateTaskWireResult::BoardUnreadable { .. }) => {
                NewTaskPresentation::Failed {
                    message: format!("refused: {result}"),
                }
            }
            CreateOutcome::Failed(error) => NewTaskPresentation::Failed {
                message: format!("refused: {error}"),
            },
        };
        true
    }

    pub fn board_accepted(
        &mut self,
        previous: Option<&BoardWireResult>,
        answer: &BoardWireResult,
    ) -> bool {
        self.board_accepted_keys(
            previous.into_iter().flat_map(|answer| {
                answer
                    .resolution
                    .rows
                    .iter()
                    .map(|row| row.summary.key.as_str())
            }),
            answer
                .resolution
                .rows
                .iter()
                .map(|row| row.summary.key.as_str()),
        )
    }

    fn board_accepted_keys<'a>(
        &mut self,
        previous_keys: impl IntoIterator<Item = &'a str>,
        keys: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        let previous_keys: Vec<_> = previous_keys.into_iter().collect();
        let keys: Vec<_> = keys.into_iter().collect();
        let added_keys: Vec<_> = keys
            .iter()
            .copied()
            .filter(|key| !previous_keys.contains(key))
            .collect();
        let matched_completed_request = self
            .awaiting_board_key
            .as_ref()
            .is_some_and(|key| added_keys.contains(&key.as_str()));
        if matched_completed_request {
            self.awaiting_board_key = None;
        }
        if self.pending.is_some() {
            // A delayed answer for an already-completed request can include
            // only that request's new key. A second added key is this pending
            // creation's BoardChanged, so reconcile both in one full answer.
            if matched_completed_request && added_keys.len() == 1 {
                return false;
            }
            self.terminal = self.pending.take();
            self.presentation = NewTaskPresentation::Idle;
            return true;
        }
        false
    }

    pub fn deactivated(&mut self) -> bool {
        let changed = !matches!(self.presentation, NewTaskPresentation::Idle)
            || self.pending.is_some()
            || self.terminal.is_some()
            || self.awaiting_board_key.is_some();
        self.presentation = NewTaskPresentation::Idle;
        self.pending = None;
        self.terminal = None;
        self.awaiting_board_key = None;
        changed
    }

    /// A broken subscription releases the pending affordance while retaining
    /// one correlation for a late refusal until the owner starts again.
    pub fn subscription_down(&mut self) -> bool {
        if self.pending.is_some() {
            self.terminal = self.pending.take();
            self.presentation = NewTaskPresentation::Idle;
            return true;
        }
        false
    }

    /// Whether a create response remains correlated to this entry.
    pub fn awaits_result(&self) -> bool {
        self.pending.is_some() || self.terminal.is_some()
    }

    pub fn action(&self) -> BarAction {
        match &self.presentation {
            NewTaskPresentation::Idle => BarAction {
                id: BarActionId::NewTask,
                label: "new task".to_string(),
                tone: ActionTone::Primary,
            },
            NewTaskPresentation::Editing { text } => BarAction {
                id: BarActionId::NewTask,
                label: format!("new task: {text}"),
                tone: ActionTone::Muted,
            },
            NewTaskPresentation::Requesting => BarAction {
                id: BarActionId::NewTask,
                label: "creating".to_string(),
                tone: ActionTone::Muted,
            },
            NewTaskPresentation::Failed { message } => BarAction {
                id: BarActionId::NewTask,
                label: message.clone(),
                tone: ActionTone::Danger,
            },
        }
    }
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

pub fn tasks_bar(state: &BoardState, entry: &NewTaskEntry) -> BottomBar {
    let (word, role, detail) = match board_status(state) {
        Ok((word, detail)) => (word, StateRole::Ok, detail),
        Err(reason) => ("unavailable", StateRole::Danger, vec![reason]),
    };
    BottomBar {
        state: StatePresentation { word, role },
        detail,
        actions: vec![entry.action()],
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
    use ctx_traits_io::task_files::BoardResolution;

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

    fn create_request() -> (NewTaskEntry, CreateRequest) {
        let mut entry = NewTaskEntry::default();
        entry.activate();
        entry.insert_char(' ');
        let request = entry
            .submit("repo-a".to_string(), 1, 1)
            .expect("non-empty title");
        (entry, request)
    }

    #[test]
    fn new_task_entry_cancels_empty_submit_and_allows_only_one_request() {
        let mut entry = NewTaskEntry::default();
        entry.activate();
        assert!(entry.submit("repo-a".to_string(), 1, 1).is_none());
        assert_eq!(entry.action().label, "new task");

        entry.activate();
        entry.insert_char('x');
        let request = entry
            .submit("repo-a".to_string(), 1, 1)
            .expect("first request");
        assert_eq!(request.title, "x");
        assert!(entry.submit("repo-a".to_string(), 2, 1).is_none());
    }

    #[test]
    fn each_create_refusal_and_transport_error_is_visible_and_terminal() {
        let outcomes = [
            CreateOutcome::Result(CreateTaskWireResult::Occupied),
            CreateOutcome::Result(CreateTaskWireResult::InvalidField {
                field: "title".to_string(),
                reason: "blank".to_string(),
            }),
            CreateOutcome::Result(CreateTaskWireResult::UnknownParent {
                parent: "0001".to_string(),
            }),
            CreateOutcome::Result(CreateTaskWireResult::AmbiguousParent {
                parent: "0001".to_string(),
            }),
            CreateOutcome::Result(CreateTaskWireResult::BoardAbsent),
            CreateOutcome::Result(CreateTaskWireResult::BoardUnreadable {
                reason: "bad toml".to_string(),
            }),
            CreateOutcome::Failed("connection closed".to_string()),
        ];
        let mut messages = Vec::new();
        for outcome in outcomes {
            let (mut entry, request) = create_request();
            assert!(entry.settle(
                &request.repo_key,
                request.generation,
                request.board_scope_generation,
                outcome,
            ));
            let NewTaskPresentation::Failed { message } = entry.presentation else {
                panic!("refusal must clear requesting and remain visible")
            };
            assert!(message.starts_with("refused: "));
            messages.push(message);
        }
        messages.sort();
        messages.dedup();
        assert_eq!(messages.len(), 7);
    }

    #[test]
    fn subscription_loss_renders_a_late_refusal_until_a_new_interaction() {
        let (mut entry, request) = create_request();
        assert!(entry.subscription_down());
        assert_eq!(entry.action().label, "new task");
        assert!(entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Failed("late reply".to_string())
        ));
        assert_eq!(entry.action().label, "refused: late reply");

        entry.activate();
        entry.insert_char('b');
        let next = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("subscription loss must free the guard");
        assert!(!entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Failed("stale reply".to_string())
        ));
        assert_eq!(entry.action().label, "creating");
        assert!(entry.settle(
            &next.repo_key,
            next.generation,
            next.board_scope_generation,
            CreateOutcome::Failed("current reply".to_string())
        ));
        assert_eq!(entry.action().tone, ActionTone::Danger);
    }

    #[test]
    fn subscription_loss_allows_a_new_submit_and_ignores_the_old_result() {
        let (mut entry, request) = create_request();
        assert!(entry.subscription_down());
        entry.activate();
        entry.insert_char('b');
        let next = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("subscription loss must free the guard");
        assert!(!entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Failed("late reply".to_string())
        ));
        assert_eq!(entry.action().label, "creating");
        assert!(entry.settle(
            &next.repo_key,
            next.generation,
            next.board_scope_generation,
            CreateOutcome::Failed("current reply".to_string())
        ));
        assert_eq!(entry.action().tone, ActionTone::Danger);
    }

    #[test]
    fn deactivation_invalidates_pending_and_clears_a_prior_failure() {
        let (mut entry, request) = create_request();
        assert!(entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Failed("old repository".to_string()),
        ));
        assert!(entry.deactivated());
        assert_eq!(entry.action().label, "new task");
        assert!(!entry.settle(
            &request.repo_key,
            request.generation,
            request.board_scope_generation,
            CreateOutcome::Failed("late reply".to_string())
        ));
    }

    #[test]
    fn a_delayed_board_for_a_completed_request_does_not_hide_a_new_request() {
        let (mut entry, first) = create_request();
        let mut created = summary(DerivedStatus::Draft);
        created.key = "created-a".to_string();
        assert!(entry.settle(
            &first.repo_key,
            first.generation,
            first.board_scope_generation,
            CreateOutcome::Result(CreateTaskWireResult::Created(created)),
        ));

        entry.activate();
        entry.insert_char('b');
        let second = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("second request");
        assert!(!entry.board_accepted_keys([], ["created-a"]));
        assert_eq!(entry.action().label, "creating");
        assert!(entry.settle(
            &second.repo_key,
            second.generation,
            second.board_scope_generation,
            CreateOutcome::Failed("second refused".to_string()),
        ));
    }

    #[test]
    fn board_acceptance_renders_a_late_refusal_until_a_new_interaction() {
        let (mut entry, first) = create_request();
        assert!(entry.board_accepted_keys([], ["created-a"]));
        assert!(entry.settle(
            &first.repo_key,
            first.generation,
            first.board_scope_generation,
            CreateOutcome::Failed("late error".to_string()),
        ));
        assert_eq!(entry.action().label, "refused: late error");

        entry.activate();
        entry.insert_char('b');
        let second = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("board acceptance must free the guard");
        assert!(!entry.settle(
            &first.repo_key,
            first.generation,
            first.board_scope_generation,
            CreateOutcome::Failed("stale error".to_string()),
        ));
        assert_eq!(entry.action().label, "creating");
        assert!(entry.settle(
            &second.repo_key,
            second.generation,
            second.board_scope_generation,
            CreateOutcome::Failed("current error".to_string()),
        ));
    }

    #[test]
    fn board_acceptance_allows_a_new_submit_and_ignores_the_old_result() {
        let (mut entry, first) = create_request();
        assert!(entry.board_accepted_keys([], ["created-a"]));
        entry.activate();
        entry.insert_char('b');
        let second = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("board acceptance must free the guard");
        assert!(!entry.settle(
            &first.repo_key,
            first.generation,
            first.board_scope_generation,
            CreateOutcome::Failed("late error".to_string()),
        ));
        assert_eq!(entry.action().label, "creating");
        assert!(entry.settle(
            &second.repo_key,
            second.generation,
            second.board_scope_generation,
            CreateOutcome::Failed("current error".to_string()),
        ));
    }

    #[test]
    fn a_full_answer_for_completed_and_pending_creations_reconciles_both() {
        let (mut entry, first) = create_request();
        let mut created = summary(DerivedStatus::Draft);
        created.key = "created-a".to_string();
        assert!(entry.settle(
            &first.repo_key,
            first.generation,
            first.board_scope_generation,
            CreateOutcome::Result(CreateTaskWireResult::Created(created)),
        ));

        entry.activate();
        entry.insert_char('b');
        let second = entry
            .submit("repo-a".to_string(), 2, 1)
            .expect("second request");

        assert!(entry.board_accepted_keys([], ["created-a", "created-b"]));
        assert_eq!(entry.action().label, "new task");
        assert!(entry.settle(
            &second.repo_key,
            second.generation,
            second.board_scope_generation,
            CreateOutcome::Failed("late error".to_string()),
        ));
        assert_eq!(entry.action().label, "refused: late error");
    }

    #[test]
    fn tasks_bar_and_footer_recover_from_matching_board_freshness() {
        let answer = BoardWireResult {
            resolution: BoardResolution {
                presence: BoardPresence::Empty,
                digest: Some("sha256:0123456789".to_string()),
                rows: Vec::new(),
                sync_report: ctx_traits_core::task::provider::SyncReport::default(),
                resolved_at: 0,
            },
            sections: Default::default(),
            joined_runs: Default::default(),
        };
        let stale = BoardState::Accepted {
            answer: answer.clone(),
            stale: Some("connection closed".to_string()),
        };
        assert_eq!(
            tasks_bar(&stale, &NewTaskEntry::default()).state.word,
            "unavailable"
        );
        assert_eq!(tasks_footer(&stale), "stale: connection closed");
        let current = BoardState::Accepted {
            answer,
            stale: None,
        };
        assert_eq!(
            tasks_bar(&current, &NewTaskEntry::default()).state.word,
            "synced"
        );
        assert!(tasks_footer(&current).contains("synced"));
    }
}
