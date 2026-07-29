//! Scenario declarations: product review examples for behavior hints.
//!
//! `[[scenario]]` sections declare positive, negative, and edge behavior
//! examples. Scenarios are product review material — they are not executable
//! test cases and do not prove behavior by themselves. Renderers may output
//! `positive` scenarios as examples and `negative` scenarios as anti-examples.
//!
//! Each scenario has a stable ID and variant so later eval-result lock data
//! can reference them deterministically.
//!
//! Canonical authoring uses singular `tag` (scalar-or-array) for repeatable
//! tags, matching MVP authoring conventions.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::shared::SlugList;

/// The behavioral variant of a scenario.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    strum::Display,
    strum::EnumString,
    strum::IntoStaticStr,
)]
#[serde(rename_all = "lowercase")]
#[schemars(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ScenarioVariant {
    /// Expected to work correctly.
    Positive,
    /// Expected to fail, refuse, or warn.
    Negative,
    /// Boundary condition or unusual input.
    Edge,
}

impl ScenarioVariant {
    /// Parse a variant from its lowercase string form.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// The canonical lowercase string for this variant.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

/// A `[[scenario]]` declaration: a product review behavior example.
///
/// Scenarios are behavior hints/specifications for humans, renderers, lint
/// checks, prompt review, and future behavioral eval harnesses. They are not
/// proof by themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Scenario {
    /// Stable scenario identifier (e.g. `"rust-review-positive"`).
    pub id: String,

    /// Behavior variant: positive, negative, or edge.
    pub variant: ScenarioVariant,

    /// Example task or prompt input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,

    /// Expected result, refusal, warning, or behavior description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,

    /// Optional advisory tags for filtering or grouping.
    ///
    /// Canonical authoring uses singular `tag` (scalar-or-array):
    /// `tag = "security"` or `tag = ["security", "rust"]`.
    #[serde(default, rename = "tag", skip_serializing_if = "SlugList::is_empty")]
    pub tags: SlugList,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a list of scenario declarations.
///
/// Checks:
/// - IDs are valid slugs and unique per trait
/// - Tags are slug-validated with canonical field paths
///
/// Returns the first error found, or `Ok(())`.
pub fn validate_scenarios(scenarios: &[Scenario]) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, scenario) in scenarios.iter().enumerate() {
        let id_path = format!("scenario[{i}].id");
        crate::shared::validate_slug_shape(&scenario.id, &id_path)?;

        if !seen.insert(scenario.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate scenario id {:?}", scenario.id),
            }
            .into());
        }

        for (j, tag) in scenario.tags.iter().enumerate() {
            let tag_path = format!("scenario[{i}].tag[{j}]");
            tag.check(&tag_path)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Audit (P44)
// ---------------------------------------------------------------------------

/// A scenario audit warning kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ScenarioAuditKind {
    /// The scenario has no expected output guidance.
    MissingOutput,
    /// The scenario has no input example.
    MissingInput,
}

/// A scenario audit finding (advisory, not a hard failure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScenarioAuditWarning {
    /// The scenario ID.
    pub scenario_id: String,
    /// The kind of audit finding.
    pub kind: ScenarioAuditKind,
    /// Human-readable detail.
    pub message: String,
}

/// Audit scenario declarations for review usefulness.
///
/// Produces advisory warnings (not errors) for:
/// - Scenarios with no expected output guidance
/// - Scenarios with no input example
pub fn audit_scenarios(scenarios: &[Scenario]) -> Vec<ScenarioAuditWarning> {
    let mut warnings = Vec::new();

    for scenario in scenarios {
        if scenario.output.as_ref().is_none_or(|s| s.trim().is_empty()) {
            warnings.push(ScenarioAuditWarning {
                scenario_id: scenario.id.clone(),
                kind: ScenarioAuditKind::MissingOutput,
                message: format!("scenario {:?} has no expected output guidance", scenario.id),
            });
        }

        if scenario.input.as_ref().is_none_or(|s| s.trim().is_empty()) {
            warnings.push(ScenarioAuditWarning {
                scenario_id: scenario.id.clone(),
                kind: ScenarioAuditKind::MissingInput,
                message: format!("scenario {:?} has no input example", scenario.id),
            });
        }
    }

    warnings.sort_by(|a, b| a.scenario_id.cmp(&b.scenario_id).then(a.kind.cmp(&b.kind)));

    warnings
}
