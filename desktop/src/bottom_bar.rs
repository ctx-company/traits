//! `screens/sessions.md`/`grammar.md` rule 8's bottom bar as a gpui-free
//! model: the words, order, tones and detail joining are all assertable in
//! plain `#[test]`s here, with no gpui `App` needed. `bottom_bar_view.rs`
//! paints exactly this model.

use crate::placeholders;
use crate::row_control::{self, RowStatus};
use crate::run_row::{RunRow, StatePresentation, presentation};

/// The two tones rule 8 distinguishes today. The third (destructive/tertiary
/// `text-muted`) tone is deliberately not defined until a screen renders one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    Primary,
    Secondary,
}

/// Which action a `BarAction` is, independent of its display label — the
/// view attaches behaviour by identity, never by matching label text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarActionId {
    WatchRaw,
    Pause,
}

/// A right-hand action word. Carrying no bound behaviour is the model's way
/// of saying "no-op" — a view attaches (or does not attach) a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarAction {
    pub id: BarActionId,
    pub label: String,
    pub tone: ActionTone,
}

/// The bottom bar's full content: a state word plus role, caller-supplied
/// detail segments, and the right-hand actions. Takes no `RunRow`,
/// `CenterPublicRow`, `RunSummary`, center handle or clock — only these
/// primitives — so any future screen composes it without depending on the
/// Sessions projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BottomBar {
    pub state: StatePresentation,
    pub detail: Vec<String>,
    pub actions: Vec<BarAction>,
}

impl BottomBar {
    /// `None` when there are no detail segments (no dangling separator, no
    /// empty element), else the segments joined with `· `, matching the
    /// export's `· frame 5 of 9 · review round 2`.
    pub fn detail_text(&self) -> Option<String> {
        if self.detail.is_empty() {
            return None;
        }
        Some(format!("· {}", self.detail.join(" · ")))
    }
}

/// The Sessions screen's composer: turns a selected `&RunRow` into a
/// `BottomBar`. Documented as caller-side, not part of the component, so
/// `0266`-`0269` can write their own composer against the same `BottomBar`
/// rather than extending this one. `control_status` is the row's current
/// `RowControls::status` entry, if any; its wording is reused verbatim from
/// `row_control::status_text` and appended as the last detail segment (kept
/// last so `0265.14`'s `frame N of M` can prepend without colliding). It
/// never moves `state`: that changes only through an accepted center delta.
pub fn sessions_bar(row: &RunRow, control_status: Option<&RowStatus>) -> BottomBar {
    let state = presentation(&row.state);
    let mut detail = Vec::new();
    if let Some(rounds) = row.verdict_rounds {
        detail.push(format!("review round {rounds}"));
    }
    if matches!(row.state, crate::run_row::RowState::Unreadable) && !row.detail_text.is_empty() {
        detail.push(row.detail_text.clone());
    }
    if let Some(status) = control_status {
        detail.push(row_control::status_text(status));
    }
    BottomBar {
        state,
        detail,
        actions: vec![
            BarAction {
                id: BarActionId::WatchRaw,
                label: placeholders::WATCH_RAW.label.to_string(),
                tone: ActionTone::Secondary,
            },
            BarAction {
                id: BarActionId::Pause,
                label: "pause".to_string(),
                tone: ActionTone::Primary,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_row::{RowState, StateRole};

    fn fixture_presentation(word: &'static str, role: StateRole) -> StatePresentation {
        StatePresentation { word, role }
    }

    #[test]
    fn detail_text_is_none_when_there_are_no_segments() {
        let bar = BottomBar {
            state: fixture_presentation("running", StateRole::Accent),
            detail: Vec::new(),
            actions: Vec::new(),
        };
        assert_eq!(bar.detail_text(), None);
    }

    #[test]
    fn detail_text_joins_a_single_segment_with_a_leading_separator() {
        let bar = BottomBar {
            state: fixture_presentation("running", StateRole::Accent),
            detail: vec!["review round 2".to_string()],
            actions: Vec::new(),
        };
        assert_eq!(bar.detail_text().as_deref(), Some("· review round 2"));
    }

    #[test]
    fn detail_text_joins_multiple_segments_matching_the_export_shape() {
        let bar = BottomBar {
            state: fixture_presentation("running", StateRole::Accent),
            detail: vec!["frame 5 of 9".to_string(), "review round 2".to_string()],
            actions: Vec::new(),
        };
        assert_eq!(
            bar.detail_text().as_deref(),
            Some("· frame 5 of 9 · review round 2")
        );
    }

    fn base_row() -> RunRow {
        RunRow {
            ledger_path: "/repo/session.json".to_string(),
            session_id: "session".to_string(),
            run_id: "run".to_string(),
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            repo_label: "repo".to_string(),
            title: "title".to_string(),
            session_title: None,
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "running".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
            elapsed_seconds: 0,
            started_at_epoch: None,
        }
    }

    #[test]
    fn sessions_bar_actions_are_watch_raw_then_pause() {
        let bar = sessions_bar(&base_row(), None);
        assert_eq!(
            bar.actions,
            vec![
                BarAction {
                    id: BarActionId::WatchRaw,
                    label: placeholders::WATCH_RAW.label.to_string(),
                    tone: ActionTone::Secondary,
                },
                BarAction {
                    id: BarActionId::Pause,
                    label: "pause".to_string(),
                    tone: ActionTone::Primary,
                },
            ]
        );
    }

    #[test]
    fn sessions_bar_carries_no_detail_when_verdict_rounds_is_none() {
        let bar = sessions_bar(&base_row(), None);
        assert_eq!(bar.detail, Vec::<String>::new());
    }

    #[test]
    fn sessions_bar_renders_review_round_when_verdict_rounds_is_some() {
        let mut row = base_row();
        row.verdict_rounds = Some(2);
        let bar = sessions_bar(&row, None);
        assert_eq!(bar.detail, vec!["review round 2".to_string()]);
    }

    #[test]
    fn sessions_bar_surfaces_the_parse_error_for_an_unreadable_row() {
        let mut row = base_row();
        row.state = RowState::Unreadable;
        row.detail_text = "bad json".to_string();
        let bar = sessions_bar(&row, None);
        assert_eq!(bar.state.word, "unreadable");
        assert_eq!(bar.state.role, StateRole::Danger);
        assert_eq!(bar.detail, vec!["bad json".to_string()]);
    }

    #[test]
    fn a_control_status_renders_as_the_last_detail_segment() {
        let mut row = base_row();
        row.verdict_rounds = Some(2);
        let bar = sessions_bar(&row, Some(&RowStatus::Requesting));
        assert_eq!(
            bar.detail_text().as_deref(),
            Some("· review round 2 · requesting…")
        );
    }

    #[test]
    fn the_state_word_is_identical_with_no_status_requesting_and_requested() {
        let row = base_row();
        let requested = RowStatus::Requested {
            verb: row_control::RowVerb::Pause,
            session_id: row.session_id.clone(),
        };
        let none_bar = sessions_bar(&row, None);
        let requesting_bar = sessions_bar(&row, Some(&RowStatus::Requesting));
        let requested_bar = sessions_bar(&row, Some(&requested));
        assert_eq!(none_bar.state.word, "running");
        assert_eq!(requesting_bar.state, none_bar.state);
        assert_eq!(requested_bar.state, none_bar.state);
        assert_ne!(none_bar.detail, requesting_bar.detail);
        assert_ne!(none_bar.detail, requested_bar.detail);
    }

    #[test]
    fn each_non_acknowledged_outcome_reaches_the_bar_with_a_distinct_message() {
        use crate::row_control::{RowControls, RowOutcome};
        use ctx_traits_io::center::ControlResult;

        let outcomes = [
            RowOutcome::Control(ControlResult::Missing),
            RowOutcome::Control(ControlResult::Ambiguous(vec!["one".to_string()])),
            RowOutcome::Control(ControlResult::NotLive),
            RowOutcome::Control(ControlResult::Unverifiable),
            RowOutcome::Control(ControlResult::Refused),
            RowOutcome::Failed("connection refused".to_string()),
        ];

        let mut messages = Vec::new();
        for outcome in outcomes {
            let mut controls = RowControls::default();
            let row = base_row();
            let request = controls.request(&row, row_control::RowVerb::Pause).unwrap();
            controls.settle(&row.ledger_path, request.generation, outcome);
            let status = controls.status(&row.ledger_path).unwrap();
            let bar = sessions_bar(&row, Some(status));
            let detail = bar.detail_text().unwrap();
            assert!(detail.contains("refused:"));
            assert!(!detail.contains("paused"));
            messages.push(detail);
        }

        let mut unique = messages.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), messages.len());
    }

    #[test]
    fn a_paused_row_renders_the_paused_word_from_the_landed_presentation() {
        let mut row = base_row();
        row.state = RowState::Paused;
        row.live = false;
        let bar = sessions_bar(&row, None);
        assert_eq!(bar.state.word, "paused");
        assert_eq!(bar.state.role, StateRole::Warn);
    }
}
