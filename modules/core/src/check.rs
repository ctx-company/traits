//! Check reports: combine validation, audit, activation, lock, resource, and
//! render-readiness information into a single reviewable report.
//!
//! `ctx traits check --locked` detects source, model-view, policy/resource, and
//! export drift once those artifacts exist. This module produces the report
//! shape; it does not execute agent behavior or product evals.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{Finding, Severity};

// ---------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------

/// A digest drift summary comparing expected and actual digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct DriftSummary {
    /// What layer the drift was detected on.
    pub layer: DriftLayer,
    /// The expected (locked) digest.
    pub expected: String,
    /// The actual (current) digest, if computable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Human-readable summary.
    pub summary: String,
    /// Whether this layer's comparison is not yet implemented.
    pub unsupported: bool,
}

/// The layers that can be checked for drift.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum DriftLayer {
    /// Canonical source trait file.
    CanonicalSource,
    /// Compiled model-visible text.
    ModelView,
    /// Resource manifest.
    ResourceManifest,
    /// Policy manifest.
    PolicyManifest,
    /// Generated export.
    Export,
    /// Lockfile state.
    Lock,
    /// Package-local static skill projection lock.
    ProjectionLock,
    /// Vendored dependency source/model/resource evidence.
    Dependency,
}

// ---------------------------------------------------------------------------
// Check report sections
// ---------------------------------------------------------------------------

/// The closed vocabulary of check report sections. A `CheckWarning` or
/// `CheckReport::with_section` naming a section outside this set fails to
/// compile rather than silently dropping the warning from the styled
/// grouped renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub enum Section {
    Validation,
    DerivedKind,
    IoContract,
    ControlBounds,
    Sequence,
    LifecycleStatus,
    MachineTrust,
    Activation,
    Dependencies,
    DependencyTrust,
    Resources,
    ResourceProtection,
    RenderReadiness,
    HiddenContentAudit,
    ScenarioEvalAudit,
    ImportEvidence,
    RuntimeEvidence,
    EvalEvidence,
    ModelView,
    ProjectionLock,
    CdkDrift,
    CdkAuthoring,
    RunConfig,
    Extensions,
    Metadata,
    CandidateLifecycleTrust,
    /// Fork provenance (`[forked-from]` in `trait.toml`, 0213) — present
    /// only for a package `ctx traits fork` produced.
    ForkedFrom,
}

impl Section {
    pub const fn as_str(self) -> &'static str {
        match self {
            Section::Validation => "validation",
            Section::DerivedKind => "derived-kind",
            Section::IoContract => "io-contract",
            Section::ControlBounds => "control-bounds",
            Section::Sequence => "sequence",
            Section::LifecycleStatus => "lifecycle-status",
            Section::MachineTrust => "machine-trust",
            Section::Activation => "activation",
            Section::Dependencies => "dependencies",
            Section::DependencyTrust => "dependency-trust",
            Section::Resources => "resources",
            Section::ResourceProtection => "resource-protection",
            Section::RenderReadiness => "render-readiness",
            Section::HiddenContentAudit => "hidden-content-audit",
            Section::ScenarioEvalAudit => "scenario-eval-audit",
            Section::ImportEvidence => "import-evidence",
            Section::RuntimeEvidence => "runtime-evidence",
            Section::EvalEvidence => "eval-evidence",
            Section::ModelView => "model-view",
            Section::ProjectionLock => "projection-lock",
            Section::CdkDrift => "cdk-drift",
            Section::CdkAuthoring => "cdk-authoring",
            Section::RunConfig => "run-config",
            Section::Extensions => "extensions",
            Section::Metadata => "metadata",
            Section::CandidateLifecycleTrust => "candidate-lifecycle-trust",
            Section::ForkedFrom => "forked-from",
        }
    }
}

impl std::fmt::Display for Section {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl PartialOrd for Section {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Section {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

/// A named section of the check report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CheckSection {
    /// The section name.
    pub name: String,
    /// Human-readable summary text.
    pub summary: String,
    /// Whether the section passed without issues.
    pub ok: bool,
}

/// A structured advisory warning surfaced by `ctx traits check`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CheckWarning {
    /// Report section that produced this warning.
    pub section: Section,
    /// Stable warning code.
    pub code: String,
    /// Field path or resource/ref evidence when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Human-readable warning message.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Check report
// ---------------------------------------------------------------------------

/// The complete check report for a trait.
///
/// Combines validation status, audit findings, activation decisions, resource
/// status, dependency status, render warnings, drift summaries, and
/// unsupported capabilities into a single reviewable report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[schemars(rename_all = "kebab-case")]
pub struct CheckReport {
    /// The trait ID being checked.
    pub trait_id: String,
    /// Whether the trait passed validation.
    pub valid: bool,
    /// Validation error message, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<String>,
    /// Hidden-content audit findings.
    pub audit_findings: Vec<Finding>,
    /// Named check sections.
    pub sections: Vec<CheckSection>,
    /// Structured advisory warning payloads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<CheckWarning>,
    /// Digest drift summaries (locked mode).
    pub drift: Vec<DriftSummary>,
    /// Unsupported capabilities encountered.
    pub unsupported_capabilities: Vec<String>,
    /// Synth provenance records associated with this check report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub synth_provenance: Vec<crate::synth::Provenance>,
    /// Overall pass/fail status.
    pub passed: bool,
}

/// Build the deterministic portability advisory for each `root = "repo"`
/// resource declaration.
///
/// Repo-root resources resolve against the invocation repository rather than
/// the trait package, so they are not portable when the trait package is
/// copied, vendored, or moved to another checkout. This is advisory only: it
/// never changes pass/fail status.
pub fn resource_root_advisories(resources: &[crate::r#trait::Resource]) -> Vec<CheckWarning> {
    resources
        .iter()
        .filter(|resource| resource.effective_root() == crate::r#trait::ResourceRoot::Repo)
        .map(|resource| CheckWarning {
            section: Section::Resources,
            code: "resource-root-repo-coupled".to_string(),
            field: Some(format!("resource.{}.root", resource.id)),
            message: format!(
                "resource {:?} declares root = \"repo\": repo-coupled, not portable across checkouts",
                resource.id
            ),
        })
        .collect()
}

/// Build advisory warnings comparing declared `[metadata] family`/`variant`
/// identity against the trait package ID.
///
/// Display-only consistency check: mismatches never affect activation,
/// resolution, or pass/fail status. Missing metadata, or an empty family
/// and variant, produces no warnings.
pub fn family_variant_advisories(
    trait_id: &str,
    metadata: Option<&crate::r#trait::Metadata>,
) -> Vec<CheckWarning> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let family = metadata.family.as_ref().map(|slug| slug.as_str());
    let variant = metadata.variant.as_ref().map(|slug| slug.as_str());

    let mut warnings = Vec::new();

    let Some(family) = family else {
        if variant.is_some() {
            warnings.push(CheckWarning {
                section: Section::Metadata,
                code: "metadata-variant-without-family".to_string(),
                field: Some("metadata.variant".to_string()),
                message: "metadata.variant is declared without metadata.family".to_string(),
            });
        }
        return warnings;
    };

    let matches = match variant {
        Some("default") => trait_id == family || trait_id == format!("{family}-default"),
        Some(v) => trait_id == format!("{family}-{v}"),
        None => trait_id == family || trait_id.starts_with(&format!("{family}-")),
    };

    if !matches {
        let expected = match variant {
            Some("default") => format!("{family} or {family}-default"),
            Some(v) => format!("{family}-{v}"),
            None => format!("{family} or {family}-*"),
        };
        warnings.push(CheckWarning {
            section: Section::Metadata,
            code: "metadata-family-id-mismatch".to_string(),
            field: Some("metadata.family".to_string()),
            message: format!(
                "declared identity family={family:?} variant={variant:?} expects trait ID {expected:?}, got {trait_id:?}"
            ),
        });
    }

    warnings
}

impl CheckReport {
    /// Create a baseline check report for a trait.
    pub fn new(trait_id: &str, valid: bool) -> Self {
        Self {
            trait_id: trait_id.to_string(),
            valid,
            validation_error: None,
            audit_findings: Vec::new(),
            sections: Vec::new(),
            warnings: Vec::new(),
            drift: Vec::new(),
            unsupported_capabilities: Vec::new(),
            synth_provenance: Vec::new(),
            passed: valid,
        }
    }

    /// Add advisory warning payloads without changing pass/fail status.
    pub fn with_warnings(mut self, warnings: Vec<CheckWarning>) -> Self {
        self.warnings.extend(warnings);
        self.warnings.sort_by(|a, b| {
            a.section
                .cmp(&b.section)
                .then(a.code.cmp(&b.code))
                .then(a.field.cmp(&b.field))
                .then(a.message.cmp(&b.message))
        });
        self.warnings.dedup();
        self
    }

    /// Add a section to the report.
    pub fn with_section(mut self, name: Section, summary: &str, ok: bool) -> Self {
        self.sections.push(CheckSection {
            name: name.as_str().to_string(),
            summary: summary.to_string(),
            ok,
        });
        self.passed = self.passed && ok;
        self
    }

    /// Add audit findings to the report.
    pub fn with_audit(mut self, findings: Vec<Finding>) -> Self {
        let audit_ok = findings
            .iter()
            .all(|finding| matches!(finding.severity, Severity::Advisory));
        self.audit_findings = findings;
        self.passed = self.passed && audit_ok;
        self
    }

    /// Add unsupported capability identifiers to the report.
    pub fn with_unsupported_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.unsupported_capabilities.extend(capabilities);
        self.unsupported_capabilities.sort();
        self.unsupported_capabilities.dedup();
        self
    }

    /// Add synth provenance records to the report.
    pub fn with_synth_provenance(mut self, provenance: Vec<crate::synth::Provenance>) -> Self {
        self.synth_provenance = provenance;
        self
    }

    /// Add drift summaries to the report.
    pub fn with_drift(mut self, drift: Vec<DriftSummary>) -> Self {
        self.drift = drift;
        if !self.drift.is_empty() {
            self.passed = self.passed
                && self
                    .drift
                    .iter()
                    .all(|d| d.unsupported || d.actual.as_deref() == Some(&d.expected));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use crate::shared::Slug;
    use crate::r#trait::Metadata;

    use super::{CheckReport, Section, family_variant_advisories};

    fn metadata(family: Option<&str>, variant: Option<&str>) -> Metadata {
        Metadata {
            family: family.map(Slug::raw),
            variant: variant.map(Slug::raw),
            ..Metadata::default()
        }
    }

    #[test]
    fn no_metadata_produces_no_warnings() {
        assert!(family_variant_advisories("plan", None).is_empty());
    }

    #[test]
    fn empty_metadata_produces_no_warnings() {
        let m = metadata(None, None);
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
    }

    #[test]
    fn variant_without_family_warns() {
        let m = metadata(None, Some("quick"));
        let warnings = family_variant_advisories("plan-quick", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].section, Section::Metadata);
        assert_eq!(warnings[0].code, "metadata-variant-without-family");
        assert_eq!(warnings[0].field.as_deref(), Some("metadata.variant"));
    }

    #[test]
    fn family_only_matching_bare_id_is_clean() {
        let m = metadata(Some("plan"), None);
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
    }

    #[test]
    fn family_only_matching_prefixed_id_is_clean() {
        let m = metadata(Some("plan"), None);
        assert!(family_variant_advisories("plan-quick", Some(&m)).is_empty());
    }

    #[test]
    fn family_only_mismatching_id_warns() {
        let m = metadata(Some("plan"), None);
        let warnings = family_variant_advisories("refactor", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].section, Section::Metadata);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
        assert_eq!(warnings[0].field.as_deref(), Some("metadata.family"));
        assert!(warnings[0].message.contains("plan"));
        assert!(warnings[0].message.contains("refactor"));
    }

    #[test]
    fn matching_non_default_variant_is_clean() {
        let m = metadata(Some("plan"), Some("quick"));
        assert!(family_variant_advisories("plan-quick", Some(&m)).is_empty());
    }

    #[test]
    fn mismatching_non_default_variant_warns() {
        let m = metadata(Some("plan"), Some("quick"));
        let warnings = family_variant_advisories("plan", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn default_variant_accepts_bare_or_family_default_id() {
        let m = metadata(Some("plan"), Some("default"));
        assert!(family_variant_advisories("plan", Some(&m)).is_empty());
        assert!(family_variant_advisories("plan-default", Some(&m)).is_empty());

        let warnings = family_variant_advisories("plan-other", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn hyphenated_family_matches_full_string() {
        let m = metadata(Some("refactor-quick"), None);
        assert!(family_variant_advisories("refactor-quick", Some(&m)).is_empty());
        assert!(family_variant_advisories("refactor-quick-style", Some(&m)).is_empty());

        let warnings = family_variant_advisories("refactor", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn hyphenated_family_with_variant_matches_full_string() {
        let m = metadata(Some("refactor-quick"), Some("style"));
        assert!(family_variant_advisories("refactor-quick-style", Some(&m)).is_empty());

        let warnings = family_variant_advisories("refactor-quick", Some(&m));
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "metadata-family-id-mismatch");
    }

    #[test]
    fn warnings_attach_without_changing_passed() {
        let m = metadata(Some("plan"), Some("quick"));
        let warnings = family_variant_advisories("refactor", Some(&m));
        assert!(!warnings.is_empty());

        let report = CheckReport::new("refactor", true).with_warnings(warnings);
        assert!(report.passed);
        assert!(!report.warnings.is_empty());
    }
}
