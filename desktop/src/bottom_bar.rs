//! `screens/sessions.md`/`grammar.md` rule 8's bottom bar as a gpui-free
//! model: the words, order, tones and detail joining are all assertable in
//! plain `#[test]`s here, with no gpui `App` needed. `bottom_bar_view.rs`
//! paints exactly this model.

use crate::placeholders;
use crate::run_row::{RunRow, StatePresentation, presentation};

/// The two tones rule 8 distinguishes today. The third (destructive/tertiary
/// `text-muted`) tone is deliberately not defined until a screen renders one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionTone {
    Primary,
    Secondary,
}

/// A right-hand action word. Carrying no bound behaviour is the model's way
/// of saying "no-op" — a view attaches (or does not attach) a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarAction {
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
/// rather than extending this one.
pub fn sessions_bar(row: &RunRow) -> BottomBar {
    let state = presentation(&row.state);
    let mut detail = Vec::new();
    if let Some(rounds) = row.verdict_rounds {
        detail.push(format!("review round {rounds}"));
    }
    if matches!(row.state, crate::run_row::RowState::Unreadable) && !row.detail_text.is_empty() {
        detail.push(row.detail_text.clone());
    }
    BottomBar {
        state,
        detail,
        actions: vec![
            BarAction {
                label: placeholders::WATCH_RAW.label.to_string(),
                tone: ActionTone::Secondary,
            },
            BarAction {
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
            trait_id: "fixture-trait".to_string(),
            state: RowState::Live,
            state_text: "running".to_string(),
            detail_text: String::new(),
            elapsed_text: "00:00:00".to_string(),
            tokens_text: "-".to_string(),
            live: true,
            modified_epoch_secs: 0,
            verdict_rounds: None,
        }
    }

    #[test]
    fn sessions_bar_actions_are_watch_raw_then_pause() {
        let bar = sessions_bar(&base_row());
        assert_eq!(
            bar.actions,
            vec![
                BarAction {
                    label: placeholders::WATCH_RAW.label.to_string(),
                    tone: ActionTone::Secondary,
                },
                BarAction {
                    label: "pause".to_string(),
                    tone: ActionTone::Primary,
                },
            ]
        );
    }

    #[test]
    fn sessions_bar_carries_no_detail_when_verdict_rounds_is_none() {
        let bar = sessions_bar(&base_row());
        assert_eq!(bar.detail, Vec::<String>::new());
    }

    #[test]
    fn sessions_bar_renders_review_round_when_verdict_rounds_is_some() {
        let mut row = base_row();
        row.verdict_rounds = Some(2);
        let bar = sessions_bar(&row);
        assert_eq!(bar.detail, vec!["review round 2".to_string()]);
    }

    #[test]
    fn sessions_bar_surfaces_the_parse_error_for_an_unreadable_row() {
        let mut row = base_row();
        row.state = RowState::Unreadable;
        row.detail_text = "bad json".to_string();
        let bar = sessions_bar(&row);
        assert_eq!(bar.state.word, "unreadable");
        assert_eq!(bar.state.role, StateRole::Danger);
        assert_eq!(bar.detail, vec!["bad json".to_string()]);
    }
}
