//! Pure presentation projection for the placeholder-served Merges pane.

use crate::placeholders::{self, MergeRowContent};
use crate::run_row::StateRole;

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
}
