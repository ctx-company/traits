//! Compact, repository-scoped runtime configuration projection.

use camino::Utf8Path;
use ctx_traits_core::r#trait::TrustVerdict;
use serde::{Deserialize, Serialize};

use crate::harness_config::{ConfigLayer, ConfigReport};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigCenterIdentity {
    pub version: String,
    pub socket: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigDocument {
    pub layer: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigRuntimeRow {
    pub name: String,
    pub value: String,
    pub qualifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigTrustRow {
    pub approved_digests: usize,
    pub approved_members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigView {
    /// Seats are intentionally a separately extensible list; an empty list is
    /// a valid answer for repositories with no configured assignments.
    pub seats: Vec<ConfigSeatRow>,
    pub runtime: Vec<ConfigRuntimeRow>,
    pub trust: ConfigTrustRow,
    pub documents: Vec<ConfigDocument>,
    /// The authored source for the highest-precedence contributing document.
    /// Defaults for an older center that has not yet served this field.
    #[serde(default)]
    pub edit_target: Option<String>,
    pub tier_warnings: Vec<String>,
    #[serde(with = "epoch_millis")]
    pub instant_epoch_millis: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigSeatRow {
    pub role: String,
    pub seat_index: Option<u32>,
    pub list_length: Option<u32>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub model_winner: Option<ConfigWinnerWire>,
    pub effort_winner: Option<ConfigWinnerWire>,
    #[serde(default)]
    pub used_by: Vec<ConfigSeatUser>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigSeatUser {
    pub member: String,
    pub scope: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigWinnerWire {
    pub layer: String,
    pub source: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "kebab-case")]
pub enum ConfigResolution {
    Resolved(ConfigView),
    Refused { reason: String },
    Failed { reason: String },
}

// `serde_json` does not deserialize `u128` directly. Epoch milliseconds fit
// in `u64`, so retain the public precision while using the JSON protocol's
// supported integer representation.
mod epoch_millis {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &u128, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = u64::try_from(*value).map_err(serde::ser::Error::custom)?;
        serializer.serialize_u64(value)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u128, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(u64::deserialize(deserializer)? as u128)
    }
}

fn layer_name(layer: ConfigLayer) -> String {
    match layer {
        ConfigLayer::BuiltIn => "built-in",
        ConfigLayer::UserGlobal => "user-global",
        ConfigLayer::Repo => "repo",
        ConfigLayer::Environment => "environment",
        ConfigLayer::Flag => "flag",
    }
    .to_string()
}

fn winner(report: &ConfigReport, key: &str) -> Option<ConfigWinnerWire> {
    report.winners.get(key).map(|winner| ConfigWinnerWire {
        layer: layer_name(winner.layer),
        source: winner.source.clone(),
        reason: winner.reason.label().to_string(),
    })
}

fn edit_target(report: &ConfigReport) -> Option<String> {
    report
        .documents
        .last()
        .and_then(|document| document.edit_path.as_ref())
        .map(ToString::to_string)
}

/// Builds one answer from one config report and one already-read trust store.
pub fn resolve_config_view(
    root: &Utf8Path,
    report: ConfigReport,
    document: &crate::trust::Document,
    center: ConfigCenterIdentity,
    instant_epoch_millis: u128,
) -> crate::Result<ConfigView> {
    let context = crate::inventory::InventoryContext::at_repo_root(root)?;
    let library = crate::library::resolve_library(&context, document)?;
    let approved: Vec<_> = library
        .rows
        .iter()
        .filter_map(|row| match row {
            crate::library::LibraryRow::Resolved(member)
                if member.trust == TrustVerdict::Verified =>
            {
                Some((
                    member.id.clone(),
                    member.variant.clone(),
                    member.canonical_digest.clone(),
                ))
            }
            _ => None,
        })
        .collect();
    let approved_digests = approved
        .iter()
        .map(|(_, _, digest)| digest)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let approved_members = approved
        .into_iter()
        .map(|(id, variant, _)| variant.map_or(id.clone(), |variant| format!("{id}.{variant}")))
        .collect();
    let members: Vec<_> = library
        .rows
        .iter()
        .filter_map(|row| match row {
            crate::library::LibraryRow::Resolved(member) => Some(member.as_ref()),
            _ => None,
        })
        .collect();
    let repo_key = crate::state::repo_key(root);
    let mut seats = Vec::new();
    let mut harnesses = Vec::new();
    for (role, configured) in &report.runtime.agent.role {
        let entries = configured.entries();
        let list_length = configured.is_list().then_some(entries.len() as u32);
        for (offset, assignment) in entries.iter().enumerate() {
            let index = configured.is_list().then_some((offset + 1) as u32);
            let key = index.map_or_else(
                || format!("agent.role.{role}"),
                |index| format!("agent.role.{role}.{index}"),
            );
            if let Some(harness) = assignment.harness.clone()
                && !harnesses.contains(&harness)
            {
                harnesses.push(harness);
            }
            let mut used_by = Vec::new();
            for member in &members {
                for agent in &member.agents {
                    let maps_to_seat = agent == role
                        || (role == "default" && !report.runtime.agent.role.contains_key(agent));
                    if !maps_to_seat {
                        continue;
                    }
                    let scoped = crate::harness_config::config_scope_assignment(
                        &report.runtime,
                        &member.id,
                        member.variant.as_deref(),
                        &repo_key,
                        agent,
                        index,
                    );
                    let differs = scoped.model != assignment.model
                        || scoped.reasoning_effort != assignment.reasoning_effort;
                    used_by.push(ConfigSeatUser {
                        member: member
                            .family_key
                            .clone()
                            .unwrap_or_else(|| member.id.clone()),
                        scope: differs.then(|| {
                            scoped
                                .qualifier
                                .unwrap_or_else(|| "scoped configuration".to_string())
                        }),
                        model: differs.then_some(scoped.model).flatten(),
                        reasoning_effort: differs.then_some(scoped.reasoning_effort).flatten(),
                    });
                }
            }
            seats.push(ConfigSeatRow {
                role: role.clone(),
                seat_index: index,
                list_length,
                model: assignment.model.clone(),
                reasoning_effort: assignment.reasoning_effort.clone(),
                model_winner: winner(&report, &format!("{key}.model"))
                    .or_else(|| winner(&report, &format!("agent.role.{role}.model"))),
                effort_winner: winner(&report, &format!("{key}.reasoning-effort"))
                    .or_else(|| winner(&report, &format!("agent.role.{role}.reasoning-effort"))),
                used_by,
            });
        }
    }
    let mut runtime = Vec::new();
    if !harnesses.is_empty() {
        runtime.push(ConfigRuntimeRow {
            name: "harness".to_string(),
            value: harnesses.join(", "),
            qualifier: "configured engine identity".to_string(),
        });
    }
    runtime.extend([
        ConfigRuntimeRow {
            name: "center".to_string(),
            value: center.version,
            qualifier: center.socket,
        },
        ConfigRuntimeRow {
            name: "store".to_string(),
            value: crate::state::global_trait_root()?.to_string(),
            qualifier: "machine-global trait store".to_string(),
        },
    ]);
    let edit_target = edit_target(&report);
    Ok(ConfigView {
        seats,
        runtime,
        trust: ConfigTrustRow {
            approved_digests,
            approved_members,
        },
        documents: report
            .documents
            .into_iter()
            .map(|document| ConfigDocument {
                layer: layer_name(document.layer),
                path: document.path.to_string(),
            })
            .collect(),
        edit_target,
        tier_warnings: report.tier_warnings,
        instant_epoch_millis,
    })
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;
    use crate::harness_config::ConfigReportDocument;

    #[test]
    fn edit_target_uses_the_last_contributor_and_its_authored_source() {
        let report = ConfigReport {
            runtime: crate::harness_config::RuntimeConfig::default(),
            winners: Default::default(),
            tier_warnings: vec![],
            documents: vec![
                ConfigReportDocument {
                    layer: ConfigLayer::Repo,
                    path: Utf8PathBuf::from("generated/runtime.toml"),
                    edit_path: Some(Utf8PathBuf::from("runtime.ts")),
                },
                ConfigReportDocument {
                    layer: ConfigLayer::Environment,
                    path: Utf8PathBuf::from("override.toml"),
                    edit_path: Some(Utf8PathBuf::from("override.toml")),
                },
            ],
            requirement_conflicts: vec![],
        };
        assert_eq!(edit_target(&report).as_deref(), Some("override.toml"));
    }

    #[test]
    fn empty_document_set_has_no_edit_target() {
        let report = ConfigReport {
            runtime: crate::harness_config::RuntimeConfig::default(),
            winners: Default::default(),
            tier_warnings: vec![],
            documents: vec![],
            requirement_conflicts: vec![],
        };
        assert_eq!(edit_target(&report), None);
    }
}
