//! Pure presentation projection for the placeholder-served Merges pane.

use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
use crate::placeholders::{self, MergeRowContent};
use crate::preview::{KeyValueRow, LandingBlock, LandingLine, NamedBlock, ValueSegment};
use crate::rail;
use crate::run_row::{StatePresentation, StateRole};
use crate::screen_header::ScreenHeader;

pub const INITIAL_SELECTION: usize = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeForm {
    Landing,
    Awaiting,
    Landed,
}

impl MergeForm {
    pub fn state(self) -> (&'static str, StateRole) {
        match self {
            Self::Landing => ("landing", StateRole::Accent),
            Self::Awaiting => ("awaiting approval", StateRole::Warn),
            Self::Landed => ("landed", StateRole::Ok),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRowData {
    pub content: MergeRowContent,
    pub state_word: &'static str,
    pub state_role: StateRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSectionData {
    pub heading: &'static str,
    pub rows: Vec<MergeRowData>,
}

pub fn merge_sections() -> Vec<MergeSectionData> {
    [
        (
            "Landing",
            MergeForm::Landing,
            placeholders::MERGE_LANDING_ROWS,
        ),
        (
            "Awaiting approval",
            MergeForm::Awaiting,
            placeholders::MERGE_AWAITING_ROWS,
        ),
        ("Landed", MergeForm::Landed, placeholders::MERGE_LANDED_ROWS),
    ]
    .into_iter()
    .map(|(heading, form, contents)| {
        let (state_word, state_role) = form.state();
        MergeSectionData {
            heading,
            rows: contents
                .iter()
                .copied()
                .map(|content| MergeRowData {
                    content,
                    state_word,
                    state_role,
                })
                .collect(),
        }
    })
    .collect()
}

pub fn row_count(sections: &[MergeSectionData]) -> usize {
    sections.iter().map(|section| section.rows.len()).sum()
}

pub fn merges_header(scope: Option<(&str, &str)>) -> ScreenHeader {
    let title = match scope {
        Some((repo_key, repo_path)) => {
            format!("merges — {}", rail::repo_display_name(repo_key, repo_path))
        }
        None => "merges".to_string(),
    };
    ScreenHeader {
        title,
        summary: placeholders::MERGES_SUMMARY.to_string(),
    }
}

pub fn merges_bar() -> BottomBar {
    BottomBar {
        state: StatePresentation {
            word: placeholders::MERGES_BAR.state_word,
            role: StateRole::Accent,
        },
        detail: vec![placeholders::MERGES_BAR.detail.to_string()],
        actions: vec![
            BarAction {
                id: BarActionId::Watch,
                label: "watch".to_string(),
                tone: ActionTone::Secondary,
            },
            BarAction {
                id: BarActionId::Hold,
                label: "hold".to_string(),
                tone: ActionTone::Primary,
            },
        ],
    }
}

pub fn merges_merge_block() -> NamedBlock {
    NamedBlock {
        heading: "merge".to_string(),
        rows: vec![
            KeyValueRow {
                key: "run".to_string(),
                value: vec![ValueSegment::neutral(placeholders::MERGES_MERGE.run)],
            },
            KeyValueRow {
                key: "target".to_string(),
                value: vec![ValueSegment::neutral(placeholders::MERGES_MERGE.target)],
            },
            KeyValueRow {
                key: "state".to_string(),
                value: vec![ValueSegment::toned(
                    placeholders::MERGES_MERGE.state,
                    StateRole::Accent,
                )],
            },
        ],
    }
}

pub fn merges_landing_block() -> LandingBlock {
    LandingBlock {
        heading: "landing".to_string(),
        lines: vec![LandingLine::consequence(
            placeholders::MERGES_LANDING_CONSEQUENCE,
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_map_to_the_grammar_roles() {
        assert_eq!(MergeForm::Landing.state(), ("landing", StateRole::Accent));
        assert_eq!(
            MergeForm::Awaiting.state(),
            ("awaiting approval", StateRole::Warn)
        );
        assert_eq!(MergeForm::Landed.state(), ("landed", StateRole::Ok));
    }

    #[test]
    fn sections_have_the_fixed_order_and_counts() {
        let sections = merge_sections();
        assert_eq!(
            sections
                .iter()
                .map(|section| section.heading)
                .collect::<Vec<_>>(),
            ["Landing", "Awaiting approval", "Landed"]
        );
        assert_eq!(
            sections
                .iter()
                .map(|section| section.rows.len())
                .collect::<Vec<_>>(),
            [1, 2, 2]
        );
    }

    #[test]
    fn rows_reuse_placeholder_content_and_descriptions_are_complete() {
        let sections = merge_sections();
        assert_eq!(
            sections[0].rows[0].content,
            placeholders::MERGE_LANDING_ROWS[0]
        );
        assert_eq!(
            sections[2].rows[1].content,
            placeholders::MERGE_LANDED_ROWS[1]
        );
        for row in sections.iter().flat_map(|section| &section.rows) {
            assert!(row.content.description.contains(" → main · "));
            assert!(!row.content.description.ends_with('·'));
        }
    }

    #[test]
    fn initial_selection_is_the_landing_row() {
        let sections = merge_sections();
        assert_eq!(INITIAL_SELECTION, 0);
        assert_eq!(
            sections[0].rows[INITIAL_SELECTION].content.title,
            "doctor schema guard"
        );
    }

    #[test]
    fn merges_chrome_uses_scope_and_preserves_role_boundaries() {
        assert_eq!(
            merges_header(Some(("fallback", "/work/acme/widgets"))).title,
            "merges — acme/widgets"
        );
        assert_eq!(merges_header(None).title, "merges");
        assert_eq!(
            merges_header(Some(("repo-a", "/srv/other/api"))).title,
            "merges — other/api"
        );
        assert_eq!(merges_header(Some(("repo-a", ""))).title, "merges — repo-a");
        assert_eq!(
            merges_header(Some(("repo-a", "/workspace"))).title,
            "merges — repo-a"
        );
        let bar = merges_bar();
        assert_eq!(
            bar.state,
            StatePresentation {
                word: "landing",
                role: StateRole::Accent
            }
        );
        assert_eq!(
            bar.actions
                .iter()
                .map(|action| action.id)
                .collect::<Vec<_>>(),
            [BarActionId::Watch, BarActionId::Hold]
        );
        assert_eq!(
            bar.actions
                .iter()
                .map(|action| action.tone)
                .collect::<Vec<_>>(),
            [ActionTone::Secondary, ActionTone::Primary]
        );
        assert_eq!(bar.detail, ["run-1a2b3c → main · deep merge"]);
        assert_eq!(
            bar.actions
                .iter()
                .map(|action| action.label.as_str())
                .collect::<Vec<_>>(),
            ["watch", "hold"]
        );
        let block = merges_merge_block();
        assert_eq!(
            block
                .rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            ["run", "target", "state"]
        );
        assert_eq!(
            block
                .rows
                .iter()
                .map(|row| row.value[0].role)
                .collect::<Vec<_>>(),
            [None, None, Some(StateRole::Accent)]
        );
        let landing = merges_landing_block();
        assert_eq!(
            landing.lines[0].text,
            format!("→ {}", placeholders::MERGES_LANDING_CONSEQUENCE)
        );
    }
}
