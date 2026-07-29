// Model-view types.
/// Model-view type definitions.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{Finding, Severity, scan_hidden_content};
use crate::digest::Digest;
use crate::render::ExtendedRenderProfile;
use crate::resource_plan::{Inclusion, Plan, plan_resource_inclusion};
use crate::r#trait::Trait;
use crate::r#trait::guidance::GuidanceItem;

type BuiltinGuidance = &'static crate::builtins::BuiltinDefinition;
type BuiltinGuidanceLookup = fn(&str) -> Option<BuiltinGuidance>;

/// One section of compiled model-visible text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Section {
    /// The section heading (e.g. `Intent`, `Behavior`, `Model`).
    pub heading: String,
    /// The compiled text content.
    pub content: String,
}

/// A record of a field excluded from model-visible compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Exclusion {
    /// The canonical field path.
    pub field: String,
    /// Why it was excluded.
    pub reason: String,
}

/// A hidden/deceptive-content normalization action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum NormalizationAction {
    /// Content was removed from the model-visible output.
    Removed,
    /// Content was replaced by a visible placeholder or safe spelling.
    Replaced,
}

/// Structured record of a model-visible sanitization step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Normalization {
    /// Field or source being normalized.
    pub source: String,
    /// Finding/code kind being normalized.
    pub code: String,
    /// Action taken.
    pub action: NormalizationAction,
    /// Number of occurrences affected.
    pub count: usize,
    /// Human-readable detail.
    pub message: String,
}

/// The compiled model-visible output for a trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct Report {
    /// The trait ID.
    pub trait_id: String,
    /// The trait version.
    pub trait_version: String,
    /// The render profile used.
    pub profile: ExtendedRenderProfile,
    /// Source digest of the authored trait text, if supplied by the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<Digest>,
    /// Compiled model-visible sections.
    pub sections: Vec<Section>,
    /// The full compiled text: the authoring envelope (rule 3's four
    /// behavior sections plus the remaining reviewer-facing sections).
    /// Consumed by `export`, `check`/`drift`/lock digests, `eval`, `diff`.
    pub full_text: String,
    /// SHA-256 digest of the authoring envelope's inner body (rule 9: this
    /// redefines `content_digest` from "digest of `full_text`" to "digest of
    /// the authoring envelope's body", so the `model-view` attribute on
    /// `<trait>` and this digest always agree).
    pub content_digest: Digest,
    /// The behavior envelope: rule 3's four sections only (`<summary>`,
    /// `<intent>`, `<behavior>`, `<resource>`), zero procedure/prompt/agent/
    /// port/signal/activation/relations/scenario content. This is what
    /// injection consumers (`ctx traits prompt`, `context plan`, host hooks)
    /// emit.
    pub behavior_text: String,
    /// SHA-256 digest of the behavior envelope's inner body.
    pub behavior_digest: Digest,
    /// Compact ID-only projection for budget-constrained context injection.
    pub summary_text: String,
    /// SHA-256 digest of the summary envelope's inner body.
    pub summary_digest: Digest,
    /// Human-readable warnings about compilation.
    pub warnings: Vec<String>,
    /// Structured hidden-content normalization records.
    pub normalizations: Vec<Normalization>,
    /// Fields excluded from model-visible output and why.
    pub exclusions: Vec<Exclusion>,
    /// Post-compile audit findings against `full_text`.
    pub post_audit_findings: Vec<Finding>,
}

impl Report {
    /// Select the exact artifact a load level makes visible. Discovery is
    /// metadata-only and therefore deliberately has no content artifact.
    pub fn artifact_for_load_level(
        &self,
        level: crate::resolve::LoadLevel,
    ) -> Option<(&str, &Digest)> {
        match level {
            crate::resolve::LoadLevel::Discovery => None,
            crate::resolve::LoadLevel::Summary => Some((&self.summary_text, &self.summary_digest)),
            crate::resolve::LoadLevel::Full => Some((&self.behavior_text, &self.behavior_digest)),
        }
    }
}
