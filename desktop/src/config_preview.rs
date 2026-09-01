//! Pure Config preview projection from the accepted center answer.

use ctx_traits_io::config_view::{ConfigResolution, ConfigSeatRow, ConfigView, ConfigWinnerWire};

use crate::config_screen::{self, ConfigState};
use crate::placeholders;
use crate::preview::{KeyValueRow, NamedBlock, ValueSegment};
use crate::run_row::StateRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatIdentity {
    pub role: String,
    pub seat_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPreview {
    pub seat_block: NamedBlock,
    pub prose: Option<&'static str>,
    pub facts_block: NamedBlock,
    pub footer: String,
}

fn failure(text: impl Into<String>) -> ConfigPreview {
    let value = vec![ValueSegment::toned(text, StateRole::Danger)];
    ConfigPreview {
        seat_block: NamedBlock {
            heading: "seat".to_string(),
            rows: vec![KeyValueRow {
                key: "name".to_string(),
                value: value.clone(),
            }],
        },
        prose: None,
        facts_block: NamedBlock {
            heading: "facts".to_string(),
            rows: vec![KeyValueRow {
                key: "status".to_string(),
                value,
            }],
        },
        footer: "unavailable".to_string(),
    }
}

fn source_row(label: &str, winner: &ConfigWinnerWire, key: String) -> KeyValueRow {
    let mut value = vec![
        ValueSegment::neutral(label),
        ValueSegment::dot(),
        ValueSegment::neutral(&winner.layer),
        ValueSegment::dot(),
        ValueSegment::neutral(&winner.reason),
    ];
    if let Some(source) = &winner.source {
        value.push(ValueSegment::dot());
        value.push(ValueSegment::neutral(source));
    }
    KeyValueRow { key, value }
}

fn seat_preview(view: &ConfigView, seat: &ConfigSeatRow) -> ConfigPreview {
    let mut name = vec![ValueSegment::neutral(&seat.role)];
    if let (Some(index), Some(length)) = (seat.seat_index, seat.list_length) {
        name.push(ValueSegment::dot());
        name.push(ValueSegment::neutral(format!("{index} of {length}")));
    }
    let used_by = if seat.used_by.is_empty() {
        vec![ValueSegment::neutral("none")]
    } else {
        let mut segments = Vec::new();
        for user in &seat.used_by {
            if !segments.is_empty() {
                segments.push(ValueSegment::dot());
            }
            segments.push(ValueSegment::neutral(&user.member));
            if let Some(scope) = &user.scope {
                segments.push(ValueSegment::dot());
                segments.push(ValueSegment::neutral(scope));
            }
            if let Some(model) = &user.model {
                segments.push(ValueSegment::dot());
                segments.push(ValueSegment::neutral(model));
            }
            if let Some(effort) = &user.reasoning_effort {
                segments.push(ValueSegment::dot());
                segments.push(ValueSegment::neutral(effort));
            }
        }
        segments
    };
    let mut rows = vec![
        KeyValueRow {
            key: "model".to_string(),
            value: vec![ValueSegment::neutral(
                seat.model.as_deref().unwrap_or("none configured"),
            )],
        },
        KeyValueRow {
            key: "effort".to_string(),
            value: vec![ValueSegment::neutral(
                seat.reasoning_effort
                    .as_deref()
                    .unwrap_or("none configured"),
            )],
        },
        KeyValueRow {
            key: "used by".to_string(),
            value: used_by,
        },
    ];
    if let Some(winner) = &seat.model_winner {
        rows.push(source_row("model", winner, "source".to_string()));
    }
    if let Some(winner) = &seat.effort_winner {
        rows.push(source_row("effort", winner, String::new()));
    }
    let provenance = config_screen::config_provenance(view);
    let verdict = config_screen::config_verdict(view);
    let mut footer = vec![provenance.citation];
    if let Some(qualifier) = provenance.document_count_qualifier {
        footer.push(qualifier);
    }
    footer.push(verdict.word.to_string());
    footer.push(format!("resolved {} UTC", provenance.instant));
    ConfigPreview {
        seat_block: NamedBlock {
            heading: "seat".to_string(),
            rows: vec![KeyValueRow {
                key: "name".to_string(),
                value: name,
            }],
        },
        prose: Some(placeholders::SEAT_PROSE),
        facts_block: NamedBlock {
            heading: "facts".to_string(),
            rows,
        },
        footer: footer.join(" · "),
    }
}

pub fn project(state: &ConfigState, selection: Option<&SeatIdentity>) -> Option<ConfigPreview> {
    let selection = selection?;
    match state {
        ConfigState::Loading => Some(failure("loading")),
        ConfigState::Failed(reason) => Some(failure(format!("unavailable: {reason}"))),
        ConfigState::Accepted {
            stale: Some(reason),
            ..
        } => Some(failure(format!("stale: {reason}"))),
        ConfigState::Accepted {
            answer,
            stale: None,
        } => match &answer.resolution {
            ConfigResolution::Resolved(view) => view
                .seats
                .iter()
                .find(|seat| seat.role == selection.role && seat.seat_index == selection.seat_index)
                .map(|seat| seat_preview(view, seat)),
            ConfigResolution::Refused { reason } => Some(failure(format!("refused: {reason}"))),
            ConfigResolution::Failed { reason } => Some(failure(format!("unavailable: {reason}"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::config_view::{ConfigRuntimeRow, ConfigTrustRow};

    #[test]
    fn facts_keep_order_absences_and_distinct_sources() {
        let view = ConfigView {
            seats: vec![ConfigSeatRow {
                role: "worker".into(),
                seat_index: None,
                list_length: None,
                model: None,
                reasoning_effort: None,
                model_winner: None,
                effort_winner: None,
                used_by: vec![],
            }],
            runtime: vec![ConfigRuntimeRow {
                name: "center".into(),
                value: "v".into(),
                qualifier: String::new(),
            }],
            trust: ConfigTrustRow {
                approved_digests: 0,
                approved_members: vec![],
            },
            documents: vec![],
            edit_target: None,
            tier_warnings: vec![],
            instant_epoch_millis: 0,
        };
        let state = ConfigState::Accepted {
            answer: ctx_traits_io::center::ConfigWireResult {
                repo_key: "r".into(),
                repo_path: "p".into(),
                resolution: ConfigResolution::Resolved(view),
            },
            stale: None,
        };
        let preview = project(
            &state,
            Some(&SeatIdentity {
                role: "worker".into(),
                seat_index: None,
            }),
        )
        .expect("preview");
        assert_eq!(
            preview
                .facts_block
                .rows
                .iter()
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>(),
            vec!["model", "effort", "used by"]
        );
        assert_eq!(preview.facts_block.rows[0].value[0].text, "none configured");
        assert_eq!(preview.facts_block.rows[2].value[0].text, "none");
    }
}
