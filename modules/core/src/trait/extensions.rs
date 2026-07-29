//! Namespaced ctx-specific trait extensions.
//!
//! These fields are not portable Agent Traits standard semantics. They are
//! advisory ctx metadata used by review/report surfaces and must not drive
//! activation, resolver scoring, permissions, host execution, or model choice.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::reference::{Kind, Reference};

/// Top-level non-standard extensions namespace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Extensions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx: Option<CtxExtensions>,
}

/// `ctx.traits` runtime/reporting extensions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct CtxExtensions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent: Option<SubagentIntent>,
}

/// Advisory subagent intent metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct SubagentIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_boundary: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_level: Option<String>,

    #[serde(
        default,
        rename = "allowed-tool",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub allowed_tools: Vec<String>,

    #[serde(
        default,
        rename = "allowed-surface",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub allowed_surfaces: Vec<String>,

    #[serde(
        default,
        rename = "required-inherited-trait",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub required_inherited_traits: Vec<String>,

    #[serde(
        default,
        rename = "forbidden-model-hint",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub forbidden_model_hints: Vec<String>,

    #[serde(
        default,
        rename = "forbidden-service-tier-hint",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub forbidden_service_tier_hints: Vec<String>,

    #[serde(
        default,
        rename = "handoff-expectation",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub handoff_expectations: Vec<String>,

    #[serde(
        default,
        rename = "evidence-expectation",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub evidence_expectations: Vec<String>,
}

pub fn validate_extensions(extensions: Option<&Extensions>) -> crate::Result<()> {
    let Some(extensions) = extensions else {
        return Ok(());
    };
    let Some(ctx) = extensions.ctx.as_ref() else {
        return Ok(());
    };
    let Some(subagent) = ctx.subagent.as_ref() else {
        return Ok(());
    };

    validate_optional_text(&subagent.role, "extensions.ctx.subagent.role")?;
    validate_optional_text(
        &subagent.task_boundary,
        "extensions.ctx.subagent.task-boundary",
    )?;
    validate_load_level(&subagent.load_level)?;
    validate_text_list(
        &subagent.allowed_tools,
        "extensions.ctx.subagent.allowed-tool",
    )?;
    validate_text_list(
        &subagent.allowed_surfaces,
        "extensions.ctx.subagent.allowed-surface",
    )?;
    validate_trait_refs(&subagent.required_inherited_traits)?;
    validate_text_list(
        &subagent.forbidden_model_hints,
        "extensions.ctx.subagent.forbidden-model-hint",
    )?;
    validate_text_list(
        &subagent.forbidden_service_tier_hints,
        "extensions.ctx.subagent.forbidden-service-tier-hint",
    )?;
    validate_text_list(
        &subagent.handoff_expectations,
        "extensions.ctx.subagent.handoff-expectation",
    )?;
    validate_text_list(
        &subagent.evidence_expectations,
        "extensions.ctx.subagent.evidence-expectation",
    )?;
    Ok(())
}

fn validate_optional_text(value: &Option<String>, field_path: &str) -> crate::Result<()> {
    if value.as_ref().is_some_and(|text| text.trim().is_empty()) {
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: "must not be empty when present".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_load_level(value: &Option<String>) -> crate::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if !matches!(value.as_str(), "discovery" | "summary" | "full") {
        return Err(crate::manifest::Error::InvalidField {
            field_path: "extensions.ctx.subagent.load-level".to_string(),
            message: "must be discovery, summary, or full".to_string(),
        }
        .into());
    }
    Ok(())
}

fn validate_text_list(values: &[String], field_path: &str) -> crate::Result<()> {
    for (index, value) in values.iter().enumerate() {
        if value.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{index}]"),
                message: "must not be empty".to_string(),
            }
            .into());
        }
    }
    Ok(())
}

fn validate_trait_refs(values: &[String]) -> crate::Result<()> {
    let field_path = "extensions.ctx.subagent.required-inherited-trait";
    validate_text_list(values, field_path)?;
    for (index, value) in values.iter().enumerate() {
        let parsed = Reference::parse(value).map_err(|_| crate::manifest::Error::InvalidField {
            field_path: format!("{field_path}[{index}]"),
            message: format!("invalid typed ref {value:?}; expected trait:*"),
        })?;
        if parsed.kind() != Kind::Trait {
            return Err(crate::manifest::Error::InvalidField {
                field_path: format!("{field_path}[{index}]"),
                message: format!("expected trait:* ref, got {}:*", parsed.kind().as_str()),
            }
            .into());
        }
    }
    Ok(())
}
