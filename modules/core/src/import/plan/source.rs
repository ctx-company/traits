/// Defines sources and evidence for import planning.
/// Import source planning.
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::assist::CandidateLifecycle;
use crate::audit::Finding;
use crate::digest::Digest;
use crate::manifest::PackageStatus;
use crate::r#trait::TrustVerdict;

// ---------------------------------------------------------------------------
// Import source
// ---------------------------------------------------------------------------

/// The source of an import operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ImportSource {
    /// Local filesystem path.
    Local { path: String },
    /// Git URL.
    Git { url: String },
}

/// The Agent Skills-compatible source profile being imported from.
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
pub enum ImportProfile {
    AgentSkills,
    Pi,
    Opencode,
    ClaudeCode,
    Codex,
    Copilot,
    Unknown,
}

impl ImportProfile {
    /// Parse a source profile from a string.
    pub fn parse(s: &str) -> Self {
        s.parse().unwrap_or(Self::Unknown)
    }

    /// Human-readable profile name.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }
}

// ---------------------------------------------------------------------------
// Import report data
// ---------------------------------------------------------------------------

/// A field inferred from the raw source during import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct InferredField {
    /// The canonical field path (e.g. `trait.name`).
    pub field_path: String,
    /// The inferred value as a string.
    pub value: String,
    /// How the value was inferred.
    pub method: InferenceMethod,
}

/// How a canonical field was inferred from raw source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum InferenceMethod {
    /// Extracted from a structured field in the source.
    Direct,
    /// Inferred from heading or title text.
    Heading,
    /// Inferred from file name.
    FileName,
    /// Inferred from body content.
    Body,
}

/// An unsupported field encountered in the source that cannot be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct UnsupportedField {
    /// The source field name or location.
    pub source_field: String,
    /// The raw value.
    pub value: String,
    /// Why it is unsupported.
    pub reason: String,
}

/// A warning about the import process.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ImportWarning {
    /// The source profile could not be determined.
    UnknownProfile,
    /// A field was ambiguous and required manual review.
    AmbiguousField { field_path: String },
    /// The source content has no recognizable trait structure.
    NoTraitStructure,
    /// Raw source files could not be fully preserved.
    IncompletePreservation,
    /// The imported guidance needed prompt-safe normalization.
    SanitizedGuidance,
}

/// A recommended review action for imported content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ReviewAction {
    /// The action to take.
    pub action: ReviewActionKind,
    /// The target field or section.
    pub target: String,
    /// Human-readable detail.
    pub detail: String,
}

/// The kind of review action recommended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum ReviewActionKind {
    /// Verify an inferred field.
    VerifyInferred,
    /// Decide whether to keep or remove an unsupported field.
    DecideUnsupported,
    /// Review unreviewed imported content for safety.
    ReviewUnreviewedContent,
    /// Check for hidden content in imported files.
    CheckHiddenContent,
    /// Confirm or choose the source profile before conversion.
    ConfirmProfile,
}

// ---------------------------------------------------------------------------
// Import request and report
// ---------------------------------------------------------------------------

/// A request to import a source into a canonical trait package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ImportRequest {
    /// The source to import from.
    pub source: ImportSource,
    /// The detected or explicit source profile.
    pub source_profile: ImportProfile,
    /// The raw source digest (computed by IO before planning).
    pub raw_source_digest: Digest,
}

/// The complete import report produced by planning.
///
/// The report records what was inferred, what was unsupported, what warnings
/// apply, and what review actions are recommended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ImportReport {
    /// The source that was analyzed.
    pub source: ImportSource,
    /// The detected source profile.
    pub source_profile: ImportProfile,
    /// The raw source digest.
    pub raw_source_digest: Digest,
    /// Fields inferred from the raw source.
    pub inferred_fields: Vec<InferredField>,
    /// Unsupported fields that cannot be canonicalized.
    pub unsupported_fields: Vec<UnsupportedField>,
    /// Untrusted frontmatter mapping and loss evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<FrontmatterEvidence>,
    /// Deterministic conversion warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversion_warnings: Vec<String>,
    /// Hidden-content findings from the raw imported source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_content_findings: Vec<Finding>,
    /// Warnings about the import.
    pub warnings: Vec<ImportWarning>,
    /// Recommended review actions.
    pub review_actions: Vec<ReviewAction>,
    /// Package status and machine-trust default an import lands at before
    /// human review: draft/unreviewed for every external source.
    pub default_lifecycle: CandidateLifecycle,
    /// Synth provenance once the draft has been normalized into canonical text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synth_provenance: Option<crate::synth::Provenance>,
    /// Explicit managed generated-artifact marker for safe re-import overwrite.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_import: Option<ManagedImportArtifact>,
    /// Multi-file import evidence: entry file, included files, skipped links.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multi_file_evidence: Option<MultiFileEvidence>,
}

/// Structured evidence from a multi-file Agent Skills import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct MultiFileEvidence {
    /// Entry file path (always `SKILL.md`).
    pub entry_file: String,
    /// Included support file paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub included_files: Vec<String>,
    /// Skipped external links with link text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_external: Vec<LinkEvidence>,
    /// Missing links with link text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_links: Vec<LinkEvidence>,
    /// Unsafe links (symlink/path escape) with link text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsafe_links: Vec<LinkEvidence>,
    /// Resource ID mappings from source path to canonical resource ID.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_mappings: Vec<ResourceIdMapping>,
    /// Graph digest when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_digest: Option<Digest>,
}

/// One link evidence entry in multi-file import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct LinkEvidence {
    /// Source file that contained the link.
    pub source_file: String,
    /// Link target as written in Markdown.
    pub target: String,
    /// Link text.
    pub link_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrontmatterEvidence {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_digest: Option<Digest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapped_keys: Vec<FrontmatterMappedKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported_keys: Vec<String>,
    pub trusted_policy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct FrontmatterMappedKey {
    pub source_key: String,
    pub target_field: String,
}

/// Explicit marker proving that a canonical trait was generated by import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct ManagedImportArtifact {
    pub schema_version: String,
    pub generated_by: String,
    pub trait_id: String,
    pub source_profile: ImportProfile,
    pub raw_source_digest: Digest,
    pub trait_digest: Digest,
}

impl ManagedImportArtifact {
    pub const SCHEMA_VERSION: &'static str = "0.1.0";
    pub const GENERATED_BY: &'static str = "ctx-traits-import";

    pub fn new(
        trait_id: impl Into<String>,
        source_profile: ImportProfile,
        raw_source_digest: Digest,
        trait_digest: Digest,
    ) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION.to_string(),
            generated_by: Self::GENERATED_BY.to_string(),
            trait_id: trait_id.into(),
            source_profile,
            raw_source_digest,
            trait_digest,
        }
    }
}

/// A pure Agent Skills import request. IO supplies the already-read source
/// text and digest; core performs only deterministic parsing and planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct AgentSkillsImportRequest {
    pub source: ImportSource,
    pub source_profile: ImportProfile,
    pub raw_source_digest: Digest,
    pub source_path: String,
    pub source_name: String,
    pub skill_markdown: String,
    /// Prior typed checklist items by resource id, for reconciling a
    /// re-import/refresh against an existing managed trait so a reworded
    /// item keeps its id (P405). Empty on a first import, and defaulted so
    /// existing native/WASM planning callers stay compatible.
    #[serde(default)]
    pub prior_checklists: BTreeMap<String, Vec<crate::r#trait::ChecklistItem>>,
}

/// Pure conversion plan for an Agent Skills-compatible source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct AgentSkillsImportPlan {
    pub trait_id: String,
    pub trait_name: String,
    pub summary: String,
    pub draft_json: serde_json::Value,
    pub report: ImportReport,
}

/// Create a baseline import report for a source.
///
/// External sources always default to package status=draft and machine
/// trust=unreviewed.
/// Inferred fields, unsupported fields, warnings, and review actions are
/// populated by the import analysis (future phases); this function creates
/// the baseline shape.
pub fn create_import_report(request: &ImportRequest) -> ImportReport {
    let mut warnings = Vec::new();
    let mut review_actions = vec![
        ReviewAction {
            action: ReviewActionKind::ReviewUnreviewedContent,
            target: import_source_target(&request.source),
            detail: "review imported content before it is approved and activated".to_string(),
        },
        ReviewAction {
            action: ReviewActionKind::CheckHiddenContent,
            target: "imported raw files".to_string(),
            detail: "run hidden-content audit on preserved raw imported files".to_string(),
        },
    ];

    if request.source_profile == ImportProfile::Unknown {
        warnings.push(ImportWarning::UnknownProfile);
        review_actions.push(ReviewAction {
            action: ReviewActionKind::ConfirmProfile,
            target: "source-profile".to_string(),
            detail: "choose or confirm the source profile before conversion".to_string(),
        });
    }

    ImportReport {
        source: request.source.clone(),
        source_profile: request.source_profile.clone(),
        raw_source_digest: request.raw_source_digest.clone(),
        inferred_fields: Vec::new(),
        unsupported_fields: Vec::new(),
        frontmatter: None,
        conversion_warnings: Vec::new(),
        hidden_content_findings: Vec::new(),
        warnings,
        review_actions,
        default_lifecycle: CandidateLifecycle {
            status: PackageStatus::Draft,
            trust: TrustVerdict::Unreviewed,
        },
        synth_provenance: None,
        managed_import: None,
        multi_file_evidence: None,
    }
}
