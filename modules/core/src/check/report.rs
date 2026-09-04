use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::audit::{Finding, Severity};
use crate::drift::DriftSummary;

use super::{CheckSection, CheckWarning, Section};

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

    use super::CheckReport;
    use crate::check::family_variant_advisories;

    fn metadata(family: Option<&str>, variant: Option<&str>) -> Metadata {
        Metadata {
            family: family.map(Slug::raw),
            variant: variant.map(Slug::raw),
            ..Metadata::default()
        }
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
