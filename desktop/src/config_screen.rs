//! Config-pane projection assembled exclusively from the center's served answer.

use std::collections::HashSet;

use ctx_traits_io::center::ConfigWireResult;
use ctx_traits_io::config_view::{ConfigResolution, ConfigView};

use crate::bottom_bar::{ActionTone, BarAction, BarActionId, BottomBar};
use crate::placeholders;
use crate::rail::repo_display_name;
use crate::run_row::{StatePresentation, StateRole};
use crate::screen_header::ScreenHeader;

// One instance per screen: boxing the Accepted payload buys nothing.
#[allow(clippy::large_enum_variant)]
pub enum ConfigState {
    Loading,
    Accepted {
        answer: ConfigWireResult,
        stale: Option<String>,
    },
    Failed(String),
}

pub fn fold_config_result(
    state: &ConfigState,
    result: Result<ConfigWireResult, String>,
) -> ConfigState {
    match result {
        Ok(answer) => ConfigState::Accepted {
            answer,
            stale: None,
        },
        Err(reason) => match state {
            ConfigState::Accepted { answer, .. } => ConfigState::Accepted {
                answer: answer.clone(),
                stale: Some(reason),
            },
            ConfigState::Loading | ConfigState::Failed(_) => ConfigState::Failed(reason),
        },
    }
}

pub fn config_header(state: &ConfigState) -> ScreenHeader {
    match state {
        ConfigState::Loading => ScreenHeader {
            title: "runtime unavailable".to_string(),
            summary: "loading configuration".to_string(),
        },
        ConfigState::Failed(reason) => ScreenHeader {
            title: "runtime unavailable".to_string(),
            summary: reason.clone(),
        },
        ConfigState::Accepted { answer, stale } => match &answer.resolution {
            ConfigResolution::Resolved(view) => {
                let summary = placeholders::config_summary(
                    view.seats.len(),
                    engine_count(view),
                    view.trust.approved_members.len(),
                );
                ScreenHeader {
                    title: format!(
                        "runtime — {}",
                        repo_display_name(&answer.repo_key, &answer.repo_path)
                    ),
                    summary: stale.as_deref().map_or(summary.clone(), |reason| {
                        format!("stale: {reason} — {summary}")
                    }),
                }
            }
            ConfigResolution::Refused { reason } | ConfigResolution::Failed { reason } => {
                ScreenHeader {
                    title: "runtime unavailable".to_string(),
                    summary: reason.clone(),
                }
            }
        },
    }
}

fn engine_count(view: &ConfigView) -> usize {
    let harnesses: HashSet<&str> = view
        .runtime
        .iter()
        .find(|row| row.name == "harness")
        .map(|row| row.value.split(", ").filter(|id| !id.is_empty()).collect())
        .unwrap_or_default();
    harnesses.len() + 1
}

/// One shared presentation of Config provenance, using only the served answer.
pub fn config_provenance_text(view: &ConfigView) -> String {
    let provenance = config_provenance(view);
    let mut parts = vec![provenance.citation];
    if let Some(qualifier) = provenance.document_count_qualifier {
        parts.push(qualifier);
    }
    parts.push(format!("resolved {} UTC", provenance.instant));
    parts.join(" · ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProvenance {
    pub citation: String,
    pub document_count_qualifier: Option<String>,
    pub instant: String,
}

pub fn config_provenance(view: &ConfigView) -> ConfigProvenance {
    let instant = utc_hh_mm(view.instant_epoch_millis);
    match view.documents.last() {
        Some(document) => {
            let name = document.path.rsplit('/').next().unwrap_or(&document.path);
            ConfigProvenance {
                citation: name.to_string(),
                document_count_qualifier: (view.documents.len() > 1)
                    .then(|| format!("of {} documents", view.documents.len())),
                instant,
            }
        }
        None => ConfigProvenance {
            citation: "built-in defaults".to_string(),
            document_count_qualifier: None,
            instant,
        },
    }
}

pub fn config_verdict(view: &ConfigView) -> StatePresentation {
    if view.tier_warnings.is_empty() {
        StatePresentation {
            word: "valid",
            role: StateRole::Ok,
        }
    } else {
        StatePresentation {
            word: "warning",
            role: StateRole::Warn,
        }
    }
}

pub fn config_footer_text(state: &ConfigState) -> String {
    config_detail(state)
}

fn utc_hh_mm(epoch_millis: u128) -> String {
    let minutes = (epoch_millis / 60_000) % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

pub fn config_bar(state: &ConfigState) -> BottomBar {
    match state {
        ConfigState::Accepted {
            answer,
            stale: None,
        } => match &answer.resolution {
            ConfigResolution::Resolved(view) => {
                let mut detail = vec![config_provenance_text(view)];
                let verdict = config_verdict(view);
                if !view.tier_warnings.is_empty() {
                    detail.extend(view.tier_warnings.clone());
                }
                let actions = match &view.edit_target {
                    Some(_) => vec![BarAction {
                        id: BarActionId::EditRuntimeToml,
                        label: placeholders::EDIT_RUNTIME_TOML.label.to_string(),
                        tone: ActionTone::Primary,
                    }],
                    None => vec![BarAction {
                        id: BarActionId::EditRuntimeToml,
                        label: "no editable source".to_string(),
                        tone: ActionTone::Muted,
                    }],
                };
                BottomBar {
                    state: verdict,
                    detail,
                    actions,
                }
            }
            ConfigResolution::Refused { reason } | ConfigResolution::Failed { reason } => {
                unavailable_bar(reason.clone())
            }
        },
        ConfigState::Accepted {
            stale: Some(reason),
            ..
        } => unavailable_bar(format!("stale: {reason}")),
        ConfigState::Loading => unavailable_bar("loading configuration".to_string()),
        ConfigState::Failed(reason) => unavailable_bar(reason.clone()),
    }
}

fn unavailable_bar(detail: String) -> BottomBar {
    BottomBar {
        state: StatePresentation {
            word: "unavailable",
            role: StateRole::Danger,
        },
        detail: vec![detail],
        actions: vec![],
    }
}

fn config_detail(state: &ConfigState) -> String {
    match state {
        ConfigState::Accepted {
            answer,
            stale: None,
        } => match &answer.resolution {
            ConfigResolution::Resolved(view) => config_provenance_text(view),
            ConfigResolution::Refused { reason } | ConfigResolution::Failed { reason } => {
                reason.clone()
            }
        },
        ConfigState::Accepted {
            stale: Some(reason),
            ..
        } => format!("stale: {reason}"),
        ConfigState::Loading => "loading configuration".to_string(),
        ConfigState::Failed(reason) => reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::config_view::{ConfigDocument, ConfigRuntimeRow, ConfigTrustRow};

    fn view() -> ConfigView {
        ConfigView {
            seats: vec![],
            runtime: vec![ConfigRuntimeRow {
                name: "center".to_string(),
                value: "center".to_string(),
                qualifier: String::new(),
            }],
            trust: ConfigTrustRow {
                approved_digests: 9,
                approved_members: vec![],
            },
            documents: vec![],
            edit_target: None,
            tier_warnings: vec![],
            instant_epoch_millis: 9 * 60 * 60 * 1_000 + 41 * 60 * 1_000,
        }
    }

    fn answer(view: ConfigView) -> ConfigWireResult {
        ConfigWireResult {
            repo_key: "repo".to_string(),
            repo_path: "/work/acme/repo".to_string(),
            resolution: ConfigResolution::Resolved(view),
        }
    }

    #[test]
    fn header_uses_served_scope_and_count_projections() {
        let mut view = view();
        view.runtime.insert(
            0,
            ConfigRuntimeRow {
                name: "harness".to_string(),
                value: "one, two, one".to_string(),
                qualifier: String::new(),
            },
        );
        view.trust.approved_members = vec!["one".to_string()];
        let header = config_header(&ConfigState::Accepted {
            answer: answer(view),
            stale: None,
        });
        assert_eq!(header.title, "runtime — acme/repo");
        assert_eq!(header.summary, "0 seats · 3 engines · 1 trusted member");
    }

    #[test]
    fn provenance_uses_last_served_document_and_utc_instant() {
        let mut view = view();
        view.documents = vec![
            ConfigDocument {
                layer: "repo".to_string(),
                path: "/one/first.toml".to_string(),
            },
            ConfigDocument {
                layer: "environment".to_string(),
                path: "/two/override.toml".to_string(),
            },
        ];
        assert_eq!(
            config_provenance_text(&view),
            "override.toml · of 2 documents · resolved 09:41 UTC"
        );
        view.documents.clear();
        assert_eq!(
            config_provenance_text(&view),
            "built-in defaults · resolved 09:41 UTC"
        );
    }

    #[test]
    fn bar_keeps_warnings_and_missing_edit_source_visible() {
        let mut view = view();
        view.tier_warnings = vec!["retired model-tier".to_string()];
        let bar = config_bar(&ConfigState::Accepted {
            answer: answer(view),
            stale: None,
        });
        assert_eq!(
            bar.state,
            StatePresentation {
                word: "warning",
                role: StateRole::Warn
            }
        );
        assert!(bar.detail.join(" ").contains("retired model-tier"));
        assert_eq!(bar.actions[0].label, "no editable source");
        assert_eq!(bar.actions[0].tone, ActionTone::Muted);
    }

    #[test]
    fn shared_provenance_and_verdict_keep_bar_and_preview_parts_aligned() {
        let mut view = view();
        view.documents.push(ConfigDocument {
            layer: "repo".to_string(),
            path: "/work/runtime.toml".to_string(),
        });
        let state = ConfigState::Accepted {
            answer: answer(view.clone()),
            stale: None,
        };
        let provenance = config_provenance(&view);
        let bar = config_bar(&state);
        assert_eq!(bar.state, config_verdict(&view));
        assert!(bar.detail[0].contains(&provenance.citation));
        assert!(bar.detail[0].contains(&provenance.instant));
    }

    #[test]
    fn bar_offers_the_placeholder_action_for_a_served_edit_target() {
        let mut view = view();
        view.edit_target = Some("/work/acme/repo/.ctx/traits/runtime.ts".to_string());
        let bar = config_bar(&ConfigState::Accepted {
            answer: answer(view),
            stale: None,
        });
        assert_eq!(bar.actions[0].id, BarActionId::EditRuntimeToml);
        assert_eq!(bar.actions[0].label, placeholders::EDIT_RUNTIME_TOML.label);
        assert_eq!(bar.actions[0].tone, ActionTone::Primary);
    }

    #[test]
    fn stale_answer_retains_chrome_behind_unavailable_marker() {
        let state = fold_config_result(
            &ConfigState::Accepted {
                answer: answer(view()),
                stale: None,
            },
            Err("center down".to_string()),
        );
        assert!(config_header(&state).summary.contains("stale: center down"));
        assert_eq!(config_bar(&state).detail, vec!["stale: center down"]);
    }
}
