//! Pure Tasks preview projection. The shell owns selection and loading; this
//! module only renders the one atomically accepted center answer.

use ctx_traits_io::center::{TaskClaimWire, TaskDetailWireResult};

use crate::preview::{KeyValueRow, NamedBlock, ValueSegment, frame_counter_text, staleness_word};
use crate::run_row::StateRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskDetailState {
    Loading,
    Accepted {
        answer: TaskDetailWireResult,
        stale: Option<String>,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lede {
    pub title: String,
    pub content: String,
}

pub fn tasks_lede(state: &TaskDetailState) -> Lede {
    match state {
        TaskDetailState::Loading => Lede {
            title: "loading".to_string(),
            content: String::new(),
        },
        TaskDetailState::Failed(reason) => Lede {
            title: "tasks unavailable".to_string(),
            content: reason.clone(),
        },
        TaskDetailState::Accepted {
            answer: TaskDetailWireResult::Missing,
            ..
        } => Lede {
            title: "task unavailable".to_string(),
            content: "task is no longer on this board".to_string(),
        },
        TaskDetailState::Accepted {
            answer:
                TaskDetailWireResult::Resolved {
                    summary, content, ..
                },
            ..
        } => Lede {
            title: summary.title.clone(),
            content: content.clone(),
        },
    }
}

fn unavailable_rows(text: &str) -> Vec<KeyValueRow> {
    ["id", "claimed", "state"]
        .into_iter()
        .map(|key| KeyValueRow {
            key: key.to_string(),
            value: vec![ValueSegment::toned(text, StateRole::Danger)],
        })
        .collect()
}

pub fn tasks_details_block(state: &TaskDetailState) -> NamedBlock {
    match state {
        TaskDetailState::Loading => NamedBlock {
            heading: "details".to_string(),
            rows: ["id", "claimed", "state"]
                .into_iter()
                .map(|key| KeyValueRow {
                    key: key.to_string(),
                    value: vec![ValueSegment::neutral("loading")],
                })
                .collect(),
        },
        TaskDetailState::Failed(reason) => NamedBlock {
            heading: "details".to_string(),
            rows: unavailable_rows(&format!("unreadable · {reason}")),
        },
        TaskDetailState::Accepted {
            answer: TaskDetailWireResult::Missing,
            ..
        } => NamedBlock {
            heading: "details".to_string(),
            rows: unavailable_rows("task unavailable: missing"),
        },
        TaskDetailState::Accepted {
            answer:
                TaskDetailWireResult::Resolved {
                    summary,
                    state,
                    current_activity,
                    claim,
                    ..
                },
            stale,
        } => {
            let heading = staleness_word(stale.as_deref(), false)
                .map(|word| format!("details · {word}"))
                .unwrap_or_else(|| "details".to_string());
            let role = if *current_activity {
                StateRole::Accent
            } else {
                StateRole::Neutral
            };
            let claimed = match claim {
                TaskClaimWire::NoClaim => vec![ValueSegment::neutral("no claim")],
                TaskClaimWire::Ambiguous(ids) => vec![ValueSegment::neutral(format!(
                    "ambiguous claim: {}",
                    ids.join(", ")
                ))],
                TaskClaimWire::Claim {
                    run_id, trait_id, ..
                } => vec![
                    ValueSegment::neutral(run_id),
                    ValueSegment::dot(),
                    ValueSegment::neutral(trait_id),
                ],
            };
            let mut state_value = vec![ValueSegment::toned(state, role)];
            if let TaskClaimWire::Claim { progress, .. } = claim
                && let Some(counter) = frame_counter_text(progress)
            {
                state_value.extend([ValueSegment::dot(), ValueSegment::neutral(counter)]);
            }
            NamedBlock {
                heading,
                rows: vec![
                    KeyValueRow {
                        key: "id".to_string(),
                        value: vec![
                            ValueSegment::neutral(&summary.key),
                            ValueSegment::dot(),
                            ValueSegment::toned(state, role),
                        ],
                    },
                    KeyValueRow {
                        key: "claimed".to_string(),
                        value: claimed,
                    },
                    KeyValueRow {
                        key: "state".to_string(),
                        value: state_value,
                    },
                ],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_core::task::graph::DerivedStatus;
    use ctx_traits_core::task::provider::TaskSummary;

    fn resolved(claim: TaskClaimWire) -> TaskDetailState {
        TaskDetailState::Accepted {
            answer: TaskDetailWireResult::Resolved {
                summary: TaskSummary {
                    key: "0266.4".to_string(),
                    title: "Task preview".to_string(),
                    stored_status: None,
                    derived_status: DerivedStatus::Ready,
                    archived: false,
                },
                content: "full served prose\n\nsecond paragraph".to_string(),
                state: "ready".to_string(),
                current_activity: false,
                claim,
            },
            stale: None,
        }
    }

    #[test]
    fn lede_uses_the_full_served_content_and_details_do_not_invent_a_claim() {
        let state = resolved(TaskClaimWire::NoClaim);
        assert_eq!(
            tasks_lede(&state).content,
            "full served prose\n\nsecond paragraph"
        );
        let details = tasks_details_block(&state);
        assert_eq!(details.heading, "details");
        assert_eq!(details.rows[1].value[0].text, "no claim");
        assert_eq!(details.rows[2].value[0].text, "ready");
    }

    #[test]
    fn progress_absences_never_render_a_zero_frame_counter() {
        for progress in [
            Ok(ctx_traits_core::procedure::run::RunProgress::NoCountedFrames),
            Ok(ctx_traits_core::procedure::run::RunProgress::NoneReached { total: 2 }),
            Err("plan unresolved".to_string()),
        ] {
            let details = tasks_details_block(&resolved(TaskClaimWire::Claim {
                run_id: "run".to_string(),
                trait_id: "trait".to_string(),
                progress,
            }));
            let text: String = details.rows[2]
                .value
                .iter()
                .map(|segment| segment.text.as_str())
                .collect();
            assert!(!text.contains("frame"));
            assert!(!text.contains("0 of"));
        }
    }
}
