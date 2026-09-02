//! Pure Tasks preview projection. The shell owns selection and loading; this
//! module only renders the one atomically accepted center answer.

use ctx_traits_io::center::{
    ClosePolicyResolution, TaskClaimState, TaskClaimWire, TaskDetailWireResult,
};

use crate::preview::{
    KeyValueRow, LandingBlock, LandingLine, NamedBlock, ValueSegment, frame_counter_text,
    staleness_word,
};
use crate::run_row::StateRole;

#[derive(Debug, Clone, PartialEq, Eq)]
// One instance per screen: boxing the Accepted payload buys nothing.
#[allow(clippy::large_enum_variant)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksBlock {
    pub heading: String,
    pub lines: Vec<String>,
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

/// Facts are served with the selected detail, never rediscovered from a board
/// or task document by the desktop.
pub fn tasks_facts_block(state: &TaskDetailState) -> NamedBlock {
    let (status, raised, parent, depends_on) = match state {
        TaskDetailState::Accepted {
            answer:
                TaskDetailWireResult::Resolved {
                    summary,
                    raised,
                    parent,
                    depends_on,
                    ..
                },
            ..
        } => (
            summary
                .stored_status
                .map(|status| format!("{status:?}").to_lowercase()),
            raised.as_deref(),
            parent.as_deref(),
            Some(depends_on.as_slice()),
        ),
        _ => (None, None, None, None),
    };
    let depends_value = match depends_on {
        Some(values) if !values.is_empty() => values
            .iter()
            .enumerate()
            .flat_map(|(index, value)| {
                let mut segments = Vec::new();
                if index > 0 {
                    segments.push(ValueSegment::dot());
                }
                segments.push(ValueSegment::neutral(value));
                segments
            })
            .collect(),
        _ => vec![ValueSegment::neutral("none")],
    };
    NamedBlock {
        heading: "facts".to_string(),
        rows: vec![
            KeyValueRow {
                key: "status".to_string(),
                value: vec![ValueSegment::neutral(
                    status.unwrap_or_else(|| "not stored".to_string()),
                )],
            },
            KeyValueRow {
                key: "raised".to_string(),
                value: vec![ValueSegment::neutral(raised.unwrap_or("not recorded"))],
            },
            KeyValueRow {
                key: "parent".to_string(),
                value: vec![ValueSegment::neutral(parent.unwrap_or("none"))],
            },
            KeyValueRow {
                key: "depends on".to_string(),
                value: depends_value,
            },
        ],
    }
}

pub fn tasks_checks_block(state: &TaskDetailState) -> ChecksBlock {
    let lines = match state {
        TaskDetailState::Accepted {
            answer: TaskDetailWireResult::Resolved { checks, .. },
            ..
        } => checks.iter().map(|check| check.name.clone()).collect(),
        _ => Vec::new(),
    };
    ChecksBlock {
        heading: "checks".to_string(),
        lines,
    }
}

pub fn tasks_landing_block(state: &TaskDetailState) -> LandingBlock {
    use ctx_traits_core::procedure::landing::{ResolvedClosePolicy, SelectedTaskClaimState};
    let line = match state {
        TaskDetailState::Accepted {
            answer:
                TaskDetailWireResult::Resolved {
                    summary,
                    closure,
                    claim,
                    close_policy,
                    checks,
                    ..
                },
            ..
        } => {
            let claim_state = match claim {
                TaskClaimWire::NoClaim => SelectedTaskClaimState::None,
                TaskClaimWire::Ambiguous(_) => SelectedTaskClaimState::Ambiguous,
                TaskClaimWire::Claim { state, .. } => match state {
                    TaskClaimState::Active => SelectedTaskClaimState::Active,
                    TaskClaimState::Pending => SelectedTaskClaimState::Pending,
                    TaskClaimState::Terminal => SelectedTaskClaimState::Terminal,
                },
            };
            let policy = match close_policy {
                ClosePolicyResolution::Effective(policy) => ResolvedClosePolicy::Effective(*policy),
                ClosePolicyResolution::NoneConfigured => ResolvedClosePolicy::NoneConfigured,
                ClosePolicyResolution::Unresolved(_) => ResolvedClosePolicy::Unresolved,
            };
            ctx_traits_core::procedure::landing::selected_task_close_line(
                &summary.key,
                summary.stored_status,
                closure.as_ref(),
                claim_state,
                policy,
                checks.len(),
            )
        }
        _ => ctx_traits_core::procedure::landing::LandingLine {
            text: "\u{2192} Task details unavailable".to_string(),
            tone: ctx_traits_core::procedure::landing::LineTone::Danger,
        },
    };
    let role = match line.tone {
        ctx_traits_core::procedure::landing::LineTone::Neutral => None,
        ctx_traits_core::procedure::landing::LineTone::Ok => Some(StateRole::Ok),
        ctx_traits_core::procedure::landing::LineTone::Warn => Some(StateRole::Warn),
        ctx_traits_core::procedure::landing::LineTone::Danger => Some(StateRole::Danger),
    };
    LandingBlock {
        heading: "landing".to_string(),
        lines: vec![LandingLine {
            text: line.text,
            role,
        }],
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
                scope: String::new(),
                validation: String::new(),
                open_steps: Vec::new(),
                state: "ready".to_string(),
                current_activity: false,
                raised: None,
                parent: None,
                depends_on: Vec::new(),
                checks: Vec::new(),
                closure: None,
                close_policy: ClosePolicyResolution::NoneConfigured,
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
                state: TaskClaimState::Active,
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

    #[test]
    fn facts_use_declared_relation_order_and_absence_words() {
        let facts = tasks_facts_block(&resolved(TaskClaimWire::NoClaim));
        assert_eq!(facts.heading, "facts");
        assert_eq!(facts.rows[0].value[0].text, "not stored");
        assert_eq!(facts.rows[1].value[0].text, "not recorded");
        assert_eq!(facts.rows[2].value[0].text, "none");
        assert_eq!(facts.rows[3].value[0].text, "none");
        let checks = tasks_checks_block(&resolved(TaskClaimWire::NoClaim));
        assert_eq!(checks.heading, "checks");
        assert!(checks.lines.is_empty());
    }
}
