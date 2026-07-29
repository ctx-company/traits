//! Typed meta-trait scaffold contracts.
//!
//! These structs are deterministic data contracts for import/explain surfaces.
//! LLM-backed traits may emit the same shapes later; core only defines and
//! validates the shape.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::collections::BTreeSet;

use crate::Trait;
use crate::digest::Digest;
use crate::source_map::SourceAnchor;
use crate::source_map::SourceMap;
use crate::r#trait::eval::{Eval, EvalVariant, validate_evals};
use crate::r#trait::scenario::{Scenario, validate_scenarios};

/// Typed scaffold emitted by trait import flows before writing canonical TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct TraitScaffold {
    pub trait_id: String,
    pub source: ScaffoldSource,
    pub declarations: Vec<ScaffoldDeclaration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_edges: Vec<ScaffoldDependencyEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_warnings: Vec<String>,
    pub check: ScaffoldCheckState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScaffoldSource {
    pub kind: String,
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScaffoldDeclaration {
    pub ref_text: String,
    pub kind: String,
    /// Confidence percentage on a closed 0-100 scale.
    pub confidence: u8,
    pub rationale: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<SourceAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScaffoldDependencyEdge {
    pub alias: String,
    pub trait_id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ScaffoldCheckState {
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    pub summary: String,
}

impl TraitScaffold {
    /// Validate scaffold-only invariants before exposing imported evidence.
    pub fn validate(&self) -> crate::Result<()> {
        for (index, declaration) in self.declarations.iter().enumerate() {
            crate::reference::Reference::parse(&declaration.ref_text)?;
            if declaration.confidence > 100 {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("trait-scaffold.declarations[{index}].confidence"),
                    message: "must be between 0 and 100".to_string(),
                }
                .into());
            }
            if let Some(anchor) = declaration.anchor.as_ref() {
                if anchor.start == 0 || anchor.end < anchor.start {
                    return Err(crate::manifest::Error::InvalidField {
                        field_path: format!("trait-scaffold.declarations[{index}].anchor"),
                        message: "must contain a valid one-based inclusive line range".to_string(),
                    }
                    .into());
                }
            }
        }
        Ok(())
    }
}

/// Typed refinement output that retains the proposed trait and source evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RefineScaffold {
    pub source_trait_id: String,
    pub source_digest: String,
    pub proposed_trait: Trait,
    pub patches: Vec<RefinePatch>,
}

/// A source-anchored proposed refinement. Anchors provide review evidence only;
/// the proposed trait still passes the normal candidate gates before any write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct RefinePatch {
    pub change: String,
    pub anchor: SourceAnchor,
}

impl RefineScaffold {
    /// Validate deterministic scaffold invariants before candidate evaluation.
    pub fn validate(&self) -> crate::Result<()> {
        if self.source_trait_id.trim().is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "refine-scaffold.source-trait-id".to_string(),
                message: "must not be empty".to_string(),
            }
            .into());
        }
        Digest::parse(&self.source_digest)?;
        if self.proposed_trait.id.as_str() != self.source_trait_id {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "refine-scaffold.proposed-trait.id".to_string(),
                message: "must equal source-trait-id".to_string(),
            }
            .into());
        }
        if self.patches.is_empty() {
            return Err(crate::manifest::Error::InvalidField {
                field_path: "refine-scaffold.patches".to_string(),
                message: "must contain at least one patch".to_string(),
            }
            .into());
        }
        for (index, patch) in self.patches.iter().enumerate() {
            if patch.change.trim().is_empty() {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("refine-scaffold.patches[{index}].change"),
                    message: "must not be empty".to_string(),
                }
                .into());
            }
            if patch.anchor.file.trim().is_empty()
                || patch.anchor.start == 0
                || patch.anchor.end < patch.anchor.start
            {
                return Err(crate::manifest::Error::InvalidField {
                    field_path: format!("refine-scaffold.patches[{index}].anchor"),
                    message: "must contain a file and valid one-based inclusive line range"
                        .to_string(),
                }
                .into());
            }
        }
        Ok(())
    }
}

/// Typed advisory design review emitted by `ctx traits critique`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ReviewScaffold {
    pub source_trait_id: String,
    pub source_digest: String,
    pub findings: Vec<ReviewFinding>,
}

/// One source-backed advisory design-lint finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ReviewFinding {
    pub rule: String,
    pub message: String,
    pub construct_ref: String,
    pub anchor: SourceAnchor,
}

impl ReviewScaffold {
    /// Validate deterministic review-scaffold invariants and source-map evidence.
    pub fn validate(&self, source_map: &SourceMap) -> crate::Result<()> {
        if self.source_trait_id.trim().is_empty() {
            return invalid_review_field("source-trait-id", "must not be empty");
        }
        Digest::parse(&self.source_digest)?;
        for (index, finding) in self.findings.iter().enumerate() {
            if !matches!(
                finding.rule.as_str(),
                "unbounded-loop"
                    | "unattributed-output"
                    | "missing-review-before-final"
                    | "over-abstraction"
                    | "stringly-reference"
                    | "weak-schema"
                    | "over-broad-trust"
            ) {
                return invalid_review_field(
                    &format!("findings[{index}].rule"),
                    "must be one of the seven critique design-lint rules",
                );
            }
            if finding.message.trim().is_empty() {
                return invalid_review_field(
                    &format!("findings[{index}].message"),
                    "must not be empty",
                );
            }
            crate::reference::Reference::parse(&finding.construct_ref)?;
            if finding.anchor.file.trim().is_empty()
                || finding.anchor.start == 0
                || finding.anchor.end < finding.anchor.start
            {
                return invalid_review_field(
                    &format!("findings[{index}].anchor"),
                    "must contain a file and valid one-based inclusive line range",
                );
            }
            if source_map.get(&finding.construct_ref) != Some(&finding.anchor) {
                return invalid_review_field(
                    &format!("findings[{index}].anchor"),
                    "must exactly match the supplied source-map anchor for construct-ref",
                );
            }
        }
        Ok(())
    }
}

fn invalid_scaffold_field<T>(scaffold: &str, field: &str, message: &str) -> crate::Result<T> {
    Err(crate::manifest::Error::InvalidField {
        field_path: format!("{scaffold}.{field}"),
        message: message.to_string(),
    }
    .into())
}

fn invalid_review_field<T>(field: &str, message: &str) -> crate::Result<T> {
    invalid_scaffold_field("review-scaffold", field, message)
}

/// Source-anchored explanation scaffold emitted by `ctx traits explain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ExplainScaffold {
    pub trait_id: String,
    pub receipt_digest: String,
    pub sections: Vec<ExplainSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ExplainSection {
    pub title: String,
    pub receipt_section: String,
    pub receipt_fact: String,
    pub construct_ref: String,
    pub anchor: SourceAnchor,
}

impl ExplainScaffold {
    /// Validate exact receipt and source-map evidence before exposing an explanation.
    pub fn validate(&self, source_map: &SourceMap) -> crate::Result<()> {
        if self.trait_id.trim().is_empty() {
            return invalid_scaffold_field("explain-scaffold", "trait-id", "must not be empty");
        }
        Digest::parse(&self.receipt_digest)?;
        for (index, section) in self.sections.iter().enumerate() {
            for (field, value) in [
                ("title", &section.title),
                ("receipt-section", &section.receipt_section),
                ("receipt-fact", &section.receipt_fact),
            ] {
                if value.trim().is_empty() {
                    return invalid_scaffold_field(
                        "explain-scaffold",
                        &format!("sections[{index}].{field}"),
                        "must not be empty",
                    );
                }
            }
            crate::reference::Reference::parse(&section.construct_ref)?;
            if section.anchor.file.trim().is_empty()
                || section.anchor.start == 0
                || section.anchor.end < section.anchor.start
            {
                return invalid_scaffold_field(
                    "explain-scaffold",
                    &format!("sections[{index}].anchor"),
                    "must contain a file and valid one-based inclusive line range",
                );
            }
            if source_map.get(&section.construct_ref) != Some(&section.anchor) {
                return invalid_scaffold_field(
                    "explain-scaffold",
                    &format!("sections[{index}].anchor"),
                    "must exactly match the supplied source-map anchor for construct-ref",
                );
            }
        }
        Ok(())
    }
}

/// Build a deterministic source-anchored explanation from a check receipt.
pub fn build_explain_scaffold(
    receipt: &crate::check::CheckReport,
    trait_ref: &Trait,
    source_map: &SourceMap,
) -> crate::Result<ExplainScaffold> {
    let trait_ref_text = format!("trait:{}", trait_ref.id.as_str());
    let trait_anchor = source_map.get(&trait_ref_text).cloned().ok_or_else(|| {
        crate::manifest::Error::InvalidField {
            field_path: "explain-scaffold.source-map".to_string(),
            message: format!("must contain source anchor {trait_ref_text}"),
        }
    })?;
    let receipt_json =
        serde_json::to_string(receipt).map_err(|_| crate::manifest::Error::InvalidField {
            field_path: "explain-scaffold.receipt".to_string(),
            message: "could not serialize check receipt".to_string(),
        })?;
    let mut warnings = Vec::new();
    let sections = receipt
        .sections
        .iter()
        .map(|section| {
            let construct_ref = explain_construct_ref(&section.name, trait_ref, source_map)
                .unwrap_or_else(|| trait_ref_text.clone());
            let (construct_ref, anchor) = match source_map.get(&construct_ref).cloned() {
                Some(anchor) => (construct_ref, anchor),
                None => {
                    warnings.push(format!(
                        "section {} fell back to {} because {} was not anchored",
                        section.name, trait_ref_text, construct_ref
                    ));
                    (trait_ref_text.clone(), trait_anchor.clone())
                }
            };
            ExplainSection {
                title: explain_section_title(&section.name),
                receipt_section: section.name.clone(),
                receipt_fact: section.summary.clone(),
                construct_ref,
                anchor,
            }
        })
        .collect::<Vec<_>>();
    warnings.sort();
    warnings.dedup();
    Ok(ExplainScaffold {
        trait_id: trait_ref.id.as_str().to_string(),
        receipt_digest: Digest::source(&receipt_json).as_str().to_string(),
        sections,
        warnings,
    })
}

fn explain_construct_ref(
    section_name: &str,
    trait_ref: &Trait,
    source_map: &SourceMap,
) -> Option<String> {
    match section_name {
        "io-contract" => trait_ref
            .ports
            .first()
            .map(|port| format!("port:{}", port.id)),
        "sequence" | "control-bounds" => source_map
            .keys()
            .find(|key| key.starts_with("sequence:procedure/"))
            .cloned()
            .or_else(|| {
                source_map
                    .keys()
                    .find(|key| key.starts_with("sequence:"))
                    .cloned()
            }),
        "resources" => trait_ref
            .resources
            .first()
            .map(|resource| format!("resource:{}", resource.id)),
        "scenario-eval-audit" | "eval-evidence" => trait_ref
            .signals
            .first()
            .map(|signal| format!("signal:{}", signal.id)),
        _ => None,
    }
}

fn explain_section_title(section_name: &str) -> String {
    section_name
        .split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Typed generated scenario and eval declarations for a source trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct EvalSynthesisScaffold {
    pub source_trait_id: String,
    pub source_digest: String,
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
    #[serde(default)]
    pub evals: Vec<Eval>,
}

impl EvalSynthesisScaffold {
    /// Validate generated declarations against their source without modifying it.
    pub fn validate(&self, source_trait: &Trait, source_digest: &Digest) -> crate::Result<()> {
        if self.source_trait_id.trim().is_empty()
            || self.source_trait_id != source_trait.id.as_str()
        {
            return invalid_scaffold_field(
                "eval-synthesis-scaffold",
                "source-trait-id",
                "must match the source trait ID",
            );
        }
        let digest = Digest::parse(&self.source_digest)?;
        if &digest != source_digest {
            return invalid_scaffold_field(
                "eval-synthesis-scaffold",
                "source-digest",
                "must match the source trait digest",
            );
        }
        if self.scenarios.is_empty() {
            return invalid_scaffold_field(
                "eval-synthesis-scaffold",
                "scenarios",
                "must contain at least one generated scenario",
            );
        }
        if self.evals.is_empty() {
            return invalid_scaffold_field(
                "eval-synthesis-scaffold",
                "evals",
                "must contain at least one generated eval",
            );
        }
        validate_scenarios(&self.scenarios)?;
        validate_evals(&self.evals)?;
        let source_scenarios = source_trait
            .scenarios
            .iter()
            .map(|scenario| scenario.id.as_str())
            .collect::<BTreeSet<_>>();
        let source_evals = source_trait
            .evals
            .iter()
            .map(|eval| eval.id.as_str())
            .collect::<BTreeSet<_>>();
        for scenario in &self.scenarios {
            if source_scenarios.contains(scenario.id.as_str()) {
                return invalid_scaffold_field(
                    "eval-synthesis-scaffold",
                    "scenarios",
                    "must not reuse a source scenario ID",
                );
            }
        }
        for eval in &self.evals {
            if source_evals.contains(eval.id.as_str()) {
                return invalid_scaffold_field(
                    "eval-synthesis-scaffold",
                    "evals",
                    "must not reuse a source eval ID",
                );
            }
            if !matches!(eval.variant, EvalVariant::Behavioral | EvalVariant::Runtime) {
                return invalid_scaffold_field(
                    "eval-synthesis-scaffold",
                    "evals.variant",
                    "must be behavioral or runtime",
                );
            }
            if eval.scenarios.is_empty() {
                return invalid_scaffold_field(
                    "eval-synthesis-scaffold",
                    "evals.scenario",
                    "must reference at least one source or generated scenario",
                );
            }
        }
        let scenario_ids = source_scenarios
            .into_iter()
            .chain(self.scenarios.iter().map(|scenario| scenario.id.as_str()))
            .collect::<BTreeSet<_>>();
        for eval in &self.evals {
            if eval
                .scenarios
                .iter()
                .any(|scenario| !scenario_ids.contains(scenario.as_str()))
            {
                return invalid_scaffold_field(
                    "eval-synthesis-scaffold",
                    "evals.scenario",
                    "must resolve against source or generated scenarios",
                );
            }
        }
        Ok(())
    }

    /// Merge a validated scaffold into a cloned source trait in canonical order.
    pub fn merge_into(&self, source_trait: &Trait) -> Trait {
        let mut merged = source_trait.clone();
        merged.scenarios.extend(self.scenarios.clone());
        merged.evals.extend(self.evals.clone());
        merged
            .scenarios
            .sort_by(|left, right| left.id.cmp(&right.id));
        merged.evals.sort_by(|left, right| left.id.cmp(&right.id));
        merged
    }
}
