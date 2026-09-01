//! Config-pane state folded exclusively from the center's served answer.

use ctx_traits_io::center::ConfigWireResult;

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

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_traits_io::config_view::{ConfigResolution, ConfigTrustRow, ConfigView};

    fn answer() -> ConfigWireResult {
        ConfigWireResult {
            repo_key: "repo".to_string(),
            repo_path: "/repo".to_string(),
            resolution: ConfigResolution::Resolved(ConfigView {
                seats: vec![],
                runtime: vec![],
                trust: ConfigTrustRow {
                    approved_digests: 0,
                    approved_members: vec![],
                },
                documents: vec![],
                tier_warnings: vec![],
                instant_epoch_millis: 0,
            }),
        }
    }

    #[test]
    fn rejected_refresh_keeps_the_served_answer_marked_stale() {
        let previous = ConfigState::Accepted {
            answer: answer(),
            stale: None,
        };
        let state = fold_config_result(&previous, Err("center down".to_string()));
        match state {
            ConfigState::Accepted { answer, stale } => {
                assert_eq!(answer.repo_key, "repo");
                assert_eq!(stale.as_deref(), Some("center down"));
            }
            ConfigState::Loading | ConfigState::Failed(_) => {
                panic!("accepted answer was discarded")
            }
        }
    }
}
