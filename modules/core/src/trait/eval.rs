//! Eval declarations and generated eval-result lock data.
//!
//! `[[eval]]` declares checks that can be run. It does not run them by itself.
//! MVP variants are `documentation`, `lint`, and `golden-render`. Future
//! variants — `behavioral` and `runtime` — are declared but may be unsupported
//! for execution in the initial implementation path.
//!
//! `EvalResult` records are generated evidence from passing eval runs. They
//! are not hand-authored and should be stored in generated reports and lock
//! files, then surfaced through `ctx traits check`.
//!
//! Canonical authoring uses singular `scenario` (scalar-or-array) for
//! repeatable scenario coverage refs, matching MVP authoring conventions.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::digest::Digest;

use crate::reference::{Kind, Reference};

/// Allowed eval input/output ref kinds for MVP.
const ALLOWED_EVAL_REF_KINDS: &[Kind] = &[Kind::Render, Kind::Fixture, Kind::Trait, Kind::Report];

/// The variant of an eval declaration.
///
/// MVP variants (`documentation`, `lint`, `golden-render`) have initial
/// implementation paths. Future variants (`behavioral`, `runtime`) are
/// declared but may be unsupported for execution.
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
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum EvalVariant {
    /// Documentation completeness or accuracy check.
    Documentation,
    /// Static lint or structural check.
    Lint,
    /// Golden-file render comparison.
    GoldenRender,
    /// Behavioral check (future harness).
    Behavioral,
    /// Runtime check (future harness).
    Runtime,
}

impl EvalVariant {
    /// Parse a variant from its kebab-case string form.
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    /// The canonical kebab-case string for this variant.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Whether this variant is supported for execution in the initial MVP
    /// implementation path.
    pub fn is_mvp_supported(&self) -> bool {
        matches!(self, Self::Documentation | Self::Lint | Self::GoldenRender)
    }
}

/// An `[[eval]]` declaration: a declared product evaluation check.
///
/// Eval declarations describe checks that can be run against a trait. They do
/// not execute by themselves. The `input` and `output` fields are product
/// refs (e.g. `render:agent-skills`, `fixture:fixtures/example.md`,
/// `trait:self`, `report:scenario-shape`), not repository test fixtures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[schemars(rename_all = "kebab-case")]
pub struct Eval {
    /// Stable eval identifier (e.g. `"agent-skills-render"`).
    pub id: String,

    /// Eval/check type.
    pub variant: EvalVariant,

    /// What the eval consumes (e.g. `render:agent-skills`, `trait:self`,
    /// `fixture:fixtures/example.md`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,

    /// Expected fixture, report, or comparison output (e.g.
    /// `fixture:fixtures/example.md`, `report:scenario-shape`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,

    /// Scenario IDs this eval covers (references to `[[scenario]]` IDs).
    ///
    /// Canonical authoring uses singular `scenario` (scalar-or-array):
    /// `scenario = "happy-path"` or
    /// `scenario = ["happy-path", "edge-case"]`.
    #[serde(
        default,
        rename = "scenario",
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::shared::deserialize_string_list"
    )]
    pub scenarios: Vec<String>,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a list of eval declarations.
///
/// Checks:
/// - IDs are valid slugs and unique per trait
/// - Scenario coverage refs are valid slug-shaped IDs
/// - Input/output refs parse as typed refs with allowed kinds
///   (render, fixture, trait, report)
///
/// Returns the first error found, or `Ok(())`.
pub fn validate_evals(evals: &[Eval]) -> crate::Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (i, eval) in evals.iter().enumerate() {
        let id_path = format!("eval[{i}].id");
        crate::shared::validate_slug_shape(&eval.id, &id_path)?;

        if !seen.insert(eval.id.clone()) {
            return Err(crate::manifest::Error::InvalidField {
                field_path: id_path,
                message: format!("duplicate eval id {:?}", eval.id),
            }
            .into());
        }

        for (j, scenario_ref) in eval.scenarios.iter().enumerate() {
            let ref_path = format!("eval[{i}].scenario[{j}]");
            crate::shared::validate_slug_shape(scenario_ref, &ref_path)?;
        }

        if let Some(ref input) = eval.input {
            let field = format!("eval[{i}].input");
            validate_eval_ref(input, &field)?;
        }

        if let Some(ref output) = eval.output {
            let field = format!("eval[{i}].output");
            validate_eval_ref(output, &field)?;
        }
    }

    Ok(())
}

/// Validate that a string is a typed ref with an allowed eval kind.
fn validate_eval_ref(ref_text: &str, field_path: &str) -> crate::Result<()> {
    let parsed = Reference::parse(ref_text).map_err(|_| crate::manifest::Error::InvalidField {
        field_path: field_path.to_string(),
        message: format!("invalid typed ref {ref_text:?}"),
    })?;

    if !ALLOWED_EVAL_REF_KINDS.contains(&parsed.kind()) {
        let allowed: Vec<&str> = ALLOWED_EVAL_REF_KINDS.iter().map(|k| k.as_str()).collect();
        return Err(crate::manifest::Error::InvalidField {
            field_path: field_path.to_string(),
            message: format!(
                "ref kind {:?} not allowed for eval input/output; expected {}",
                parsed.kind(),
                allowed.join(", "),
            ),
        }
        .into());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Audit (P44)
// ---------------------------------------------------------------------------

/// An eval audit warning kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum EvalAuditKind {
    /// The eval variant is not supported for execution in the initial MVP.
    UnsupportedVariant,
    /// The eval references a scenario ID that is not declared.
    UnresolvedScenarioRef,
}

/// An eval audit finding (advisory, not a hard failure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EvalAuditWarning {
    /// The eval ID.
    pub eval_id: String,
    /// The kind of audit finding.
    pub kind: EvalAuditKind,
    /// Human-readable detail.
    pub message: String,
}

/// Audit eval declarations for review usefulness.
///
/// Produces advisory warnings (not errors) for:
/// - Evals with variants unsupported for MVP execution (`behavioral`,
///   `runtime`)
/// - Evals that reference undeclared scenario IDs
pub fn audit_evals(evals: &[Eval], scenario_ids: &BTreeSet<&str>) -> Vec<EvalAuditWarning> {
    let mut warnings = Vec::new();

    for eval in evals {
        if !eval.variant.is_mvp_supported() {
            warnings.push(EvalAuditWarning {
                eval_id: eval.id.clone(),
                kind: EvalAuditKind::UnsupportedVariant,
                message: format!(
                    "eval {:?} uses variant {:?} which is not supported for execution in MVP; declaration is preserved but execution is deferred",
                    eval.id,
                    eval.variant.as_str()
                ),
            });
        }

        for scenario_ref in &eval.scenarios {
            if !scenario_ids.contains(scenario_ref.as_str()) {
                warnings.push(EvalAuditWarning {
                    eval_id: eval.id.clone(),
                    kind: EvalAuditKind::UnresolvedScenarioRef,
                    message: format!(
                        "eval {:?} references undeclared scenario {:?}",
                        eval.id, scenario_ref
                    ),
                });
            }
        }
    }

    warnings.sort_by(|a, b| a.eval_id.cmp(&b.eval_id).then(a.kind.cmp(&b.kind)));

    warnings
}

// ---------------------------------------------------------------------------
// Eval-result lock data (generated evidence)
// ---------------------------------------------------------------------------

/// Generated eval-result evidence from a passing eval run.
///
/// These records are generated by the eval runner, not hand-authored. They
/// should be stored in generated reports and lock files, then surfaced
/// through `ctx traits check`. They do not prove behavioral safety — they
/// record exact inputs and generated artifacts for review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct EvalResult {
    /// Which declared eval was run.
    pub eval_id: String,

    /// Timestamp for the passing run (RFC 3339).
    pub evaluated_at: String,

    /// Digest of the eval input (trait content, prompt refs, resource digests
    /// relevant to the run).
    pub input_digest: Digest,

    /// Digest of the output/report content.
    pub output_digest: Digest,

    /// Render or execution profile used (e.g. `"agent-skills"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    /// Warnings produced during the eval run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl EvalResult {
    /// Sort tuple for deterministic ordering:
    /// `(eval_id, profile_or_empty, evaluated_at, input_digest, output_digest)`.
    fn sort_key(&self) -> (&str, &str, &str, &str, &str) {
        (
            &self.eval_id,
            self.profile.as_deref().unwrap_or(""),
            &self.evaluated_at,
            self.input_digest.as_str(),
            self.output_digest.as_str(),
        )
    }
}

impl PartialOrd for EvalResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EvalResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Sort order groups by profile before timestamps; field order is wire order.
        self.sort_key().cmp(&other.sort_key())
    }
}
